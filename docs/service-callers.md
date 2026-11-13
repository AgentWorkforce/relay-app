# Service callers: the Flows spawn runtime on the desktop core

Status: Desktop's side is implemented in `desktop-core/src/service_callers`
behind the core's spawn-callers hook. Anything marked **Proposed** is not
yet decided. Section 11 records the decisions already settled. The relay
side is relay PR #2103 (`crates/relay-node`, head `8ea590da`). This
document is Desktop's side of it.

Invoke transport at this head: the `agent-relay-node` pin (`26fc34c6`)
carries the legacy `spawn.invoke` transport, and Desktop uses only that.
The strict, replay-safe invoke transport is relay PR #2119; it is not
merged, not pinned and not used by any code here. Desktop moves the pin
only after #2119 merges to relay `main`, and no proof recorded before that
re-pin covers the strict transport.

## 1. Purpose and scope

The Flows local Node spawn runtime is a separate process on the same
computer. It needs to start and drive agents through the desktop core and
read back what they report. Today it cannot. The native spawn controls
(`spawn.invoke`, `spawn.status`, `spawn.release`) accept only a session
registration, named by `caller_name` alone (`caller_token` in
`crates/relay-node/src/relay/spawn/control.rs`). Desktop has no caller
secret and no `caller.json`. The only other way in would be a shared human
credential, which this contract rules out.

What relay-node provides in #2103:

- `Plugins.spawn_callers: Option<Arc<dyn SpawnCallers>>`
  (`crates/relay-node/src/relay/daemon/plugins.rs`). Without it every
  `caller_secret` is refused.
- The `SpawnCallers` trait. `resolve(workspace, name, secret)` returns an
  `ExternalCaller` or a typed refusal. `admit(caller, call)` admits or
  refuses one call before any Relaycast request. `record(permit, outcome)`
  sees what came of an admitted call.
- `ExternalCaller { agent_id, name, token, harnesses, directory_roots,
  owns_only }`: a Relaycast agent whose token stays in the core, and its
  bounds.
- `SpawnCall { peer_pid, workspace, request }` with `SpawnRequest::{Invoke,
  Status, Release}`.
- `CallContext { peer_pid }`, filled by the bridge server from the kernel's
  peer credentials (`crates/relay-node/src/relay/server.rs`), and the new
  `context` argument of `MethodExtension::call`.
- An optional `caller_secret` on `SpawnInvokeParams`,
  `SpawnInvocationParams` and `SpawnReleaseParams`
  (`crates/relay-node/src/relay/contract/spawn_control.rs`). Their
  hand-written `Debug` prints it as `<redacted>`.
- `RefusalReason` (`crates/relay-node/src/relay/protocol.rs`) and
  `RpcError::refused_because`.
- The harness and folder check (`scoped_directory`) and the `owns_only`
  release check (`release_as`, `release_target_ownership`).

One relay-node change beyond #2103 is required, because the result path
(section 8) reads `released` from the receipt: `SpawnControlInvocation`
must carry `released: bool`, true only when the upstream output's
`released` is JSON `true`, and the generated Swift must declare it as a
required `Bool`. #2103's exact head `8ea590da` drops the upstream field, so
a strict consumer can never see a real release. The projection is relay PR
#2108 (`layered/spawn-released-projection`, one commit on `8ea590da`) and
its `@agent-relay/core-bridge` counterpart is PR #2109 (on #2102's head).
Desktop regenerates Swift from the real pin that contains them. Completion
is never read as release.

The core never stores, lists or provisions service callers. Desktop does,
in `desktop-core/`:

- the registry, its secrets and their rotation;
- the management bridge methods and `caller.json`;
- `SpawnCallers::connection`, `admit` and `record`;
- ownership of `spawn.status`. The core does not check it for external
  callers (`invocation_as` has no owner test);
- the result inbox bridge.

Out of scope:

- Any change to `crates/relay-node` beyond #2103 and the `released`
  projection above, and remote callers.
