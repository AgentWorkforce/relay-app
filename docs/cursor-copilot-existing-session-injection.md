# Cursor and VS Code Copilot: delivery through their hooks

The macOS and Linux cores deliver relay messages into a running Cursor CLI
(`cursor-agent`) session through Agent Relay's hooks in Cursor's
`~/.cursor/hooks.json`. This is turn delivery: a session sees a message at
its next tool call, or when its turn ends. A Cursor CLI session waiting at
its prompt runs no hook, so its messages wait for its person to start the
next turn.

The Cursor editor's agent runs the same hooks and is delivered to the same
way ([Cursor editor conversations](#cursor-editor-conversations)). An
editor conversation that has finished its turn is woken as well: its `stop`
hook waits for the next message and starts a turn with it
([Waking an editor conversation](#waking-an-editor-conversation)). Cursor
Cloud agents are a different route ([Cursor Cloud](cursor-cloud.md)).

The current VS Code **Copilot harness** has a verified native hook route on
macOS ([Copilot in VS Code](#copilot-in-vs-code)). Standalone Copilot CLI and
the older VS Code Local harness remain separate, unsupported delivery routes.

## Connect a Cursor CLI session

1. Settings › Relay › **Cursor sessions** › **Turn On**. The row shows
   once Cursor CLI or the Cursor editor's agent has run on this computer
   (`cli-config.json` or `projects/` in Cursor's data folder). It is off
   unless turned on, like Devin's hooks.
2. Start the Cursor CLI session again if it was open before the hooks were
   on.
3. The session appears in the Agents tab once it has a conversation (after
   its first prompt), titled with the conversation's name. Turn on
   **Receives messages** for it, or let it register itself with
   `POST /register` on the session socket.

`cursor_direct_delivery` is the `settings.get`/`settings.set` key. Only
the person can turn it on; an agent may turn it off.

## Hook file

Turning the setting on adds one entry per event to `~/.cursor/hooks.json`
(Cursor reads user hooks from the home directory, whatever
`CURSOR_CONFIG_DIR` says):

```json
{
  "version": 1,
  "hooks": {
    "postToolUse": [{ "command": "'<probe>' cursor-hook", "timeout": 10 }],
    "stop": [
      { "command": "'<probe>' cursor-hook --wait", "timeout": 3600, "loop_limit": null }
    ],
    "beforeSubmitPrompt": [{ "command": "'<probe>' cursor-hook", "timeout": 10 }]
  }
}
```

The `stop` entry may wait for an hour and has no limit on follow-ups in a
row (Cursor's default `loop_limit` is 5), so it can keep an editor
conversation awake; `beforeSubmitPrompt` only ends such a wait when a newer
turn starts ([Waking an editor conversation](#waking-an-editor-conversation)).
The core brings an earlier install's entries up to these when it starts;
the setting reads as on only once this waiting `stop` entry is in place, so
an earlier install that cannot be rewritten reads as off, and turning it on
tries again and says why it cannot.
Agent Relay Dev adds `--dev` (after `--wait`) and keeps its own inbox. Every other key and
entry is kept, a symlinked file is edited through the link, the mode is
kept and the original is saved once as `hooks.json.relay-desktop-backup`.
A file that is not a JSON object (for example one with comments) is left
unchanged and the row says so. Turning it off removes only Agent Relay's
entries for this build.

The Cursor editor reads the same file, and picks up a change to it without
a restart. Its agent runs the hook too, and its conversations are delivered
to the same way ([Cursor editor conversations](#cursor-editor-conversations)).

## Cursor editor conversations

The editor's agent keeps no conversation file open: one extension host per
window (`Cursor Helper (Plugin): extension-host <workspace> [n-m]` on macOS)
runs every agent conversation of that window, subagents included, and runs
the `hooks.json` hooks as its own children, with the conversation in the
payload (`conversation_id`, plus `workspace_roots`; `transcript_path` is
null for subagents). Its tool processes carry `CURSOR_CONVERSATION_ID`.
Verified with Cursor 3.27.22 on macOS.

- **Proof.** The first agent above the hook is an extension host of this
  user's: the kernel's executable is Cursor's
  `…/Contents/Frameworks/Cursor Helper (Plugin).app/…/Cursor Helper (Plugin)`
  and its title says `extension-host`; on Linux, where Cursor sets no
  title, Cursor's `cursor` binary started as the extension host
  (`VSCODE_CRASH_REPORTER_PROCESS_TYPE=extensionHost` in its environment, or
  `--type=extensionHost`). Its executable and its parent's must be Cursor's
  own and of one install (on macOS the parent is that bundle's
  `Contents/MacOS/Cursor`, on Linux the same `cursor` binary), and the
  parent must be Cursor's main process started as the app: its environment
  readable and without `ELECTRON_RUN_AS_NODE` (Cursor run as plain Node
  could start a helper titled like a host), and no `--type=`, `--inspect…`
  or `--remote-debugging…` switch. Cursor's own means: on macOS
  validly signed by Cursor's developer (team `VDXQ22DGB9`) and installed
  under `/Applications` or `~/Applications`, owned by root or this user and
  writable by no one else; on Linux in a
  folder Cursor's packages install to (`/usr/share/cursor`,
  `/usr/lib/cursor`, `/opt/cursor`, `/opt/Cursor`, `/snap/cursor`, or a
  mounted Cursor AppImage under `/tmp/.mount_Cursor…`), owned by root or
  this user and writable by no one else. Programs already running as the
  same user are outside this route's threat model: such a program could
  mount its own `/tmp/.mount_Cursor…`, and the AppImage path is accepted
  knowingly. The editor's own `cursor-agent … worker start`
  directly under such a host is walked past. The conversation must not be
  one any `cursor-agent` of this user's holds (under whichever data folder,
  so a hook's own `CURSOR_DATA_DIR` cannot hide one), or one a record gives
  to another window's host that still runs, however long since its last
  hook. Every process between the hook and the
  host must have a readable environment and show no other
  `CURSOR_CONVERSATION_ID`. Cursor gives a hook no `CURSOR_CONVERSATION_ID`
  (only the payload), so a hook naming none must be what Cursor starts for
  it: the host's own child, or the child of the host's `sh -c` or of the
  window's worker, and unless its own window already holds the
  conversation, Cursor must have written the conversation's transcript in
  the last hour (it is running now), in the window's own data folder: the
  `CURSOR_DATA_DIR` of the extension host or Cursor's main process, else
  `~/.cursor`, never the one the hook's own environment names. Code running
  as this user is out of scope here as for every route: that includes
  Cursor extensions, which run as the user inside the extension host that
  starts the hooks. Such an extension, or a tool of the window that
  replaces its own process with the hook without the variable, can claim a
  conversation that is running now and that no running window holds yet;
  Cursor gives a hook nothing that proves which conversation started it. Claims are taken under
  a lock per conversation, so two windows' first hooks cannot both record
  it; the one that loses hands nothing over. The hook then records, in
  `~/.agentworkforce/desktop/cursor/editor/<conversation>.json` (private),
  which host (pid and start time) ran it, the window's folder and when.
- **Live.** A conversation is live while that same host runs (same pid and
  start time, so a reloaded window is a new host) and ran one of its hooks
  in the last 12 hours. Delivery stages its messages only then; otherwise
  they wait, as for a stopped CLI session.
- **Discovery** lists each live conversation beside the CLI sessions, as a
  `cursor-cli` session hosted by **Cursor**, titled with the first line of
  its first prompt from Cursor's transcript
  (`agent-transcripts/<id>/<id>.jsonl`, or
  `agent-transcripts/<parent>/subagents/<id>.jsonl` for a subagent), in the
  window's folder. A conversation a `cursor-agent` holds is listed as that
  CLI session.
- **Caller identification** ends at the first extension host above the
  caller: the conversation the caller's own `CURSOR_CONVERSATION_ID` names
  (or the processes below the host that show one, since macOS hides an
  Apple binary's environment; if they disagree, nobody), when a hook of
  that conversation already recorded it live under this host; otherwise
  nobody. A tool's environment alone never puts a conversation on a host,
  so an agent can register itself from its second tool call on. Tags
  inherited from an outer agent name nobody inside an editor window.
- **Delivery and settlement** are the CLI's, below. A `stop` follow-up of a
  subagent is looked for in its `subagents/` transcript.

A conversation that has run no hook since the hooks were turned on (or in
12 hours) is not listed until its next tool call or turn.

### Waking an editor conversation

An editor conversation idle in its window runs no hook until its person
starts a turn, so Agent Relay's `stop` entry runs with `--wait`. When an
editor conversation's turn completes with nothing pending, the hook stays
running and answers the conversation's next message as `followup_message`,
which Cursor submits as the next user turn; when that turn ends, the hook
runs and waits again. Cursor marks the turn completed before it runs `stop`
hooks and keeps it so while one runs, so the chat is idle while the hook
waits and its person can type.

A waiting hook takes a message only while all of these hold, and checks
them again under a lock per conversation before it claims, so a message is
claimed and answered by one hook or put back. A message already pending as
the turn ends is answered at once, under the same checks:

- its parent is the process that started it, still under the window's
  extension host, which runs with the same pid and start time (a closed or
  reloaded window ends it without claiming anything);
- it is the conversation's newest waiting hook
  (`inbox.d/waiting/<conversation>.wait`) and waits after the
  conversation's current turn (`<conversation>.turn`, a versioned record
  Agent Relay's `beforeSubmitPrompt` hook writes as every turn starts, a
  follow-up's included, before the turn's other hooks). Every run of Agent
  Relay's hook first ends a waiting hook when either belongs to another
  turn, and a `stop` hook of an older turn that runs late neither claims
  nor waits. While the record is missing, unreadable, corrupt or of another
  version, no hook claims or waits for the conversation, and its messages
  stay queued for its next hooks until the next turn starts: a turn whose
  `beforeSubmitPrompt` did not run, as when Cursor sessions were turned on
  mid-turn, is not woken;
- the conversation's record still gives it to that host and a hook of it
  ran in the last 12 hours, and this build's waiting `stop` entry is still
  in `hooks.json` (read every 15 seconds while there is nothing to claim,
  and before each claim under a lock that turning the setting on or off
  takes too, `inbox.d/.hooks.lock`: turning it off waits for a hand-over
  already under way, and after that no waiting hook hands anything over;
  when that lock cannot be taken, the setting says so and `hooks.json` is
  left as it is).

It looks for a message once a second, a directory listing and a few
`proc_pidinfo` calls; it stops a minute before Cursor's hour runs out and
on SIGTERM, SIGINT or SIGHUP, at its next look and never between a claim
and its answer. Every identity check of the route still applies: the hook
proves its caller as above before it records the conversation and waits.

- **Cursor CLI sessions** run the same `stop` entry but answer at once, as
  before, and keep Cursor's limit of five follow-ups in a row that the
  entry lifts. A waiting hook has not been tried under Cursor CLI.
- **Other tools' `stop` hooks** run beside it: Cursor runs a step's hooks
  together and merges their answers once all have ended, so another tool's
  `followup_message` is applied only when Agent Relay's hook ends, and when
  both answer, the entry Cursor ran later wins. Superset's and Orca's
  answer none. After five relay-started turns in a row, Cursor stops
  running `stop` entries that keep its default limit of five (Superset's,
  Orca's `stop` entry and an older Agent Relay build's, here) until the
  person's next prompt resets the count.
- **Inline edits** (Cmd-K) run `stop` hooks too; they keep no transcript,
  so the route never claims one and nothing waits there.
- **A narrow race remains.** If a message is handed over in the moment
  between a person submitting a prompt and Cursor running that prompt's
  `beforeSubmitPrompt` hooks, Cursor drops the follow-up because a newer
  turn owns the chat (`stop hook follow-up dropped` in its log) while the
  message reads delivered. Nothing a hook does can close that window.

The editor route is not available on Windows. On Linux it is covered by
the unit tests only; the waiting hook has been run live on macOS.

## Identity

A `cursor-agent` process holds its conversation's
`<CURSOR_DATA_DIR or ~/.cursor>/chats/<workspace hash>/<conversation id>/store.db`
open for as long as that conversation is the one it runs, idle or working
(verified with Cursor CLI 2026.09.10 and 2026.10.22). That descriptor is the
proof, the way a held lock is for Devin:

- **Discovery** lists each conversation a live `cursor-agent` of this user
  holds, where the kernel's executable is Cursor's own `node` in
  `cursor-agent/versions/<version>/` (or a binary named `cursor-agent`), the
  store sits in the chats layout and the directory entry is still the inode
  the process holds. A process holding none (a TUI before its first prompt)
  or several is nobody, and a conversation two processes hold is not listed.
  The title is the conversation name Cursor keeps in the store; the folder is
  the process's working directory.
- **Caller identification** ends at the first `cursor-agent` ancestor: its
  conversation is the caller, or nobody. A tag inherited from an outer agent
  (a Cursor started in a Codex terminal inherits `CODEX_THREAD_ID`) does not
  name the caller. So a registered Cursor CLI session gets the same reply and
  agents commands as Claude Code and Codex, and can register itself.
- **The hook** reads Cursor's payload, takes the conversation from
  `conversation_id` and drains that session's inbox only when the hook runs
  under the `cursor-agent` that holds it, and that process is the only one
  holding it (a second Cursor that resumed the same conversation makes
  neither one the session). Anything else answers `{}`.

## Delivery and settlement

Delivery stages `<session>.<message>.msg` in
`~/.agentworkforce/desktop/cursor/inbox.d/` (`inbox-dev.d` for Agent Relay
Dev), private, synced and renamed into place, and reports it accepted. The
delivery stays unacknowledged on Relaycast until it settles. Delivery asks
about it again every few seconds, and each time renews its lease, a `<name>.lease` file beside it whose modification time is
the lease and whose contents are the moment the message was staged, its
place in the queue (file times are too coarse on Linux to order two messages
staged a moment apart). A staged message whose lease is more than two minutes old is dropped
by the hook instead of handed over: its session was taken off the relay, or
the core stopped. Relaycast still holds an unacknowledged delivery, so a core
that asks again stages it again. A message only moves forward, staged →
claimed → drained, and delivery looks in that order, so a retry never stages
again a message the hook is handing over.

1. At a `postToolUse`, the hook answers `{"additional_context": …}`; at a
   `stop` whose turn completed, `{"followup_message": …}`, which Cursor
   submits as the next user turn. A turn the person stopped, or that failed,
   gets no follow-up. Several waiting messages go in one answer, oldest
   first.
2. Before printing, the hook moves each message to `claimed/` and syncs it.
   Once the answer is written it moves them to `drained/`; if the write
   fails they go back to the inbox.
3. Delivery reads `drained/` as delivered and acknowledges. A message still
   in `claimed/` after 30 seconds (a hook answers within moments of its
   claim) is a hook that died after claiming. Cursor writes a `stop` follow-up into its transcript
   (`~/.cursor/projects/<project>/agent-transcripts/<id>/<id>.jsonl`) as a
   user turn, so a claim whose attribution line is there is delivered. A
   `postToolUse` context is given to the model but not written to the
   transcript, so such a claim is in doubt: reported failed and never sent
   again.

So each message is handed to Cursor once or reported in doubt, never twice.
What is not observable is whether Cursor used an `additional_context` it was
given; live runs showed the model reading it (it went on to read the relay
messaging skill and declined a peer's instruction to run a command, as it
should). Tombstones in `drained/` are kept for a day after their message was
claimed, well past the hour Relaycast keeps redriving a delivery, however
many messages are handed over meanwhile.

A `/new` chat in the same TUI holds a new conversation, which is a new
session to put on the relay. Messages for the old one wait.

## Claude Code hooks inside other agents

Cursor CLI also runs the hooks in `~/.claude/settings.json` (its
`claude-user` hook source, verified in Cursor CLI 2026.10.22), and gives them
`CLAUDE_PROJECT_DIR`. A Cursor started from a Claude Code terminal inherits
that session's `CLAUDECODE` and `CLAUDE_PID` too. Copilot CLI and Devin run
Claude-format hooks as well. So Agent Relay's Claude Code SessionStart hook
(`claude_session_hook`) now finds its Claude Code process by walking up from
the hook and stopping at the first agent: Claude Code, or Cursor CLI,
Copilot CLI, Devin, Codex or OpenCode. Under any of the others it answers
`{}` without asking the core, so it no longer tells a Cursor session it is
the Claude session that started it.

## Copilot in VS Code

The current **Copilot** harness has been checked with VS Code 1.144.0
(commit `2c33b50b16c472efe58357d1d2a8a5414bc23fbf`), bundled Copilot Chat
0.72.0 and runtime `1.0.103-unstable.r40031404408.gba6c003` on macOS.
The earlier CLI 1.0.7 findings below do not describe this editor runtime:

- The bundled Microsoft-signed `copilot-runtime --headless --stdio` is a
  child of VS Code's own utility host, itself below the signed main app.
- It holds real `session-state/<id>/inuse.<pid>.hold` descriptors for each
  native session. Verification compares the open inode with the current
  directory entry and checks executable/parent identities and start times.
  A PID filename, copied transcript path or reused PID cannot pass.
- The installed hook command replaces its shell with Agent Relay's helper.
  The session socket checks that exact executable and event argv, its direct
  runtime parent, UID and the native held session before recording a live
  chat or handing over a message. A generic MCP or tool process is not a hook.
- A `postToolUse` hook returns top-level `additionalContext`. The completed
  turn's `agentStop` hook waits without model calls and returns
  `decision: block` with the incoming message as `reason`, resuming the same
  editor chat. A live disposable proof returned `RELAY_INBOUND_HOOK_OK`.
- Ordinary terminal tools have none of `COPILOT_SESSION_ID`,
  `COPILOT_AGENT_SESSION_ID` or `GITHUB_COPILOT_SESSION_ID`. They cannot choose
  one of the runtime's several held chats as a socket caller. SessionStart
  context and message reply instructions link the hosted Relay MCP to this
  chat's registered desktop address instead. Independent editor chats were
  verified to keep separate MCP connections across an A → B → A switch.

The live probes above establish native hook behavior and chat affinity. Full
integration acceptance additionally requires the reviewed desktop build and
hosted service: a disposable chat appears on Relay, receives a workspace DM,
sends its reply through the real `agent-relay` MCP, and has its session history
automatically uploaded and read back. That packaged round trip remains pending
for this backend integration; record the build, native session ID, Relay message
and reply IDs, and durable history receipt when it passes.

The hook exposes only a flattened tool name, not a verified MCP server URL.
The managed Copilot namespace is `agentrelay` (no hyphen), and Desktop signs
only the twelve exact underscore-named Relay tools. It rejects the ambiguous
legacy `agent-relay-*` prefix. `desktop-core/src/copilot_attestation/tool-names.json` records
synthetic outputs from the installed SDK 1.0.73 native naming function, including
the legacy collision between server `agent-relay` / tool `send_relay_message`
and server `agent` / tool `relay-send_relay_message`. This is metadata evidence,
not a live tool round trip. Cloud additionally requires the distinct stored
Copilot provider credential, enrolled key, exact call/native chat proof and
single-use nonce. The proof does not establish cryptographic endpoint identity
from a field the hook does not provide.

Choose **GitHub Copilot (VS Code)** in provider setup and start a new chat
with the **Copilot** harness. On macOS the native `~/.copilot/mcp-config.json`
uses the running app's `copilot-mcp` stdio bridge, pinned to the selected site,
account and workspace. The legacy VS Code global user configuration retains
HTTP for ordinary workspace tools under the distinct `relayhistory` server name.
VS Code can forward its global MCP configuration into the native harness and
stores root enablement by name, so using `agentrelay` for both would collide.
Linux and Windows retain HTTP in both
formats and report native messaging unavailable. Setup also installs the owned
native hook file and a separate no-op `preMcpToolCall` observer file on macOS;
the observer enables the SDK's control journal without changing tool arguments
or permission decisions. Existing user hooks and other
MCP servers stay intact. History uses RelayHistory's existing `copilot` reader;
uploads follow the person's sharing selection and run automatically.

Older Agent Relay builds still write the legacy VS Code entry as `agentrelay`
and can undo this migration. Every app sharing that VS Code profile must use an
updated build; avoid running old and new builds against the same profile at
the same time. After a downgrade or an older development build rewrites the
entry, update that app and repair Copilot again to restore the separate names.

The native pre-tool hook issues a bounded, exact-call local intent rather than
a Cloud proof. The authenticated app bridge redeems it once at dispatch,
rechecking the Microsoft-signed runtime, held session inode, current provider
selection/configuration/grant, registration, account generation and control
journal since issuance. A requested manual permission must have its matching
approved completion; native rule-approved calls retain the SDK's normal policy.
Journals provide additional call/cancellation correlation, not cryptographic
permission or native identity. Relay's twelve native tools are advertised as
model-only; MCP Apps are unsupported for these native identity operations.

Core holds the provider grant, mints the unchanged 60-second Cloud proof at
dispatch and performs one bounded HTTPS POST. Neither credential nor proof is
returned to the stdio helper. A duplicate intent gets a cached result or an
unknown-outcome error, never another POST. Sends receive an idempotency key
before approval if the caller supplied none. Provider repair/removal, grant or
account changes, native cancellation and replaced session files invalidate
unstarted intents. After updating, repair the provider and reconnect the MCP
server (or reload the editor window) so an existing runtime discards its cached
HTTP transport. Project-level overrides must not replace the managed server.

The stop hook waits for at most 59 minutes; VS Code and the chat must stay
running. Cancelling the hook, closing the chat, sleeping the computer or
hitting Copilot's own consecutive-block limits ends that wake opportunity.
A message then waits for a fresh active hook; Relay does not launch an executor
or reopen a closed editor conversation. Delivery is scoped to the current
registration, journalled before handoff and never blindly retried after an
uncertain response. Stop follow-ups require a native queued user message as
receipt; a hook decision alone is not proof of acceptance.

The older **Local** harness gets the MCP connection but has no wake or history
adapter here. Standalone CLI 1.0.7 was previously checked: its PID-named `.lock`
was closed, post-tool output was ignored and the account used then had no model
entitlement. Those observations stay version-specific; this change does not
claim standalone CLI delivery.

## Windows

Cursor CLI delivery is Unix-only for now: the identity proof reads other
processes' open files, which the Windows process table does not provide.
The core reports **Cursor CLI is not available on Windows.** for the setting
and refuses delivery there.

## Verification

Unit tests in `probe/src/relay/cursor_cli/tests.rs` cover identity,
discovery, staging, leases and order, claims, transcript settlement, the
hook's caller check, the Claude hook's agent boundary and the settings
editor. `probe/src/relay/cursor_cli/editor_tests.rs` covers the editor
route's host proof, hook records, liveness, titles and claim rules,
`probe/src/relay/cursor_cli/wait_tests.rs` the waiting `stop` hook (waiting
and answering, a closed window before and during the wait, the newest hook
winning, a newer turn ending it, an older turn's `stop` hook run late, one
hand-over under a race, an answer that did not get out, the wait running
out, turning the setting off and a conversation no longer the window's),
and `probe/src/probe/remote_sources/cursor_tests.rs` the Cloud Agents
connector. `tests.rs` also covers a Cursor CLI session answering at once
under the waiting entry, and an earlier install being brought up to date or
reading as off when it cannot be.
`scripts/test-cursor-live.sh` runs the same path against a real,
disposable Cursor CLI session (`--model auto`, a throwaway project and a
throwaway inbox). Cursor CLI 2026.10.22 runs no project-level hooks, so the
script adds its hook to the person's `~/.cursor/hooks.json` for the run and
puts the file back as it was on exit (if it changed meanwhile, only the
script's own entries are removed).
Its messages come from an agent that does not exist and ask for no reply.

Live runs against Cursor CLI 2026.10.22 confirmed the payload fields, the held
store, `additional_context` reaching the model, and the `stop` follow-up
recorded in the transcript. Runs with this branch's built hook confirmed
three things more. A hook run from outside the holding `cursor-agent`
answers `{}` and leaves the inbox alone. A message staged during a tool call
is handed over at that call, and one staged as the turn ends is handed over
as a recorded user turn. Each is handed over once, with nothing left claimed.
In one earlier run the first message went at the turn's end instead of at
the tool call. That tool call's hook claimed nothing, so the message was
neither lost nor repeated.

The waiting hook was run live with Cursor 3.27.22 on macOS against a
throwaway conversation in a scratch window, whose folder's own
`.cursor/hooks.json` (the editor runs project hooks; the CLI does not) ran
this branch's hook with a disposable home. `wait::tests::live_editor_conversation_is_woken_by_its_messages`
(ignored; `RELAY_CURSOR_LIVE_HOME`, `RELAY_CURSOR_EDITOR_SESSION`) left the
conversation idle for 20 seconds with its hook waiting, then staged one
message, six times in a row with a debug build and three with a release
build in a second conversation. Each was handed over 62 to 150 ms after it
was staged (1.4 to 2.3 s for a hook's first look after a long wait),
recorded once as a user turn within 1.6 to 4.7 s, and the next hook was
waiting about a second after the reply. Cursor ran the entry at
`loop_count` 5 and 6, and kept the chat's status `completed` throughout
each wait. A `beforeSubmitPrompt` for a newer turn ended the waiting hook
within 0.2 s, and removing the waiting entry ended one within 12 s, each
answering `{}` with no turn started. The waiting release build used no
measurable CPU and about 48 MB of memory; its runs that end at once took
38 to 94 ms in Cursor.
