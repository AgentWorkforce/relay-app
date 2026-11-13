# Existing Oh My Pi sessions

Oh My Pi (`omp`, [omp.sh](https://omp.sh/docs)) is a Pi fork with the same
extension API. The macOS and Linux cores discover and deliver into a running
Oh My Pi session through the same bundled extension as Pi
(`probe/src/relay/pi/agent-relay.mjs`), with the same private per-runtime
inbox, pinned registration, history-confirmed delivery and final answers. See
[Pi delivery](pi-existing-session-injection.md) for the identity and delivery
model; this page lists only what differs.

## Connect

1. Run Agent Relay. Its core installs the extension at
   `~/.omp/agent/extensions/agent-relay.js` when Oh My Pi's agent directory
   exists. `PI_CONFIG_DIR` (a directory name under your home, as Oh My Pi reads
   it) moves that to `~/<PI_CONFIG_DIR>/agent`. `PI_CODING_AGENT_DIR` is not
   read for Oh My Pi, because Pi uses the same name.
2. Oh My Pi loads extensions only when a session starts; it has no `/reload`
   for them. Sessions you start after step 1 connect on their own. For a
   session that was already open, exit it and run `omp --continue` (or
   `omp --resume <id>`) in its folder.
3. Enable **Receives messages** for that Oh My Pi session in Agent Relay's
   session table. Messages arrive as attributed user turns once Oh My Pi is
   idle, and automatic replies use the completed final assistant message.

Sessions appear with the `omp` kind, labelled **Oh My Pi**, and their runtimes
live under `~/.agentworkforce/desktop/omp/`. Registrations pin `omp:` bindings,
so a Pi binding never routes into Oh My Pi or the reverse.
`AGENT_RELAY_ENABLE_OMP=0` turns off discovery, installation and delivery.
Named profiles (`omp --profile`) keep their own agent directory and do not load
the extension. Oh My Pi is unavailable on Windows for the same reason as Pi: the
extension checks file owners with `process.getuid`, which Bun does not provide
there.

If Oh My Pi and Pi share an agent directory (`PI_CONFIG_DIR=.pi`, or two paths
that resolve to one folder), only one copy of `agent-relay.js` can live there.
The copy already installed keeps its harness and runtime, and the other
harness's installation is refused with a message that names the variable to
change, instead of overwriting it. Give Oh My Pi a directory of its own with
`PI_CONFIG_DIR` (or Pi with `PI_CODING_AGENT_DIR`) and restart Agent Relay.

Native mobile continuation (**Continue sessions from your phone**, see
[mobile-native-continuation.md](mobile-native-continuation.md)) is offered for
Pi only: the extension advertises it only when Pi loads it, and the core
never sends owner-native input to an `omp` session. Oh My Pi sessions keep the
attributed relay delivery above.

## Differences from Pi

- **Submitting a turn.** Oh My Pi only queues a `sendUserMessage` that names
  `deliverAs: 'followUp'`, even when it is idle. The extension submits only
  while the session is idle, so for Oh My Pi it sends without `deliverAs`,
  which starts the turn. Pi keeps `followUp`.
- **Switching sessions.** Oh My Pi reports `/new`, `/resume` and forks as
  `session_switch` or `session_branch`, not a second `session_start`. The
  extension re-registers whenever the session ID changes, so the old session's
  inbox is closed and the new one is listed.
- **Titles.** `setSessionName` returns a promise in Oh My Pi. The extension
  confirms a rename only after that promise settles and does not resend a
  rename that is still saving.
- **Transcripts.** Oh My Pi writes `~/.omp/agent/sessions/<project>/<time>_<id>.jsonl`
  lazily, after the first assistant message, with UUIDv7 session IDs. The first
  delivery stays accepted until that file holds the message, as with older Pi.

## Verification

`node --test probe/src/relay/pi/extension.test.mjs` covers both harnesses.
`bash scripts/test-omp-live.sh` installs pinned Oh My Pi 18.12.6 with Bun into a
temporary directory and runs `probe/tests/omp_live_session.rs`: a real Oh My Pi
TUI against a deterministic loopback model, with a fresh home and no
credentials. It checks discovery through `LocalSessions`, idle delivery, a
message waiting unclaimed while another response runs, final answers,
duplicate suppression, the saved JSONL transcript, `/new`, receipts after exit,
and that `omp --resume` gets a new binding that the old registration cannot
use. CI runs it on Linux and macOS.