- Human approval authority. A service caller is a Relaycast agent. Per
  [ownership-delegation.md](ownership-delegation.md#authority) an agent id
  identifies an actor and grants no human permission. Its spawns are
  requester spawns as in
  [native-spawn.md](native-spawn.md#who-counts-as-the-owner).
- Placement. The runtime reads `spawn.targets` and names the node, as
  [node-lifecycle.md](node-lifecycle.md#placement-stays-with-the-requester)
  describes. `spawn.invoke` still starts agents only on this computer's own
  node.

## 2. Terms

- **Service caller.** A local process outside any session that the
  signed-in person allowed to call the spawn controls under its own name.
  It is backed by its own Relaycast agent in the current workspace.
- **Caller granularity.** One caller per Flows project or worker profile,
  shared by that project's parallel tasks (settled, section 11). Never one
  broad install-wide caller, and never one per run. Distinct project scopes
  get distinct callers, each with its own rotation and revocation.
- **Publisher identity.** That Relaycast agent, as `{id, name}`. Spawned
  agents report to it. Its token never leaves the core.
- **Caller id.** Desktop's opaque, immutable registry id for one caller
  (`sc_` plus 26 base32 characters, **Proposed**). Never reused.
- **Caller name.** The value sent as `caller_name`, and the Relaycast agent
  name. It matches the core's `segment` rule (1 to 200 of
  `A-Z a-z 0-9 _ -`). Unique per workspace.
- **Caller secret.** A random value in `caller.json` that never crosses
  the bridge. The runtime uses it only as the key input of the
  connection-bound challenge ([service-callers-auth.md](service-callers-auth.md)).
  Desktop keeps only its hash.
- **caller.json.** The private file that hands one caller's name, secret
  and bounds to the runtime (section 5).
- **Owner (`owns_only`).** The caller that started a spawn. Every Desktop
  caller has `owns_only: true`: it reads, releases and receives results for
  its own spawns only.
- **Scope.** `ExternalCaller.harnesses` and `directory_roots`.
- **Result inbox.** Durable, Desktop-held deliveries to the publisher
  identity, consumed over the same authenticated socket (section 8).
- **Iterator.** One task's cursor over the inbox. Many iterators share one
  caller identity.

## 3. Registry

Desktop keeps one registry per account and workspace in the app data
directory (`~/Library/Application Support/com.agentrelay.desktop` on macOS,
`$XDG_DATA_HOME/com.agentrelay.desktop` on Linux, as in
[relay-core.md](relay-core.md#the-agent-store)).

Path (**Proposed**): `<app data>/ServiceCallers/<sha256(site_url/workspace_id)>.json`.
This is the key `cloud_chat::conversation_path`
(`desktop-core/src/cloud_chat.rs`) already uses. It is derived from the
account alone, so sign-out can delete the file without reading it.

Permissions. The file is written with
`agent_relay_node::platform::fs::write_private`, as the Claude attestation
key is (`Key::load_or_create` in `desktop-core/src/claude_attestation.rs`):
a 0700 folder, a 0600 temporary file, then a rename. A file with group or
other bits set, or not owned by this user, resolves no caller until it is
rewritten. On Windows the mode bits do nothing; the file relies on the
per-user profile's access list, like every other private file in the app
data directory. That is the `cfg(windows)` counterpart.

Each caller record holds (**Proposed**):

| Field | Meaning |
| --- | --- |
| `caller_id`, `name` | Registry id and caller name. |
| `agent_id`, `token` | The publisher identity's immutable Relaycast id and its token. |
| `secrets` | Up to two `{sha256, created_at, retires_at}` entries. |
| `harnesses` | Non-empty list of harness ids. |
| `directory_roots` | Each root as given and as resolved. |
| `owns_only` | Always `true`. |
| `enabled`, `created_at`, `rotated_at`, `expires_at` | Lifecycle. |
| `invocations` | The ownership ledger: invocation ids this caller's calls returned, with the worker's agent id once known. Newest 1000. |

The file also records `site_url`, `workspace_id`, `base_url`,
`relaycast_workspace_id` and `user_id`. It does not record the identity
generation: the core's `identity_sync_generation` lives for one process and
starts again at every launch, so it only guards in-flight calls (below).

- `token` is kept the way the core keeps session tokens (`Store.tokens` in
  the 0600 `injectable-agents.json`). No method returns it and nothing logs
  it.
- A secret is 32 bytes from the system RNG. At rest it is the hex SHA-256
  of its UTF-8 form (`sha2`, already a `desktop-core` dependency). A slow
  password hash adds nothing at that entropy. Digests are compared in
  constant time. The plaintext is never written to the registry.

`owns_only` is enumerated by owner, never inferred from a name:

- `spawn.status`: the invocation id must be in the caller's ledger.
- `spawn.release`: the core checks the worker's recorded requester
  (`SpawnOrigin.requester.agent_id`,
  `crates/relay-node/src/relay/store/spawn.rs`) against the caller's
  `agent_id`, and asks Relaycast when it is missing. Desktop also refuses a
  release whose `expected_agent_id` is not in the ledger.
- The inbox delivers only items from the caller's own workers (section 8).

A registry belongs to one `site_url`, `workspace_id` and `user_id`, the
`CloudAccount` fields `copilot_dispatch::route::owner` reads. A caller ends
with any of these:

- **Sign-out.** `Account::sign_out` (`desktop-core/src/account/sign_out.rs`)
  calls `Account::forget_account_in_core`
  (`desktop-core/src/account/profile.rs`), which already deletes the cloud
  chat through `cloud_chat::forget`. The registry, handoff files and
  inboxes are deleted there too. Each caller's Relaycast agent is then
  released with the workspace key, best effort. **Proposed.**
- **Account change.** A registry written for another `site_url`,
  `workspace_id` or `user_id` resolves nothing and is deleted
  (`account_mismatch`).
- **Workspace change.** `Account::select_workspace` gives a different
  `workspace_id` and so a different path. The old file is never read for
  the new workspace and is deleted on the same path as sign-out.
- **In-flight calls.** `admit` compares the generation it started under, as
  `copilot_attestation.rs` and `attestation_route.rs` do. Work queued
  before a clear is dropped, like `forget_cleared_links`
  (`desktop-core/src/desktop_identity.rs`).
- **Calls in flight across a revoke or sign-out.** A call admitted before
  its caller was revoked or signed out ends without writing to the
  caller's files: no inbox, ledger or folder is recreated. A worker its
  invoke started is released once, exactly (`/v1/agents/release-exact`
  with the worker's name and agent id and the workspace key), on the next
  watcher tick; a worker the invoke did not name yet is found by its exact
  invocation id among the core's spawns for 10 minutes. A failed release is
  retried up to five times. Each call ends with one terminal line
  (`late_worker_released`, `late_worker_release_failed`,
  `late_worker_unknown`, `failed` or `dropped`): in the audit after a
  revoke, in the core log after a sign-out, whose deleted tree the audit
  would otherwise recreate. A permit dropped by a cancelled handler names
  no worker, so nothing is released for it. After a revoke a release still
  pending is also kept in the registry the revoke leaves until its
  terminal line, so a core restarted first still releases the worker once.
  A sign-out deletes the `ServiceCallers` tree, so before it does, every
  release still pending (in memory or in the registry) is written to
  `<app data>/ServiceCallerReleases.json`, outside that tree: schema 1,
  created 0600 through the registry's private write, holding per release
  the invocation id, caller id, worker name and agent id when known, the
  workspace identity and the workspace key the release needs. A core
  started after the sign-out reads it once, signed in or not, and releases
  each worker once with the same exact release, its idempotency key
  `service-caller-late-<invocation_id>`. Each release leaves the file when
  it reaches its terminal line, and the file is deleted when no release is
  left, so a second restart releases nothing.

## 4. Provisioning and rotation

Only the signed-in person provisions, through the app or the probe
([agent-relay-probe.md](agent-relay-probe.md#relay-core)); never a remote
peer, Cloud or a relay message.

The bridge admits only this user's processes. Every management method also
refuses with `management_peer` when:

- `CallContext.peer_pid` is `None`;
- the peer, or an ancestor, is a registered session or spawned agent
  (`Plugins.processes`, the relay `ProcessTable`);
- the call carries a `caller_secret`.

A same-user process outside any session tree can still reach these methods.
A probe-initiated `provision` or `rotate` always stays pending until the
person confirms it in Settings. The app's own call proceeds directly.
Headless approval is a later design; it is not implicit authority.

Who counts as the app. Only the operating system's attestation of the
peer as this app's signed code makes a call the app's (`Host::attest_app_peer`,
answering `App`, `NotApp` or `Unavailable`). No executable path, basename or
process name authorizes anything. In this phase no platform attests a
same-user peer: macOS code-signing attestation from the audit token comes
later, and Linux and Windows have no peer identity a same-user process
cannot forge. So every peer, the app included, is treated as the probe: its
`provision` and `rotate` stay pending, and the app-only changes (`decide`,
an immediate provision or rotation that names a `handoff_path`,
`set_enabled` and `revoke`) are refused with the class
`app_attestation_unavailable` until attestation lands. `list` stays
readable. The honest limit: until then a pending request cannot be
approved from Settings either, and disable and delete are not available
from any peer; revoke by signing out still works.

Preview feature. To prove a service caller end to end before attestation
exists, the cargo feature `service-callers-dev-attest` makes the core
answer `App` for a peer with the core's own uid, and `NotApp` for any
other uid or a peer whose uid cannot be read. Windows stays `Unavailable`.
It is off by default and has no run-time switch. It is used only on a
preview branch, and a build outside the debug profile fails with a
`compile_error!`. A core built with it logs a `DANGEROUS PREVIEW` line when
it starts. It must never be in a release build, a release diff or a
landing diff; `scripts/test-dev-attest-guard.sh` enforces that.

Management methods (**Proposed**, in `desktop-core/src/contract.rs`,
answered by `AccountMethods`):

| Method | Params | Returns |
| --- | --- | --- |
| `spawn_callers.list` | none | `{callers: [CallerSummary]}` |
| `spawn_callers.provision` | `{name, harnesses, directory_roots, handoff_path?, expires_in_days?}` | `{caller, handoff_path?}` |
| `spawn_callers.rotate` | `{caller_id, overlap_seconds?, handoff_path?}` | `{caller, handoff_path?, old_secret_retires_at}` |
| `spawn_callers.set_enabled` | `{caller_id, enabled}` | `{caller}` |
| `spawn_callers.revoke` | `{caller_id}` | `Done` |
| `spawn_callers.decide` | `{caller_id, approved}` | `{caller, handoff_path?}` |

`CallerSummary` is `{caller_id, name, publisher: {id, name}, harnesses,
directory_roots, owns_only, enabled, state, requested_by, requested_at,
created_at, rotated_at, expires_at, secrets_live}`. `state` is
`pending_provision`, `pending_rotation`, `enabled`, `disabled` or `expired`;
`requested_by` is `app` or `probe`. No method returns a secret, a hash or a
token.

Pending requests:

- A probe `provision` returns only `{caller}` with `state:
  pending_provision`; its `caller_id` is the key `decide` takes. It never
  returns, accepts or records a `handoff_path`, secret, token or file
  descriptor, and makes no Relaycast request. Only the pending record is
  written, and it survives a restart.
- A probe `rotate` returns `{caller}` with `state: pending_rotation` and
  `old_secret_retires_at: null`. The current secret keeps working.
- `decide {caller_id, approved: true}` is app only. For a provision it
  registers the publisher, writes `caller.json` at
  `<app data>/ServiceCallers/handoff/<caller_id>.json` (0600) and returns
  `{caller, handoff_path}` to the app. For a rotation it commits the new
  secret, `caller.json` and the cleared pending change as one: if any part
  fails nothing changes, the old secret stays the live one and the rotation
  stays pending.
- `decide {approved: false}` on a provision deletes the pending record and
  leaves no credential, file or registration; on a rotation it clears the
  pending change and the current secret stays.
- `decide` on a caller with nothing pending is a `conflict`, not
  retryable, with the class `not_pending`.

Provision:

1. Check the peer, the account and the workspace.
2. Validate `name` (the `segment` rule; not a session registration name;
   not another caller's), `harnesses` (non-empty `HarnessId` names) and
   `directory_roots` (section 7).
3. Register the publisher identity, a Relaycast agent named `name`, with
   the workspace key. Keep its id and token.
4. Generate the secret, store its hash and write `caller.json`.
5. Return the summary and the handoff path. The secret leaves Desktop only
   in that file.

Rotate writes a new secret and replaces `caller.json` atomically. The old
hash stays valid until `retires_at = now + overlap_seconds`: 600 by default,
86400 at most (**Proposed**). At most two secrets are live; a rotate during
an overlap ends the older one at once. The publisher identity does not
change. `expires_at` (90 days by default, **Proposed**) ends every secret;
calls then get `service_expired` until a rotate.

Revoke deletes the record, ledger, inbox and handoff file at once and
releases the Relaycast agent. Agents the caller started before the revoke
keep running; the person can still release them from the app. A worker an
invoke still in flight starts after it is released once (section 3). `set_enabled false` keeps the
record and refuses with `service_disabled`.

## 5. caller.json

Schema `agent-relay-service-caller/2`. The exact field list, the reader
rules and the refusal texts are in
[service-callers-handoff-v2.md](service-callers-handoff-v2.md); in short:

| Field | Meaning |
| --- | --- |
| `schema` | `"agent-relay-service-caller/2"`. A reader refuses any other value; a `/1` file is refused with a clear error and never upgraded in place. |
| `auth` | `{scheme, challenge, authenticate}`: `"challenge-hmac-sha256/v1"`, `"spawn_callers.challenge"`, `"spawn_callers.authenticate"` ([service-callers-auth.md](service-callers-auth.md) section 7). |
| `caller_id` | Registry id, for support. Sent only as the `challenge` param. |
| `caller_name` | The Relaycast agent name. Never sent as a param; `spawn_callers.authenticate` returns it. |
| `publisher` | `{id, name}` of the publisher identity. |
| `workspace` | `{site_url, base_url, workspace_id, relaycast_workspace_id}`. Desktop checks its own record. |
| `account` | `{user_id}`. |
| `secret` | `rsc_` plus 43 base64url characters. Never sent on the wire; it keys the challenge authenticator ([service-callers-auth.md](service-callers-auth.md) section 3). |
| `created_at`, `rotated_at`, `expires_at` | RFC 3339 times; `rotated_at` is absent until the first rotate. |
| `harnesses` | Harness ids it may spawn. |
| `directory_scope` | Resolved folder roots it may spawn in. |
| `owns_only` | Whether it may release only the agents it started. |
| `socket` | The bridge endpoint: `~/.agentworkforce/desktop/core.sock` (or `AGENT_RELAY_CORE_SOCKET`); on Windows the named pipe `\\.\pipe\agent-relay-<hash>`. |
| `inbox` | The result-inbox route on `socket`: `{transport: "core-bridge", open, next, ack, fail, close, disconnect}` naming the `spawn_callers.inbox.*` and `spawn_callers.disconnect` methods. |
| `desktop_version` | The core version that wrote it. |

It never holds the Relaycast token.

How Flows finds it. Flows is given the file explicitly and nowhere else:
the SDK option `serviceCallerFile`, the CLI flag `--service-caller-file`
or the environment variable `RELAYFLOW_SERVICE_CALLER_FILE`. The value must
be an absolute, private path. Flows does not scan the app data directory
and has no default location. Once a caller file is configured, each of
these is an ambiguous configuration and Flows fails closed with an error:

- the file is missing or invalid;
- `hello.methods` lacks a method of the contract (section 8);
- the file's publisher or workspace differs from `spawn_callers.identity`;
- `RELAY_AGENT_TOKEN` is also set.

Remote is a separately explicit mode. It is never the fallback.

Location and permissions:

- Default `<app data>/ServiceCallers/handoff/<caller_id>.json`, or the
  `handoff_path` an app provisioning call names. A probe call never names
  one (section 4); Desktop picks the default when the person approves.
- `handoff_path` must be absolute. Its folder must exist, be owned by this
  user and be writable by no one else.
- It is created 0600 with `fs::private` and `fs::no_follow`, so a planted
  symbolic link is refused. Rotate replaces it; revoke deletes it. It never
  leaves the computer, and the probe never prints it.

Who holds caller.json. Whoever can read the file is the caller: the secret
in it is the whole proof, and the bridge admits every process of this user.
So the file is exactly as private as this user's account. Desktop does not
and cannot test the Node side: what Flows does with the file once it reads
it (who else it hands the secret to, whether it logs it) is Flows' code and
Flows' tests. User-authored local Flows run in the user's trust domain on
macOS, Windows and Linux (decided 2026-11-08); a Flows proxy process may
be architecture, but it is never the security boundary, and no Desktop
rule depends on which Flows process reads the file. A runtime that
proxies calls for others gives them the caller's authority; the scope,
ownership and capacity limits of sections 6 to 8 still bound what that
authority can do, and revoke or rotate ends it.

Presenting the secret. The secret is never presented. The runtime
authenticates each bridge connection once, with `spawn_callers.challenge`
and `spawn_callers.authenticate`, and later calls on that connection carry
no credential ([service-callers-auth.md](service-callers-auth.md)).

## 6. Authentication on each call

Authentication. A caller authenticates once per bridge connection by
answering a challenge bound to the connection, the caller and the
workspace scope; the result is a binding of `(caller_id, authority
generation)` held in the server's own state for that connection,
rechecked before every side effect and torn down by `connection_closed`.
The connection id is server-only `CallContext` provenance: it is not in
the transcript or on the wire, and the client never supplies or receives
it. The challenge, transcript, state machine, error
classes and removal of the per-request `caller_secret` are specified in
[service-callers-auth.md](service-callers-auth.md), which supersedes the
per-request secret wherever this document still mentions it, and the
60 s lease fallback for a dropped connection (leases are released at
`connection_closed`).

For any call on a bound connection, in order:

1. **Peer.** The bridge serves a connection only when the kernel reports
   this user's uid (`peer.uid == current_uid()` in `server.rs`). Desktop
   repeats the check on the process, as `copilot_dispatch::route::route`
   and `copilot_dispatch::peer::runtime` do: `peer_pid` must be `Some` and
   `ProcessTable::process(pid)` must report
   `uid == agent_relay_node::relay::peer::current_uid()`. `connection` has no
   peer, so for spawn calls this runs in `admit`, before any Relaycast
   request. This check is defence in depth, not a platform gate. On
   Windows the core cannot read the calling process today and the check
   has no `cfg(windows)` counterpart yet, so service callers there report
   themselves unavailable with `peer_check_unavailable`
   (`Class::PeerCheckUnavailable`); the feature is reported, not hidden.
   The policy is one function, `peer::available`, and it covers every
   service-caller method: `challenge` and `authenticate` included, so no
   binding is ever made, the management methods, every bound and
   per-request call (refused before anything else in `methods::call`), and
   `connection`, which refuses a connection bound before. Tests set the
   `peer::UNAVAILABLE` failpoint to run the Windows policy on every
   platform; the `cfg(windows)` unit test checks the real one. Once that
   counterpart exists, the named pipe's owner-only DACL plus the
   per-connection challenge is the Windows boundary
   ([service-callers-auth.md](service-callers-auth.md) sections 1 and 2).
2. **Binding.** The connection holds a binding whose authority
   generation is current
   ([service-callers-auth.md](service-callers-auth.md) section 4). An
   unknown caller and a wrong authenticator answered alike at bind time.
   `connection` resolves the binding from memory only: no file is read
   and nothing is written on the way to a bound caller. It checks the
   live binding against the registry as this process last read or wrote
   it (`registry::cached`). Every registry write goes through
   `registry::save`, which refreshes that copy before the writer returns,
   so a revoked, disabled or rotated caller is refused on its next call.
   A secret past its retirement or a caller past its expiry that no write
   has recorded yet is refused all the same: the binding goes stale as
   the write would make it, and the write is left to the next check that
   holds the files lock (the two-second sweep at the latest). A rotation
   still marked for rollback answers `rotation_rollback_pending` without
   touching the binding.
3. **Live and bound.** The caller is enabled and not expired. The
   registry's account and workspace equal the current `CloudAccount` and the
   core's `Workspace`. The identity generation has not moved.
4. **Scope and ownership.** A bound `connection` returns `harnesses: Some(..)`,
   `directory_roots: Some(..)` and `owns_only: true`. The core checks folder
   and harness on invoke and the requester on release. `admit` checks the
   ledger on status and release.
5. **Record.** `record` adds a successful invoke's invocation id to the
   ledger, and the worker's agent id once a status reports it. The receipt
   is gated on that write: a success whose record fails answers a
   `store_failed`, audited as `store_failed`, and the same request with
   the same `idempotency_key` replays the invocation and recovers its
   receipt. A successful invoke whose identity generation moved while it
   ran is never left unowned: while the account and workspace are
   unchanged its outcome is recorded as usual, audited as
   `generation_moved`, and the caller binds again to reach it; once they
   are another, the worker is released once as a late worker and the
   invoke answers `generation_moved` instead of a receipt.
   The ledger keeps 1,000 invocations. Past that it evicts the oldest
   entry whose result is settled or never came; only when every entry is
   still unsettled does it evict the oldest, and then that invocation's
   inbox slot is dropped with it, since nothing can open or settle it any
   more.

Each call is checked against its connection's binding: the binding is
the only thing that carries over between calls, and it is rechecked before
every side effect.

Refusals carry a static class and nothing else, like
`copilot_dispatch::unavailable` and the `&'static str` classes in
`copilot_dispatch/peer.rs` (`native_peer_unavailable`,
`native_peer_changed`). Each class maps to one `RefusalReason`:

| Internal class (**Proposed**) | `RefusalReason` |
| --- | --- |
| `caller_unknown`, `secret_mismatch` | `service_unknown` |
| `caller_disabled` | `service_disabled` |
| `caller_expired`, `secret_retired` | `service_expired` |
| `account_mismatch`, `workspace_mismatch`, `generation_moved` | `workspace_mismatch` |
| `not_in_ledger`, `requester_other`, `item_not_owned` | `not_owner` |
| `harness_outside`, `directory_outside` | `outside_scope` |
| `peer_unreadable`, `peer_uid_mismatch`, `peer_in_session_tree`, `secret_on_management`, `peer_check_unavailable`, `app_attestation_unavailable` | `management_peer` |

`secret_mismatch` is answered as `caller_unknown`: an unknown name and a
wrong secret read alike to the caller, and only the audit tells them apart.
Classes outside a refusal carry a kind and no reason: `settlement_conflict`,
`iterator_open`, `name_taken` and `not_pending` are `conflict`; `not_found`;
`node_unavailable` and `node_other` are `refused`; `not_signed_in`;
`store_failed`; `rate_limited`; `live_agent_cap` and `reservation_full` are
`busy`; `settlement_invalid` and `invalid_input` are `invalid_params`
(-32602). `app_attestation_unavailable` stands for "unavailable": the core's
`ErrorKind` has no such kind and its `RefusalReason` is closed, so the
nearest is `refused` with `management_peer`.

The class goes to the audit log with the caller id, method and time. No
message, log line or audit entry holds a secret, hash, token or task text.

## 7. Harness and directory scope

Every caller has a non-empty harness list and at least one root. Desktop
never returns `None` for either; `None` leaves only the node's own check.

Harnesses are `HarnessId` names (`codex`, `claude`, `opencode`, `grok`,
`muse`, `devin`). The core compares the request's `harness` exactly.

Roots at provisioning must be absolute, existing folders. Each is stored as
given and as resolved by `std::fs::canonicalize`. A root may not be the
filesystem root, the home folder or a folder that contains it, or the app
data directory, a folder inside it or a folder that contains it
(**Proposed**).

On each spawn:

- The core refuses a relative `directory` or one with a NUL byte.
- `scoped_directory` canonicalises the folder and each root and admits the
  folder only when it `starts_with` a root. `Path::starts_with` compares
  whole components, so `/src` does not admit `/srcx`.
- The resolved folder is what Relaycast receives, so a symbolic link
  swapped after the check changes nothing (relay test
  `a_scoped_spawn_forwards_the_checked_path_after_a_symlink_swap`).
- A folder inside the app data directory is refused even under a root.
- A root that no longer resolves admits nothing. On Windows both sides
  are verbatim (`\\?\`) paths, so the comparison holds.

## 8. Result path

relayflows-node-spawn, the Flows runtime owner, decided the result path.
This is the normative minimum Desktop contract:

> Flows runtime selection needs the result-inbox bridge as the supported local default. A separately scoped publisher credential would leave Flows holding RELAY_AGENT_TOKEN-equivalent authority and preserve the remote transport boundary we are replacing; keep it only as an explicit remote/alternative lane, not automatic local fallback. Minimum Desktop contract for one shared service-caller runtime: return exact publisher {id,name} plus stable base_url/workspace identity; expose cancellable durable inbox consumption with independent per-task iterators over one caller identity; each delivery must include inbox item id, message id, sender {id,name}, and text; expose ack(read+journaled metadata) and fail(nonretryable malformed-result metadata) only for that caller; allow iterator close without disconnecting the shared identity; allow one final runtime disconnect after all tasks drain; preserve redelivery until ack/fail; authenticate every operation with the provisioned caller and never return its Relay token. spawn.invoke/task delivery must make the spawned agent report to that same publisher identity without exposing a token. Flows will compose LocalNodeSpawnPort + bridged inbox/publisher, select it only when the full local contract is present, and fail closed rather than silently falling back.

### Default: the result-inbox bridge

Reporting. A caller's spawns name its publisher identity as requester. A
spawned Claude or Codex DMs its final answer to the requester, and any
spawned agent can report with its report command
([native-spawn.md](native-spawn.md#reporting-back-to-the-requester)). So
every report reaches the publisher identity without any token leaving the
core. The core reads that agent's DMs with the token it holds.

Admission. An item is kept only when its Relaycast sender id is a local
spawn registration whose `SpawnOrigin.requester.agent_id` is the caller's
`agent_id` and whose invocation is in the caller's ledger. Anything else is
dropped and counted.

Methods (**Proposed**). Each runs on a bound connection, takes no
credential and runs section 6 first. Feature detection is settled: the authority is the
`@agent-relay/core-bridge` client's `hello.methods` list, not anything
`spawn_callers.identity` returns. Flows selects the local runtime only when
the complete set is present in `hello.methods`: `spawn.targets`,
`spawn.invoke`, `spawn.status`, `spawn.release`, `spawn_callers.identity`,
`spawn_callers.inbox.open`, `spawn_callers.inbox.next`,
`spawn_callers.inbox.ack`, `spawn_callers.inbox.fail`,
`spawn_callers.inbox.close` and `spawn_callers.disconnect`.

| Method | Params | Returns |
| --- | --- | --- |
| `spawn_callers.identity` | none | `{publisher: {id, name}, node: {id, name}, base_url, workspace_id, relaycast_workspace_id}` |
| `spawn_callers.inbox.open` | `{invocation_id}` | `{iterator_id}` |
| `spawn_callers.inbox.next` | `{iterator_id, timeout_ms}` | `{delivery}` or `{delivery: null}` at timeout |
| `spawn_callers.inbox.ack` | `{item_id, metadata: {"relayflows_result": "journaled"}}` | `{item_id, message_id, action: "ack", state: "acked", settled_at_ms}` |
| `spawn_callers.inbox.fail` | `{item_id, metadata: {"relayflows_result": "rejected"}}` | `{item_id, message_id, action: "fail", state: "failed", settled_at_ms}` |
| `spawn_callers.inbox.close` | `{iterator_id}` | `Done` |
| `spawn_callers.disconnect` | none | `Done` |

- `identity` returns the publisher, this computer's own spawn node, `base_url`
  and workspace identity only. If a `contract` list is kept it is diagnostic
  and never authority.
- `node` is the exact node `spawn.invoke` accepts: `id` is the `node_id`
  this install's enrolment saved (`spawn-node.json`), and `name` is the
  enrolled name (an alternate when another node held this computer's own).
  It is the authenticated, dynamic authority for this core's own node, read
  on each `identity` call and never copied into `caller.json`. A reply
  without `node`, or with a malformed one, fails closed: the runtime admits
  nothing.
- Selection and readiness are distinct. The runtime never selects a node
  from `spawn.targets`, which lists every node in the workspace with no
  local marker; `node` is the selection. Before it admits a run, the
  runtime validates readiness: it must find exactly one entry in
  `spawn.targets` whose `id` and `name` both equal `node`, with
  `live: true`, `handlers_live: true`, `stale: false` and the capability
  `spawn:<harness>` for the run's harness. Zero entries, more than one, an
  entry that is not ready, or an entry matching on only one of the two
  fields fails closed. The 11-method set does not change.
- The runtime passes `node.id` and `node.name` as `spawn.invoke`'s
  `node_id` and `node_name`, and the core and Desktop revalidate them on
  every invoke: the core refuses a `node_name` that is not the enrolled
  name (relay `spawn/control.rs`, "Native spawn starts agents only on this
  computer's own spawn node"), and the Desktop slice refuses a `node_id`
  other than `node.id` the same way. Either mismatch is refused before any
  Relaycast request, with a test for each.
- A delivery is `{item_id, message_id, sender: {id, name}, text,
  invocation_id, sent_at}`. A terminal failure delivery has the same fields
  except that it has no `text` and carries `failure: {code, bytes, limit}`
  (see Capacity). A delivery carries one of `text` and `failure`, never
  both.
- Iterators are per task: each covers one ledger invocation, and many may
  be open on one caller at once. Opening one for an invocation not in the
  ledger is `not_owner`. An iterator delivers only its own invocation's
  results, so parallel iterators never lease each other's. This needs a
  Flows-side source change, a merge gate and not a follow-up: Flows #707's
  `NodeResultInboxClient.subscribe` and `waitForNodeResult` carry no
  invocation id today. The final Flows refactor adds `invocationId`
  (passed `spawn.id`) so the local adapter opens that exact iterator; the
  remote adapter may ignore it.
- `next` waits at most 30000 ms. It is cancellable: closing the iterator,
  or dropping the connection, ends a pending `next` at once with no item
  consumed.
- `ack` means read and journaled; its `metadata` is exactly
  `{"relayflows_result": "journaled"}`. `fail` means a nonretryable
  malformed result; its `metadata` is exactly
  `{"relayflows_result": "rejected"}` and the item is not redelivered. Any
  other metadata is refused, so no task or error text is ever stored in it.
  Either works only on the caller's own items (`item_not_owned` otherwise).
- Durability: items persist across core restarts until acked or failed. An
  item delivered and not settled is delivered again on the next `next` of
  any iterator for that invocation once its 60 s lease ends, including one
  on a later connection of the same caller. Closing an iterator releases
  its delivered, unsettled leases at once. So does the drop of the bridge
  connection that opened it: the core reports it with
  `MethodExtension::connection_closed` (relay #2111), and Desktop closes
  every iterator opened on that `CallContext.connection_id` and releases
  their leases immediately. The 60 s expiry is the crash fallback only,
  for a connection whose close is never reported: an iterator opened
  without a connection id ends after 60 s with no `next`, and its leases
  with it. A lease binds only the
  delivery, never the settlement: any authenticated call of the same
  caller may `ack` or `fail` the item.
- Settlement is idempotent per `(caller_id, item_id)`: the first `ack` or
  `fail` returns a receipt, `{item_id, message_id, action, state:
  "acked"|"failed", settled_at_ms}`, stored in the item's tombstone before
  it is returned. Repeating the same settlement returns the byte-identical
  receipt, on any connection of the same caller and across a restart. The
  other action (an `ack` after a `fail`, or the reverse) or different
  metadata is `settlement_conflict`, not retryable, and changes nothing. A
  settled item keeps its tombstone for exactly 7 days; after that, and for
  an unknown item, settlement is `not_found`. Another caller's item is
  `item_not_owned`. Errors carry no receipt, and no timeout, lease expiry
  or lost connection settles an item.
- Errors on the wire: every error is the core's `RpcError`, code -32000
  (-32602 for invalid params), `message` the class string of section 6 and
  nothing else, and `data: {kind, retryable, reason?}`. The generated
  contract names the classes as `CallerErrorClass`, with `other` for a
  class a client does not know.
- One caller identity serves several bridge connections at a time, each
  with its own per-invocation iterators; the core never ties the identity
  to one connection, so a runtime may keep dedicated connections for long
  `next` polls.
- `identity` and the authority checks of section 6 complete on their own
  call, before any `spawn.invoke`, so Flows can validate identity and
  authority before it admits a run.
- An unsettled delivery is never evicted, by age or by count.
- `close` ends one iterator and leaves the caller and the others alone.
- `disconnect` is the runtime's one final call after all tasks drain; it is
  refused while an iterator is open. Unsettled items stay.

Storage (**Proposed**): `<app data>/ServiceCallers/<key>.results/<caller_id>.json`,
written like the registry.

Capacity is reserved, not trimmed. Each caller is bounded at 1000 records
and 16 MiB. An invocation holds exactly one terminal result slot with a
512 KiB UTF-8 text budget. `admit` reserves the full 512 KiB before it
admits the spawn, so 16 MiB bounds active reservations to 32 per caller;
the 1000-record cap mainly bounds the 7-day settled metadata. When the
reservation cannot be made, the new spawn is refused, before any Relaycast
request. The reservation is released when the invocation's item is
settled. Settled items keep only their id and metadata, which carries the
reserved capacity figures, and expire after exactly 7 days. A full caller
refuses the new spawn (`reservation_full`); it never evicts a live
tombstone to make room. All of it goes with the caller on revoke or
sign-out.

A reservation whose invoke ends without a worker (a failed or cancelled
handler) is released at once. When that write fails, the release is kept
in memory and written at the caller's next inbox read, record or
reservation, so the capacity comes back then; after a restart the
unbound-slot purge reclaims it. An invoke that started a worker names its
invocation, and the slot it is to be bound to, in the registry before its
claim ends; when the bind write fails, the caller's next inbox load
writes it, after a restart as well, so the worker's report is kept once.

Reports after the one terminal item are dropped and audited: a count and a
class, never text.

An oversize report is never truncated and never silently dropped. A report
over 512 KiB leaves a small durable rejected-delivery record in the
reserved slot with the typed code `result_too_large`, `bytes` and `limit`.
It reaches Flows as a terminal failure delivery
(`failure: {code: "result_too_large", bytes, limit}`, no `text`). It stays
unsettled until Flows journals it and calls `fail` with the exact rejected
metadata. The spawned agent is released normally. The same typed terminal
path covers a reserved write that cannot commit, with the code
`result_write_failed` (**Proposed** name). There is no retry and no hang.

### Alternative lane: a publisher credential

The runtime could hold the publisher identity's token, or a second scoped
credential, and read and send on the relay as itself. That means a second
secret with its own rotation, Relaycast-side scoping, and a network path
outside the core's checks. It gives the runtime authority equal to a relay
agent token and keeps the remote transport boundary this contract replaces.

It is an explicit remote or alternative lane only. It is never a local
fallback: when the local contract is incomplete or refuses, or the
configuration is ambiguous (a caller file together with `RELAY_AGENT_TOKEN`,
section 5), Flows fails closed. Desktop provides no method that issues it.

## 9. What never happens

- No token exchange, and no method, file or log returns the Relay token.
- No broad or prefix trust: no wildcard, name prefix or `None` scope.
- No human credential reuse: the Cloud session, workspace key and session
  tokens are never used for a caller. No secret leaves the computer.
- No caller reads, releases or receives results for another caller's or a
  session's spawns.
- No remote provisioning, rotation or revocation, and no silent fallback
  from the inbox to a publisher credential.
- No name-based authority: no executable path, basename or process name
  makes a peer the app, and no response serializes a caller secret, hash
  or token.
- Nothing in relay's public core beyond #2103.

## 10. Acceptance tests

- `a_cold_core_resolves_a_caller_provisioned_before_restart`
- `a_rotated_caller_accepts_the_old_secret_until_the_overlap_ends`
- `sign_out_deletes_the_registry_and_refuses_every_caller`
- `an_owns_only_caller_cannot_status_or_release_another_callers_spawn`
- `a_harness_outside_the_caller_scope_is_refused_before_relaycast`
- `a_directory_escaping_its_root_by_symlink_or_dotdot_is_refused`
- `no_log_audit_or_error_contains_a_caller_secret_or_token`
- `a_peer_whose_uid_is_not_this_user_is_refused`
- `the_result_inbox_delivers_only_the_callers_own_results`
- `an_unsettled_delivery_is_redelivered_until_ack_or_fail`
- `flows_selects_the_local_runtime_only_when_every_contract_method_is_present`
  (checks the complete method set in `hello.methods`)
- `ack_and_fail_metadata_are_exactly_the_two_fixed_values`
- `a_spawn_is_refused_before_relaycast_when_the_512_kib_result_reservation_cannot_be_made`
- `a_failed_release_is_written_at_the_next_reservation`
- `an_invocation_holds_exactly_one_terminal_result_and_later_reports_are_audited`
- `an_oversize_report_becomes_a_typed_result_too_large_terminal_failure_that_flows_must_fail`
- `an_iterator_delivers_only_its_own_invocations_results`
- `a_probe_provision_stays_pending_until_the_person_confirms_in_settings`
- `no_method_ever_returns_the_callers_relay_token`
- `identity_names_the_enrolled_node_and_any_other_node_in_spawn_invoke_is_refused`
  (`node_id` and `node_name` each, against `node`, before any Relaycast
  request)
- `an_identity_reply_without_a_node_or_with_a_malformed_one_admits_nothing`
- `run_admission_needs_exactly_one_ready_target_matching_node_id_and_name`
  (zero, several, not live, handlers not live, stale, missing
  `spawn:<harness>`, or a match on one field only each fail closed)
- `one_caller_identity_serves_concurrent_bridge_connections_with_per_invocation_iterators`
- `an_iterator_close_releases_its_delivered_unsettled_lease_at_once_and_an_unidentified_connection_falls_back_to_the_lease`
- `a_connection_close_releases_its_iterators_leases_at_once`
- `a_connection_close_ends_a_pending_next_on_its_iterator`
- `another_connections_close_does_not_release_this_connections_lease`
- `an_identified_connections_idle_iterator_stays_open_until_its_connection_closes`
- `a_connection_close_without_iterators_changes_nothing`
- `identity_and_authority_validation_complete_before_flows_admits_a_run`
- `an_expired_lease_is_redelivered_to_a_later_same_caller_connection`
- `no_response_serializes_a_caller_secret_or_token`
- `a_tombstone_is_retained_under_capacity_pressure`
- `another_callers_item_is_refused_not_owner`
- `a_probe_provision_discloses_no_path_secret_or_fd`
- `a_pending_request_survives_restart`
- `approving_a_rotation_commits_atomically_with_the_old_secret_live_until_commit`
- `rejecting_leaves_no_credential_or_registration_artifacts`
- `a_forged_app_peer_cannot_decide`
- `app_only_mutations_report_unavailable_without_attestation`
- `decide_on_a_non_pending_caller_is_kind_conflict_reason_not_pending`

Relay's tests for the core half: `spawn/control/tests/callers.rs`.

Wiring, when code follows:

- Move the `agent-relay-node` pin in `desktop-core/Cargo.toml` and
  `gtk/Cargo.toml` (`929454d1` today) to the #2103 merge, refresh both
  lockfiles and run `scripts/generate-contract.sh`.
- Add `context` to `AccountMethods::call`
  (`desktop-core/src/account/plugin.rs`) and `SessionLocations::call`
  (`desktop-core/src/session_locations/extension.rs`).
- Set `plugins.spawn_callers` in `with_account` (`desktop-core/src/account.rs`).
  macOS and GTK get the same Settings rows and wording.

## 11. Decisions recorded

Settled on 2026-11-08 by relayflows-node-spawn, the Flows runtime owner:

1. Result path: the result-inbox bridge (section 8). The 60 s redelivery
   lease is settled. An unsettled delivery is never evicted; capacity is
   reserved in `admit` and a spawn is refused when it cannot be. `ack` and
   `fail` metadata are exactly the two fixed values. Capacity is settled:
   1000 records and 16 MiB per caller, and one terminal result slot of
   512 KiB per invocation, reserved in full (32 active reservations per
   caller). Later reports are dropped and audited. An oversize report is a
   typed `result_too_large` terminal failure delivery, never truncated.
2. Feature detection: the `hello.methods` list of the
   `@agent-relay/core-bridge` client is the authority, and Flows needs the
   complete method set of section 8. The caller file is passed explicitly
   only (`serviceCallerFile`, `--service-caller-file`,
   `RELAYFLOW_SERVICE_CALLER_FILE`), as an absolute private path, with no
   scanning or default discovery. An ambiguous configuration fails closed
   (section 5).
3. Provisioning confirmation: a probe-initiated `provision` or `rotate`
   always stays pending until the person confirms in Settings. Headless
   approval is a later design. The pending surface is
   `spawn_callers.decide {caller_id, approved}`, returning
   `{caller, handoff_path?}`; `provision` and `rotate` return
   `handoff_path` only to the app. `decide` on a caller with nothing
   pending is the `conflict` class `not_pending`. App-only changes need the
   operating system's attestation of the app peer and are refused with
   `app_attestation_unavailable` until it exists (section 4).
4. Granularity: one caller per Flows project or worker profile, shared by
   its parallel tasks; distinct project scopes get distinct callers
   (section 2).

Still open, as a Flows merge gate (a source change, not a follow-up):

- Flows #707's `NodeResultInboxClient.subscribe` and `waitForNodeResult`
  have no invocation id. The final Flows refactor adds `invocationId`
  (passed `spawn.id`) so the local adapter opens that exact iterator
  (section 8).
