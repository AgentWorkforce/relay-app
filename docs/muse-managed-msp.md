# Muse managed MSP delivery

The Rust desktop core contains a default-disabled Muse adapter for sessions the
desktop app owns through one long-lived `muse serve` process. The app keeps that
process's MSP stdio connection and exposes a separate private Unix control
socket to the delivery adapter. The adapter never starts another Muse host.

This is not injection into an arbitrary already-open Muse TUI. Muse 1.4 refuses
a second MSP host's `session/resume` and `turn/start` with `sessionInUse`. The
TUI's owner-only `ms-*.sock` is peer-message IPC rather than MSP user-turn IPC,
and an external send was rejected as `sender_unverified`. Those routes remain
unsupported until Muse provides an explicit, authenticated, owner-opted-in
user-turn endpoint.

## Immutable binding

Before a managed route can be eligible, its binding pins:

- Relay workspace and owner;
- local uid and canonical workspace root;
- exact session and model;
- app host pid plus kernel start identity;
- host generation (`endpoint_id`);
- non-secret credential principal/source reference (`auth_id`);
- Muse version and MSP schema fingerprint;
- trusted Muse executable path; and
- control socket path, device and inode.

The socket's parent must be owner-only, the socket is mode `0600`, and both
client and server read peer credentials from the kernel (`getpeereid` plus
`LOCAL_PEERPID` on macOS, `SO_PEERCRED` on Linux). A stale/replaced socket,
wrong uid, host generation, session, workspace, auth reference, or schema is
refused before an MSP command.

## Delivery and recovery

The shared delivery journal creates and fsyncs one UUIDv7 `commandId` together
with the complete immutable request hash before the first provider side effect.
For Muse only, an uncertain attempt may be reconciled within the same immutable
managed-host generation by resubmitting the exact same `turn/start` command ID
and byte-identical payload. A changed payload under the same delivery is
refused as `RequestConflict`; no new command ID is minted. Replacing the host,
manager process, or control socket changes the bound generation/identity and is
also refused. This slice does not migrate pending commands between host
generations. Providers without durable command idempotency retain the previous
no-replay behavior.

The bare RPC acknowledgement is not a receipt. Delivery is confirmed when both
of these refer to the exact bound session and acknowledged turn:

1. `turn/start` returns `status: accepted` and echoes the command ID;
2. an authoritative completed `userMessage` carries that command ID, turn ID,
   and exact prompt text.

That boundary proves Muse durably accepted the exact user message. Delivery
does not wait for model execution or claim that the agent turn completed; long
and queued turns therefore do not hold the route for the control timeout.

On same-generation replay, the manager asks `view/page` for durable view events
so a completed user item that arrived while the control client was disconnected
can still settle the original journal record. Timeout, EOF, malformed frames,
and incomplete correlation remain in doubt.

## Evidence and rollout

A disposable Muse 1.4 proof on macOS established the managed path end to end:
the private control client reached an already-open app-owned MSP host; the host
returned a matching session/command/turn and completed user item. A separate
disposable protocol probe showed that Muse
itself returned a value-identical result for an identical UUIDv7 command and
payload across a synthetic host restart, and returned `command_id_conflict` for
a changed payload. The product slice here intentionally makes the narrower
same-generation guarantee: it has no host replacement/rebinding path. A wrong
session was refused, and a stopped manager left no live socket.

The Rust tests exercise the same private-socket exchange and kernel peer checks
on the repository's macOS and Linux jobs, exact UUIDv7 replay after an in-doubt
attempt within one immutable host generation, replacement-generation and
changed-payload refusal, stale identity rejection, and durable acceptance
correlation. Discovery, registration, and the production gate remain disabled
until the desktop app supplies and persists the complete managed-host binding.
