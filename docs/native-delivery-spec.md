# Native message delivery into Claude and Codex sessions

Status: implemented in relay-desktop (PR #2) and verified live on macOS,
2026-09-19/20. Written to carry the approach over to `../relay`.

Versions everything below was verified against: Codex CLI 0.153.3, Codex
desktop app bundling `codex` 0.155.0-alpha.9.2, Claude Code CLI 2.1.274–2.1.278,
Claude desktop 2.2553.1, macOS 25.5 (arm64).

## 1. The idea

Today `../relay` delivers a message to an agent by owning its terminal: it
spawns the CLI inside a PTY, runs a terminal emulator over the output to guess
when the agent is ready, and types the message in as paced keystrokes
(`crates/relay-pty`, `crates/broker/src/{pty_worker,wrap}.rs`, roughly 15K
lines). That only reaches agents relay itself launched, costs a broker process
and a VT parse per agent, and breaks when a vendor restyles its TUI.

Both vendors now ship their own way to hand a running session a message. Using
those instead means:

- **any already-running session is reachable**, including the desktop apps,
  which a PTY can never wrap;
- **nothing runs between messages** — one short command or socket write each;
- **the vendor decides when the session is ready**, so there is no screen
  scraping, pacing or echo verification;
- the message arrives as what it is (a user turn, or a labelled peer message),
  not as forged typing.

| Target | Route | Interface | Arrives as | Completion signal |
| --- | --- | --- | --- | --- |
| Codex desktop app | `codex queue` | public CLI command | user turn | yes — session file |
| Codex terminal | `codex queue` | public CLI command | user turn | yes — session file |
| Claude desktop app (folder sessions) | `claude --print --cloud` | CLI flag, account-gated | user turn | **no** |
| Claude terminal | peer messaging socket | **private**, versioned | labelled peer message | not yet (see 3.4) |

What this does **not** replace: spawning and supervising agents, answering
their trust/permission prompts, reading their output, and every other CLI
(Gemini, Cursor, …). Those still need the PTY path.

## 2. Architecture

```
sender ─▶ inbox ─▶ [validate target] ─▶ [pause] ─▶ [rate limit] ─▶ [sharing policy]
                                                                       │ deny ─▶ rejected (remembered)
                                                                       │ hold ─▶ outbox(held) ─▶ user approves
                                                                       ▼ deliver
                                                              outbox(pending)     durable, per-target FIFO,
                                                                       │          idempotent on message id
                                                              delivery worker ─▶ backend.send(target, text)
                                                                       │               ├─ codex queue
                                                                       │               ├─ claude --print --cloud
                                                                       │               ├─ claude peer socket
                                                                       │               └─ (relay) PTY fallback
                                                                       ▼
                                                              settle ─▶ acks: queued → delivered → completed | failed | …
```

The queue, policy and acks are backend-independent. A backend is three things:

1. **discover** — list targets and whether they are reachable now;
2. **send** — hand over one message, reporting *sent*, *refused (permanent)*,
   *failed (retryable)* or *in doubt*;
3. **settle** (optional) — later say whether the turn started, finished, and
   what the reply was.

In relay-desktop: `CodexOutbox` (SQLite), `CodexSharingPolicy`,
`CodexInbox`/`ClaudeInbox`, `CodexDelivery`/`ClaudeDelivery`, and the backends
`CodexQueueBridge`, `CodexBridge` (legacy socket), `ClaudeBridge`. For relay
this maps to one trait with the PTY injector as just another implementation.

## 3. The routes

### 3.1 Codex — `codex queue` (app and terminal)

```
codex queue --thread <thread-uuid> --message=<text>
→ stdout: "Queued message <item-id> for thread <thread-uuid>."   exit 0, ~0.2 s
→ error:  "Error: No active session found matching '<x>'."        exit 1
```

Codex keeps a durable per-thread queue in `~/.codex/queue_1.sqlite`
(`queued_items(id, thread_id, payload_json, queue_order, …)`; app-server
methods `thread/queue/{add,list,update,delete,reorder,start}`, notification
`thread/queue/changed`). The desktop app's bundled server and every terminal
`codex` open the same database. Whichever process has the thread loaded takes
the item when the thread is idle and runs it as a normal user turn; the row is
deleted when taken. No daemon needs to be running — the CLI writes the row
directly.

- **Discovery:** `~/.codex/state_5.sqlite`, table `threads` (`id`, `source`,
  `title`, `cwd`, `rollout_path`, `updated_at`). `source = 'vscode'` is the
  desktop app, `'cli'` a terminal, `'exec'` a headless run.
- **Which binary:** prefer the one inside the desktop app
  (`/Applications/ChatGPT.app/Contents/Resources/codex`) — it is the build that
  reads the queue for app threads — then the user's CLI. Check
  `codex queue --help` exits 0 before using a binary; older builds lack it.
- **Thread ids must be UUIDs.** `--thread` also matches session *names*; never
  let an externally supplied value be matched that way.
- **Text is in argv** (no stdin form). Attach it as `--message=<text>` so a
  leading dash is never parsed as a flag. It is visible in `ps` briefly.
- **Settle:** the thread's session file (`rollout_path`, JSONL) records
  `task_started`, `item_completed` (the `UserMessage` with its text),
  `task_complete` (`last_agent_message`) and `turn_aborted`, each with a
  `turn_id`. Codex assigns its **own** `client_id` to a queued message and does
  not record the queue item id, so the message must carry its own marker: the
  attribution line ends with `· ref <12 hex>` = first 6 bytes of
  SHA-256(message id). Find the `UserMessage` whose first line carries the ref
  → its `turn_id` → that turn's outcome.
- **States:** after `codex queue` succeeds the message is *handed over*, not
  delivered. It becomes *delivered* when the session file shows its turn, then
  *completed* (with reply) or *aborted*. See 3.1.1 for how the probe reports
  each step.
- **Never re-send on doubt.** "Not in Codex's queue and not in the session
  file" is also what the instant between dequeue and record looks like. Require
  that state to persist (45 s) before retrying; treat an unreadable queue as
  "still waiting"; fail rather than retry once the message has expired. Record
  which route each send took and settle it by that route's rules, not by
  whichever route is available later.
- **Open:** a thread loaded nowhere presumably holds the item until it is next
  opened. The CLI has no `delete` (the app-server API does), so an expired
  message can linger in Codex's queue.

#### 3.1.1 Terminal turns and settlement

A terminal Codex takes queued messages only between turns, one turn per
message, in queue order. A message handed over during a long turn therefore
waits for that turn, however long it runs (typing into the TUI's terminal is
not a route); Agent Relay's Codex hook delivers one sooner, inside the turn
(3.1.2). The probe hands every message
for a thread whose terminal TUI is live straight to that thread's queue, at
once, even mid-turn, and never through another host: the queue is first in,
first out, so a later message cannot overtake an earlier handoff. Whichever
host has the thread runs its queued items first, so while any relay message
(from this probe or another sharing the queue) waits in a thread's queue, a
message for that thread joins the queue behind it instead of starting a turn
through the app-server or a window, even with no terminal running. Each handoff then
carries a settlement, derived only from Codex's own files:

| Settlement | Evidence |
| --- | --- |
| `handed_off` | The queue row bearing the message's `ref` is in `queue_1.sqlite`, and either the rollout shows no running turn or no terminal Codex runs the thread (a turn left open by a killed terminal is not a running turn); or Codex has taken the row and the rollout has not recorded it yet. |
| `waiting_for_turn` | The queue row is there, a terminal Codex runs the thread, and the rollout shows a turn started (`task_started`) with no `task_complete`/`turn_aborted` yet. |
| `visible` | The rollout records the `UserMessage` whose first line carries the `ref`: the message is a turn in the session. |

`handed_off` and `waiting_for_turn` are pending: Relaycast is not acknowledged
until `visible`.

#### 3.1.2 Into a running turn: Agent Relay's Codex hook

Where Agent Relay's Codex hook is installed and trusted (setting
`codex_stop_hook`, *Codex mid-turn delivery*), a message for a registered
agent whose thread a live Codex (terminal, app, app-server or `exec`) is
running a turn of does not wait for that turn. Codex runs the hook after
every tool call (PostToolUse) and as a turn is about to end (Stop), and both
can add text to the running turn (verified with Codex 0.159, 2026-10-26):

