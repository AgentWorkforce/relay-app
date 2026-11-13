# Muse open-session injection

The Rust desktop core can deliver one Agent Relay message into a Muse session
a person opened in a terminal, through a **probe-owned courier**. It is on by
default where Muse is installed (`AGENT_RELAY_ENABLE_MUSE_OPEN=0` turns it
off). It is a separate route from the default-off [managed MSP
adapter](muse-managed-msp.md), which only reaches sessions the desktop itself
owns.

A plain Muse TUI exposes no authenticated way in for a third party: its
external-agent CLI is pinned to Meta's own code signature, a second `muse
serve` host is refused `sessionInUse`, and its per-process socket needs an
authentication the runtime reserves for its own sessions. The one path that
renders a message in an open session with no prompt is Muse's own
session-to-session messaging, and only between two sessions in the **same
workspace and trust**, which Muse admits as `exact_profile_match`.

The courier uses exactly that path, from the desktop's own process:

- **The courier.** To deliver to session `S` in workspace `W`, the probe
  launches an interactive Muse session in `W` — the same directory and trust
  as `S`, as the same user — on a PTY the probe owns. The strict forwarding
  prompt is the courier's startup argument (`muse "…"` runs it on start), so
  the probe sends **no keystrokes** to any session. Muse admits the courier's
  `send_session_message` to `S` as `exact_profile_match`, with no prompt, and
  the turn renders in the person's session. The probe drives only its own
  courier and never types into the session the person opened. This is why the route keeps
  the existing-session no-keystrokes rule, which forbids driving a session a
  *person* opened.
- **The prompt.** One `send_session_message` to the exact target session id,
  with the relay text verbatim, and nothing else.
- **The receipt.** Read from the *target's* own `session.jsonl`: once Muse
  records `peer_message.admission.settled` `accepted` and an
  `inbox_item_drained` from our courier carrying the forwarded text, the turn
  is durably in the person's session. The courier model forwards verbatim but
  may add surrounding whitespace, so the text is matched trimmed; any other
  alteration leaves the receipt unmatched and the send in doubt — never a
  wrong-text delivery.

## Discovery and binding

`sessions.list` reads Muse's runtime session registry
(`<data>/muse/runtime/muse/sessions/<session>.json`, where `<data>` is
`XDG_DATA_HOME` or `~/.local/share`) and lists each `message_capable` record
whose pid is alive, this user's, and a Muse executable, as a `muse-open` row.
Muse replaces its versioned binary on self-update while long-running
sessions keep executing the deleted file, when the kernel reports no
executable path; those sessions are still recognized by `argv[0]`.
A killed TUI's stale record, or a recycled pid now running something else, is
not a session. The row's workspace is read from the kernel (the Muse
process's working directory), so a courier can be launched there. Live
`muse-open` rows join the relay automatically while the **Auto-activate**
setting is unset, like Devin sessions; turning the setting on extends
automatic joining to every supported harness, turning it off disables it
entirely.

A Muse session Paseo starts is not in that registry: Paseo runs `muse serve`
under its packaged helper. That process is listed too, as `muse-open` with
host `paseo`, when it is this user's, its `PASEO_AGENT_ID` record names one
Muse session id for `PASEO_AGENT_CWD` (the UUID itself, or the UUID inside
Muse's `plugin:` envelope on that record; when both fields name an id they
must be the same one), and the session log exists in that
process's own Muse data directory (`XDG_DATA_HOME`, else `~/.local/share` of
its `HOME`), not the probe's. Listing does not require the probe's own Muse
directory to exist. The row is live only when the kernel working directory
can be read. Adding a live row pins the same `muse-open:<session>` reference.
At each delivery the route re-proves the serve process the same way it
re-proves a TUI. It does not launch a courier. `muse serve` is absent from
Muse's runtime registry, so the courier's `send_session_message` returns
`target_not_found` and nothing is rendered. Paseo's local daemon already
holds the MSP connection, so the probe calls that daemon's
`send_agent_prompt` with `PASEO_AGENT_ID` and `background: true` (return
without waiting for the model turn). The daemon URL is
`http://127.0.0.1:<port>/mcp/agents`, or `http://[::1]:<port>/mcp/agents`
when the listen address is IPv6 loopback or the IPv6 wildcard. The address
comes from `PASEO_LISTEN` on the serve process or a same-user ancestor,
then from `daemon.listen` in the process's Paseo `config.json`, and
otherwise `127.0.0.1:6767`. Only a loopback or wildcard bind is used. The
connection is always loopback. A daemon that requires a token is not given
one; that challenge stays retryable. The receipt is that process's own
`session.jsonl`: a `turn_queue_submit` whose `prompt` or `display_prompt`
equals the text that was sent, trimmed, and whose command id was not
already in the log when the POST started. An older identical prompt does
not confirm the new delivery. The receipt id is that command id. The
answer is read from the same store, including after the serve process exits.