| Hook output | What the rollout records |
| --- | --- |
| PostToolUse `{"hookSpecificOutput": {"hookEventName": "PostToolUse", "additionalContext": <text>}}` | a `response_item` `message` with role `developer` and the text as given, right after the tool's output; no event of its own |
| Stop `{"decision": "block", "reason": <text>}` | a `HookPrompt` item in the same turn (and a user `response_item` wrapping it in `<hook_prompt>`); the turn goes on |

Both keep the attribution line first, so the `ref` settles the message from
the rollout like any other (`rollout::RolloutSummary`, tied to the running
turn). The text gains the peer authority note after its header lines, since
PostToolUse context reaches the model as a developer message.

- **Offer.** With no relay message ahead of it in Codex's queue or the
  thread's history, the router offers the message to the hook
  (`codex::turn_hook::TurnInbox`): settlement `waiting_for_turn`, "Waiting —
  handed over at the session's next tool call or as its turn ends". It is not
  sent anywhere else while the turn runs. Conductor's chats keep their own
  Stop-hook route (`stop_hook`), and Relay Connect keeps the queue, as it
  observes an accepted handoff only there.
- **Take.** The hook (`agent-relay-probe codex-stop-hook`) sends only the
  thread, turn, event and `stop_hook_active` to `POST /codex/stop-hook` on the
  session socket. A thread with nothing offered is answered `{}` at once,
  before any identity check, so a tool call is not held up; otherwise the core
  answers only a caller below the trusted Codex holding the thread's writer
  lock, and hands over one message per hook run, first in first out. The hook
  waits at most 5 s for the core, well inside Codex's 10 s hook timeout; any
  failure prints `{}`.
- **Fallback.** Codex runs no Stop hook for an interrupted or failed turn, and
  a turn a killed Codex left open never ends. Once the rollout shows no
  running turn, or no live Codex holds the thread, the router's next pass
  (every 5 s) sends a message no hook took through the usual routes.
- **Exactly once.** Each message is claimed by one route, and the claim is
  written to `codex-turn-hook.json` in the app's data directory before the
  route has the message: `hook` before the hook prints it, `elsewhere` before
  `codex queue`, the app-server or a window is given it. Neither ever becomes
  the other, across restarts. A message the hook handed over is settled by the
  rollout alone: `visible` once recorded, *in doubt* (never resent) if it is
  not within 45 s; its claim stays for the message's 24 h lifetime, so a
  restart before the doubt is recorded still finds it. A claim that cannot be
  written hands nothing over ("Waiting — Agent Relay cannot record which route
  takes the message"). Each change re-reads the file under a lock
  (`codex-turn-hook.lock`), so cores sharing a data directory keep each
  other's claims.
- **Only while delivery offers it.** Texts are kept in memory only, and the
  hook takes an offer only if delivery offered it again within the last
  minute (it does every few seconds while the message waits). One that
  expired on Relaycast, or whose agent left the relay, is not handed over;
  after a restart the next delivery pass offers what still waits. An accepted handoff is observed every 5 s, never written
again, and never age-expired; a row that leaves the queue without reaching the
rollout within 45 s settles *in doubt* and is not resent. Registered agents
report the settlement on `activity.list` entries (`settlement`); Relay Connect
reports it on `GET /connect/status` (`delivery.pending[].settlement`,
`delivery.recent[].settlement`) and on `activity.list` under the Connect
participant's name. The latency from acceptance to `visible` is the remainder
of the session's current turn; it is reported, not bounded.

Legacy route, kept as fallback: the desktop app's local window-routing
socket, version-pinned, app threads only and only while open in a window.

### 3.2 Claude desktop app — `claude --print --cloud`

The desktop app's folder sessions are **cloud sessions** (`session_…` /
`cse_…`): the model loop runs on Anthropic's side; locally the app only runs
`claude --attach-serve cse_…` to serve the folder's file tools. There is no
local socket, and input never passes through the machine.

```
printf '%s' "<text>" | claude --print --cloud <session-id>
→ stdout: "Sent to cloud session.\nSession ID: …\nView: https://claude.ai/code/…"   exit 0, ~1.5 s
→ errors: "invalid session ID: must be a cse_… or session_… tagged ID"
          "cloud session … is archived and cannot accept new messages"
          "Attaching to an existing cloud session is not enabled for your account."
```