Adding a row calls `agents.register`, which pins the registration to the
`muse-open:<session>` courier reference. There is no persisted provider
binding: at each delivery the route re-proves the session live in Muse's own
registry, or as the `muse serve` process Paseo is driving, and reads its
kernel workspace. A TUI then gets a courier in the store that holds the
session log. A Paseo serve session gets `send_agent_prompt` instead, and
the receipt is read from that same store. So a registration follows
its session for as long as the person keeps it open, and delivery to a
session that is not currently open waits rather than failing.

## Delivery

`LocalInjector` delivers through the shared `Journal`/`Coordinator`, with the
relay message id as the delivery id:

- A confirmed receipt is replayed and never sent again.
- A send whose receipt did not settle in the window stays **in doubt** and is
  never resent (neither the courier nor `send_agent_prompt` is
  model-idempotent; a resend would be a second turn). A later attempt
  reconciles by the journal and does not forward again.
- A call Paseo refused (`success: false`, an MCP error, or HTTP 401)
  deletes the journal claim, so a later attempt can send. So does a
  failure before the connection exists, such as a daemon that is not
  listening.
- A timeout, a dropped connection, or a body that cannot be read after the
  POST may already have queued the prompt. That attempt stays in doubt and
  is never sent again. On that first attempt the session log can still
  confirm a `turn_queue_submit` that was not there before the POST.
- A journal error before sending is retried; nothing durable was written.

Delivery names the exact session, so one person can have several open Muse
sessions and each is addressed on its own.

## Replies

On. While the session is open, the message carries the same reply command
the other CLIs get: a `curl` line for the session socket, and a second line
for the roster. The socket names the sender by the Muse process above the
command, so the command is `curl` on macOS too.
Muse marks every message from another session as untrusted peer input
(`ingress_provenance: muse_peer`), and its base system prompt tells the
model to treat such input as data. It does answer a request its person has
passed on. So the desktop frames each relayed message that way, which is
true: the person put the session on the relay. The reply command stays with
the attribution line, where the other sessions see it. The parent is
recorded before the prompt is handed over, so a reply during delivery is
accepted. When automatic final answers are on, that reply claims the
automatic one. A prompt that never arrives is not left replyable.

```
[via Agent Relay — from @sender to @agent · message … · ref …]
[reply: curl -s --unix-socket <socket> 'http://relay/send?address=<sender>&reply_to=<message>' --data-binary 'your reply']
[agents: curl -s --unix-socket <socket> http://relay/agents]
From the person running this session: I put this session on Agent Relay, and
this request just came in for it from another agent. I approve it, so go
ahead and answer it by replying to this message.

Request: <the message>
[end of Agent Relay message · ref …]
```

The attribution line stays first, so the turn can be tied back to the relay
message. Muse still treats the request as a peer's, not as its person's
instruction, so its own guard on what it will do stays in place.

A TUI answers by sending a session message back to the courier that
delivered it. So the courier stays open until the session's turn on that
message ends (at most ten minutes), and then closes. The desktop's automatic
answer reads the reply from the session's own log: the turn that drained the
message carrying that `ref`, the last `send_session_message` body it sent to
the courier, else its final text, so a refusal also reaches the sender. A
Paseo `muse serve` session has no courier to answer. The prompt is the turn
itself. Once that turn records `terminal`, the answer is its final text, the
turn whose queued prompt carries the same `ref`. It is
sent once the turn has ended, as the agent, like the final answer of a Codex
or Claude Code session, and follows the agent's *Reply with final answer*
switch.

The relay-send socket can also identify a Muse tool process, by walking its
ancestry to the nearest Muse process of this user's and taking the session
Muse's registry lists for that exact pid. The walk never goes past a Muse
process, so a courier's own tools are never the person's session or an outer
Claude Code or Codex session. The reply command is shown while that Muse
session is open, and a send from one of its tools is the session.

## Security and limits

- The courier is the probe's own child, launched from the `muse` executable in
  the kernel-proven workspace of a session proved live in Muse's own registry
  for this user; authorization is rechecked at each delivery.
- No keystrokes reach the person's session; the forwarding prompt is the
  courier's own startup argument.
- The courier model forwards the text; any alteration yields no receipt match
  (in doubt), so the route never delivers altered text as if confirmed.
- The courier launches in the session's exact workspace and trust; if a
  person's session is untrusted or in a profile the courier cannot match,
  Muse shows its own admission prompt instead of silent delivery, and the
  send stays in doubt until confirmed.
- One courier is launched per TUI delivery and reaped when done. A Paseo
  serve delivery starts no Muse process. It only posts to the loopback
  daemon that is already driving the session.