- Uses the user's existing `claude` sign-in. **Never read Claude credentials**
  (the desktop keychain entry, `~/.claude/.credentials.json`) to call the
  sessions HTTP API directly.
- **Discovery:** `~/Library/Application Support/Claude/local-agent-mode-sessions/<account>/<org>/remote-session-spaces.json`
  → `{entries: [{sessionId, folders, memoryEnabled}]}`. Read-only.
- **Validate the id** (`^(session|cse)_[A-Za-z0-9]{8,64}$`) before it reaches
  the command line: any other word is read as a flag, or as the description of
  a **new** cloud session.
- **Text on stdin**, run from a neutral working directory so no project
  settings or hooks load. `--output-format stream-json` is refused in this mode.
- **No completion signal.** The command confirms the hand-off only. Acks end at
  *delivered*; there is no reply text. A timed-out or interrupted send cannot be
  checked afterwards, so it is **failed, not retried** (at-most-once).
- Only failures Claude reports before accepting are retried; archived, invalid
  and account-gated are permanent.
- **Outbound caveat:** these sessions run their shell in a Linux sandbox VM,
  not on the Mac. They cannot run local commands; they can write files in their
  connected folder and use MCP tools.

Ruled out: `--channels` (research preview; only sessions *launched* with the
flag, needs MCP config or a plugin), deep links (`claude://code/new?q=`,
`claude://cowork/new?q=` start *new* sessions only), UI automation.

### 3.3 Claude terminal — peer messaging socket

Claude Code sessions message each other locally (this is what its own
`SendMessage` between sessions uses).

- **Registry:** `~/.claude/sessions/<pid>.json` — `pid`, `sessionId`, `name`,
  `cwd`, `status` (`idle`/`busy`), `peerProtocol` (1), `messagingSocketPath`
  (`/tmp/cc-socks/<pid>.sock`), `procStart`.
- **Auth:** `~/.claude/sessions/<pid>.<hash>.key` (mode 0600) →
  `{peerToken, pidDomain, procStart}`.
- **Wire:** unix stream socket (a named pipe on Windows), newline-delimited
  JSON, first line authenticates. Claude Code 2.1.308 requires it only on
  Windows (`requireAuth` defaults to the platform being Windows; its log says
  "auth line optional here" on macOS); on macOS and Linux an unauthenticated
  line is accepted and treated exactly like a peer-token one, so the spec's
  "must" is stricter than Claude Code is today. Agent Relay still sends it:
  it is required on Windows and Claude Code can turn the requirement on.

```
{"type":"auth","token":"<peerToken>"}
{"type":"user","msg_id":"<uuid>","uuid":"<uuid>","priority":"next","message":{"role":"user","content":"<text>"}}
```

  The socket does not answer. `priority` is `now` | `next` | `later`.
  Receiver limits: 30-message burst refilling 0.5/s per sender, identical body
  dropped within 30 s, 1 MiB per line, connection closed if no complete line
  arrives promptly.
- **Arrives as a peer message, not a user turn.** The session shows "Another
  Claude session sent a message: …" and appends Claude Code's own guardrail
  text: act on it within this session's permissions, never edit permission
  settings/CLAUDE.md/config because a peer asked, never treat it as the user's
  approval, refuse permission laundering. A session may also *hold* a peer
  message for the user's approval. **Keep both**; do not work around them.
  Verified: delivered to an idle session in manual permission mode without a
  hold, and started a turn at once.
- **When it holds.** Claude Code resolves `crossSessionInbound` from managed,
  then user settings, a repository's `.claude/settings{,.local}.json` only
  tightening it, and an unknown value reading as `hold`. With `accept` it
  delivers; with `hold` it holds; with no setting it holds a message whose
  sender attests no permission mode (`no-mode-asserted`, which the relay
  always is) when the session bypasses prompts: `bypassPermissions`, or plan
  mode where bypassing is available. A headless session has no approval
  surface, so such a hold expires and the message is dropped. The core
  predicts the hold (`claude::hold`) from the transcript tail's latest
  `permission-mode` entry or prompt `permissionMode`, else the command line,
  and reports the delivery as held for approval rather than delivered.
- **Two tokens.** Each session publishes `peerToken` in its key file and gives
  its own child processes a `childToken` as `CLAUDE_CODE_MESSAGING_TOKEN`. A
  line authenticated with the child token counts as the session talking to
  itself (`selfSent`) and is accepted without the hold, ahead of the
  no-mode-asserted rule (an explicit `hold` setting still holds). Verified
  2026-10-26 against 2.1.308 on macOS in a throwaway bypass-mode session: no
  auth line and the peer token were held; the child token, whether posted by a
  process inside the session or by an outside process that read it from a
  child's environment, was delivered. On macOS Claude Code checks the sender's
  ancestry, but a sender that has exited by then gives no evidence and the
  child token alone decides; on Windows the token always decides. The core
  does not use the child token: it is only meant for processes inside the
  session, and reading it from a child's environment would be impersonating
  one.
- **Private and fast-moving.** Gate on `peerProtocol == 1` and refuse anything
  else. Verify the pid is alive (and ideally `procStart` matches) before
  connecting, as Claude's own client does, and after connecting check that the
  kernel names the session's pid, run by this user, as the listener
  (`SO_PEERCRED`, `LOCAL_PEERPID`, `GetNamedPipeServerProcessId`); otherwise
  nothing is written, so a replaced socket never receives the token.
- **`/clear`** keeps the process, pid, socket and name and rewrites
  `<pid>.json` with a new `sessionId`; the new transcript opens with the
  `/clear` command. The core remembers which process ran each conversation,
  moves an agent to the new conversation in that process (unless it is on the
  relay already), refuses messages sent before the clear, and does not take a
  process the session started before the clear as the new conversation's
  caller.
- **Policy decision outstanding:** this reads another session's credential
  file. It is the same user's file, but it should be an explicit product
  decision, which is why it is only in `scripts/relay-send` so far.

### 3.4 Settling Claude turns (not built)

Claude terminal sessions write transcripts under
`~/.claude/projects/<cwd-slug>/<sessionId>.jsonl`; a settle step like Codex's
could be built on them. Peer control frames (`notify_when_idle`,
`peer_message_status`: held/denied/delivered/refused/dropped/expired) exist but
need a reply socket of the sender's own registered in the same namespace.

## 4. Delivery semantics

- **Idempotent on the sender's message id.** Re-receiving an id re-acks its
  current state, never re-queues.
- **Per-target FIFO.** The next message for a target is not sent until the
  previous one is handed over. A Codex terminal queue keeps their order itself,
  so messages join it behind an earlier handoff without waiting for its turn.
- **Ack ladder:** `queued` → (`held_for_approval` → `queued`) → `delivered` →
  `completed{reply}` | `aborted` | `failed{error}` | `expired` | `rejected{error}`.
  For Claude app targets `delivered` is terminal success. A Codex terminal
  handoff settles `handed_off` → `waiting_for_turn` (while a turn runs) →
  `visible` before it counts as `delivered` (3.1.1).
- **Truthful acks.** A message being sent acks `queued`, never `delivered`.
  Acks for one message are serialized so a later state never precedes an
  earlier one.
- **Remembered rejections.** Refusals that depend on current state (paused,
  rate limit, not shared) are stored, so a repeat after things change replays
  the rejection instead of delivering a message its sender was told was refused.
- **Fail closed.** Default-deny per target; sharing is re-evaluated in full at
  delivery time (unshared → rejected; switched to ask-first → back to held,
  unless that message was already approved). Unknown permission mode is treated
  as unrestricted, which requires a second explicit opt-in to auto-deliver.
- **Attribution.** Every relayed message starts with
  `[via Agent Relay — from <name> · ref <12 hex>]` so neither user nor model
  reads it as the local user's own input, and its text ends with
  `[end of Agent Relay message · ref <12 hex>]`. Control, zero-width and bidi
  characters are dropped from the text, and a line in it that imitates
  either, or a reply line, has its brackets turned to parentheses, so the
  text cannot open a second message or close this one early.
- **Rate limit:** 30 per sender per target per minute; rejections count.
- **Expiry:** 24 h default, shortened (never extended) by the sender.

## 5. Outbound: how an agent sends

Inbound is solved natively; outbound depends on what the agent can do.

| Agent | Can run local commands | Outbound |
| --- | --- | --- |
| Codex app / terminal | yes | MCP tool, or a CLI call |
| Claude terminal | yes | MCP tool, or a CLI call |
| Claude desktop app | **no** (Linux sandbox) | MCP tool; file outbox as a stopgap |

The production answer is the Agent Relay MCP tools relay already gives agents.
A session this app registered has no token of its own, though, so on Linux
the app also serves a Unix socket a session sends through as itself; the
kernel names the caller and the app sends with that session's agent token.
See [Sending from a session](injectable-agents.md#sending-from-a-session-linux).
`scripts/relay-send watch <folder>` is a test fixture: the agent writes
`<folder>/.relay-outbox/<name>.json` = `{"to": "codex:<id>" | "claude:<name>" |
"claude-app:<folder|id>", "text": …, "from": …}` and a watcher delivers it.

## 6. Bringing this to `../relay`

1. **Introduce a delivery-backend trait** beside the PTY injector: `discover`,
   `send`, `settle`. Keep the broker's queue and verification states; map
   *sent/refused/failed/in-doubt* onto them.
2. **Add the Codex backend first.** It is a public command with a real
   completion signal, and it replaces the most fragile code: Codex readiness
   detection, chunked paced writes and echo verification.
3. **Add the Claude peer-socket backend** for terminal Claude, behind the
   `peerProtocol` gate and a config flag, once the credential-file question is
   decided. It removes paste-then-Enter and banner detection.
4. **Keep PTY as the fallback** for CLIs with no native route and for a native
   route that refuses (old binary, unknown protocol, account gate). Spawning,
   supervision and prompt auto-answering are unchanged.
5. **Agents relay launched can use the native route too** — relay knows the
   thread/session id it started, so it does not have to type into its own PTY.
6. **Drop the per-injection MCP reminder block** where the native route is used,
   if its only purpose was surviving TUI redraws; keep it if it carries reply
   instructions.
7. **Platform:** paths here are macOS. On Linux, Codex's state lives under
   `$CODEX_HOME` (default `~/.codex`) the same way; Claude's socket directory is
   `$XDG_RUNTIME_DIR/cc-socks` or `/tmp/cc-socks[-<uid>]`, read it from the
   registry rather than constructing it.

Risks to plan for: `codex queue` and the queue schema can change (detect with
`queue --help`, parse only the confirmation line); the Claude peer protocol is
private (version-gate, fail to PTY); `--cloud` sending is account-gated; both
vendors' desktop apps update themselves, so checks belong at send time, not at
install time.

## 7. Evidence

- Codex app: `CodexInbox → CodexDelivery → codex queue`, acks
  `queued → delivered → completed`, reply read back from the session file.
- Codex terminal: independently started TUI picked up a queued message in
  seconds and answered.
- Claude app: `ClaudeInbox → ClaudeDelivery → claude --print --cloud`, acks
  `queued → delivered`; message visible in the desktop conversation.
- Claude terminal: authenticated peer frame delivered to an independently
  started session and answered.
- Cross-app: the Claude and Codex desktop apps played a full nine-move game of
  tic-tac-toe against each other — five Claude→Codex hops (file outbox →
  `codex queue`) and four Codex→Claude hops (`claude --print --cloud`) — with no
  failed, duplicated or reordered message.
- Logic (policy, ordering, idempotency, retries, expiry, restart durability, ack
  ordering) is covered by XCTests in CI and a local harness; see
  `docs/plan.md` §9–§10 for the contracts.

## 8. Testing by hand

```
scripts/relay-send list                                   # targets on this machine
scripts/relay-send codex <thread-uuid> "…" --from Me      # Codex app or terminal
scripts/relay-send claude-app <folder|session-id> "…"     # Claude desktop app
scripts/relay-send claude <session-name> "…"              # Claude terminal
scripts/relay-send watch <folder>                         # deliver an agent's file outbox
```

When starting throwaway sessions to test against: launching `codex` or `claude`
in an untrusted directory, or answering their first-run prompts, **writes the
user's config**. Use an already-trusted directory, `claude --strict-mcp-config`,
and diff the config afterwards.
