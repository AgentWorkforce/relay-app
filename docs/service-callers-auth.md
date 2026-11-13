# Service caller authentication: connection-bound challenge (**Proposed**)

This proposal replaces the per-request `caller_secret` of
[service-callers.md](service-callers.md) (relay #2103, Desktop phase 1).
A Flows runtime proves once per bridge connection that it holds a
caller's `caller.json` secret. It does so by answering a fresh challenge
that Desktop issues on that connection. The result is a binding of
`(caller_id, authority generation)` that the server keeps in that
connection's own state. No later call on the connection carries a
credential, and no static secret crosses the bridge.

Status: phase 2 is implemented on the Desktop preview branch, with the
contract authority's ruling (relayflows-node-spawn, 2026-11-08) applied;
section 9 records it. `spawn_callers.challenge` and
`spawn_callers.authenticate` bind a connection as described here, and
`spawn_callers.inbox.ack` / `spawn_callers.inbox.fail` answer with the
final `InboxSettled` receipt. The items in the
[removal list](#10-removal-list) are deleted before any Desktop release.

Ordering. This proposal gates the publication of
`@agent-relay/core-bridge` 17.2.2 and any Desktop user release. It is
**not** the gate for the first preview end-to-end run: that run may use
the current per-request `caller_secret` after relay #2111, under
quarantine (section 9, ordering note G).

## Requirements

These are the agreed hard gate, as amended by the contract ruling of
section 9 (items B and D and the receipt key case are applied here in
place). The rest of this document must satisfy each one.

1. **No static `caller_secret` crosses the bridge, ever.** The current
   per-request `caller_secret` wire (relay #2103 / phase 1) is interim
   scaffolding and must not be normalised; this proposal replaces it. Every
   public Rust/Swift/TS parameter, doc line and log that carries
   `caller_secret` is listed in the [removal list](#10-removal-list) with
   its file path, so phase 2 can delete it.
2. **The server issues the challenge.** The server is the Desktop
   `spawn_callers` extension running inside the core. It issues a fresh,
   high-entropy (at least 32 bytes from a CSPRNG), single-use, short-lived
   challenge. The challenge is bound to:
   - the protocol version;
   - the issuing connection. The server keeps the challenge in that
     connection's own slot, keyed internally by `CallContext.connection_id`.
     The connection id is server-only provenance: it is not in the
     transcript, in any request or response, or in the fixture, and the
     client never supplies, receives or relies on it;
   - the caller identity (`caller_id`);
   - the workspace scope: site origin, workspace id and user id, exactly as
     the registry scopes callers today.
3. **The client returns a domain-separated authenticator.** The client is
   the Flows runtime holding `caller.json`. It computes the authenticator
   over a canonical, versioned byte transcript. This document specifies:
   - the exact byte layout: length-prefixed fields, fixed order, version
     byte first, and the domain-separation string
     `agent-relay/service-caller/v1`;
   - the MAC construction: symmetric HMAC-SHA256 keyed from the
     `caller.json` secret, with the choice justified and no asymmetric
     requirement (Ed25519 was considered and not chosen);
   - the wire encodings: base64url without padding for binary, and
     lowercase hex for ids where the code already uses hex;
   - verification: the server verifies in constant time and consumes the
     challenge atomically. Each challenge allows one verify attempt, and
     success or failure consumes it.
4. **The binding result is `(connection, caller_id, authority
   generation)`, held by the server.** The connection half is the server's
   per-connection slot; it never appears on the wire. Later calls on that
   connection carry no credential.
   - The authority generation is the registry's per-caller counter. It
     advances on rotate, revoke, disable and delete, and on account change.
   - It is rechecked before every side effect, not only at bind time. Side
     effects include lease hand-out, settlement and provision.
5. **`connection_closed` is the teardown barrier.** When the core reports
   the connection closed:
   - the binding is dropped, and a closed marker takes its place until the
     connection's in-flight calls have drained;
   - open iterators are closed;
   - leases are released immediately. The 60 s expiry remains the crash
     fallback only, for a connection whose close is never reported, such
     as a call with no `connection_id`. Desktop does this part already,
     ahead of phase 2: each iterator records the `connection_id` it was
     opened on, and `connection_closed` closes those iterators and
     releases their leases at once, as `inbox.close` does;
   - no call on that connection can succeed afterwards, even one that was
     already in flight. A call that reaches the side-effect step after
     teardown is refused with a typed error, and no external call can fall
     through to the NotExternal (session) path
     ([ordering](#teardown-against-in-flight-calls)).
6. **Every external call resolves to one of three states:**
   - **NotExternal.** The call came from inside the core or a session peer,
     and no binding was attempted. It takes the existing session path,
     unchanged.
   - **Bound with the current generation.** It is a service caller.
   - **Known-but-unbound, stale generation, revoked or disabled.** It gets a
     typed refusal.

   A call never falls back to a session identity with the same name.
7. **Typed errors are uniform and allow no enumeration.** An unknown
   caller, a wrong authenticator, an expired challenge and a consumed
   challenge are indistinguishable on the wire: same class, same timing
   class.
   - The error wire stays `RpcError {code: -32000, message: <static class
     string>, data: {kind, retryable, reason?}}` (Amendment D).
   - The auth classes are added to the existing table.
8. **Rates and bounds:**
   - at most 1 outstanding challenge per connection, at most 8 outstanding
     per `caller_id` and at most 256 outstanding in total;
   - a challenge TTL of 30 s;
   - at most 3 authenticate attempts per connection. An attempt counts
     when its challenge is consumed by a failed `authenticate`, or expires,
     without a binding. The third issued challenge can still bind; only
     after three such attempts is the connection refused further auth;
   - a size bound on every field.
9. **Legacy secret-bearing external calls fail closed with a typed
   class.** There is no Desktop-only mock method. Where each value may
   appear:
   - the `caller.json` secret, the key `K` and the stored digests never
     appear in any request, response, error, log, audit record or
     delivery;
   - `authenticator` appears only in the `spawn_callers.authenticate`
     request;
   - `nonce` appears only in the `spawn_callers.challenge` response;
   - `challenge_id` appears only in the `spawn_callers.challenge` response
     and the `spawn_callers.authenticate` request;
   - none of these appears in any other response, or in any error, log,
     audit record or delivery.
   - Audit records (`audit.rs`) log only `caller_id`, the server-internal
     `connection_id`, generation and class. Audit may record the
     connection id; the wire never carries it.
10. **Settlement returns a receipt.** The ack/fail receipt returned to the
    caller is `{item_id, message_id, action: "ack"|"fail", state:
    "acked"|"failed", settled_at_ms}`, with snake_case wire keys
    (generated Swift properties may be camelCase). It is consistent with
    Amendment A:
    - settlement is idempotent per `(caller_id, item_id)`: an identical
      repeat returns the byte-identical original receipt, on the same
      connection, on a later connection of the same caller, and across a
      core restart;
    - the receipt is stored in the item's 7-day tombstone, which is
      durable before the first receipt is returned;
    - any authenticated call of the same caller may settle;
    - refusals carry no receipt and change nothing: the opposite action or
      different metadata is `settlement_conflict` (not retryable), another
      caller's item is `item_not_owned`, and an unknown item or one past
      its tombstone is `not_found`;
    - nothing synthesizes a receipt: no timeout, lease expiry, empty
      `next` or transport loss settles an item.

## 1. Purpose and threat model

### Trust model

Trust follows the project's decision of 2026-11-08, which replaces the
trusted-proxy model of Amendment C:
- User-authored local Flows run in the user's trust domain, on macOS,
  Windows and Linux alike. Whoever can read `caller.json` is the caller.
  Desktop does not, and cannot, test what Flows does with the file.
- The credential is scoped (harnesses, directory roots, `owns_only`,
  workspace), revocable (rotate, revoke, disable and account change
  advance the authority generation and end every binding) and
  connection-bound (a binding never leaves the connection that made it).
- Proxy, sandbox or same-UID process isolation is not a gate. Flows may
  use a proxy process as architecture, but it is never the security
  boundary, and no rule of this document depends on which Flows process
  reads `caller.json` or opens a bridge connection.
- The boundary, on every platform, is the operating system's same-user
  admission to the bridge plus possession of the secret, proven per
  connection by the challenge:
  - macOS and Linux: the Unix socket admits only processes of this user
    (`peer.uid == current_uid()` in relay `server.rs`);
  - Windows: the named pipe `\\.\pipe\agent-relay-<hash>` is created with
    a DACL that grants only the current user (`relay::transport`;
    [relay-core.md](relay-core.md)), so the OS refuses other users.
  - A PID, uid or token check on the peer is defence in depth. On macOS
    and Linux the existing process check runs as today.
  - Windows at this head: the core cannot read the calling process and
    the peer check has no `cfg(windows)` counterpart yet, so service
    callers report themselves unavailable there. Every service-caller
    method, `challenge` and `authenticate` included, answers
    `peer_check_unavailable` (reason `management_peer`), and
    `SpawnCallers::connection` refuses with it. The feature is reported,
    not hidden. The policy is one function, `peer::available`; tests set
    its `peer::UNAVAILABLE` failpoint to run it on every platform.
  - The ruling of section 9 (Windows supported, the pipe DACL plus the
    challenge as the boundary, any PID or token check defence in depth
    and never a platform gate) is the target once that counterpart
    exists. It is not what this head does.
- The core assigns `connection_id` per accepted bridge connection.
  - The first id is 1, and ids are not reused within a core process.
  - It is server-only `CallContext` provenance. The client never
    supplies, receives or relies on it. Desktop reads it only from
    `CallContext.connection_id`, keys its per-connection state by it, and
    may write it to audit.
  - Core-internal calls carry `CallContext::default()`, where
    `connection_id` is `None`.
- An earlier review rejected connection-bound authentication only because
  no connection identity existed at relay 046ff334. Relay #2111 adds
  `CallContext.connection_id` and `MethodExtension::connection_closed`.
- The scoped, revocable, connection-bound credential is the recorded
  direction (2026-11-08). There is no sandbox gate.

### Threats covered

| Threat | Countermeasure |
| --- | --- |
| Secret capture on the wire (a socket tap, a logged request, a crash dump of a bridge client) | The secret never crosses the bridge. A captured authenticator is a MAC over one consumed nonce, held in one connection's slot, so it is useless on any other connection or challenge. |
| Replay | Each challenge is single-use. It is removed from the table before verification, so success and failure both consume it, and it expires 30 s after issue. Challenges and bindings live only in memory and do not survive a core restart. |
| Connection substitution (an authenticator or challenge made on connection A presented on connection B) | The server keeps each challenge only in the slot of the connection that requested it, and `authenticate` looks only in the calling connection's slot. A's `challenge_id` is never in B's slot, so B gets `auth_failed`. The connection is bound by this server-side slot, not by the transcript: `challenge_id` and the fresh nonce, with caller and scope, make the public transcript unique to that one issue. |
| Same-name impersonation (a session or agent named like a caller) | Resolution is by connection state, never by name (section 2, [tri-state](#tri-state-resolution)). The session path reads its requester from the process table. The bound path reads the caller from the binding. Neither consults the other. |
| Stale authority after rotate, revoke, disable or account change | The per-caller `authority_generation` advances under the live lock. That tears down every binding of the caller at once. Every side effect then rechecks the generation under the same lock. |
| PID reuse | No PID is part of the binding. Where the peer is readable, its PID and uid are checked once, at challenge time, as defence in depth. A reused PID or a reconnect arrives on a new connection with no binding. |
| Enumeration of caller ids | `challenge` answers any well-formed `caller_id`, known or not. `authenticate` fails uniformly (`auth_failed`) in one timing class. |
| Online guessing | There are 3 attempts per connection, and each challenge allows exactly one. A brute force has to open new connections, and each one is audited. The HMAC key has 256 bits of entropy. |

Out of scope:
- a process of this user that can read `caller.json`, because it is the
  caller and is inside the user's trust domain;
- code running inside a user-authored Flows process, for the same reason;
- root or an administrator.

Revoke or rotate ends what any of these hold.

## 2. Method surface

`hello.methods` remains the only authority for method discovery. A
runtime selects the local service-caller lane only when it finds both new
methods in `hello.methods`, together with the 11-method set of
[service-callers.md section 8](service-callers.md#8-result-path). That
makes 13 required methods. The lane is offered on macOS, Windows and
Linux.

### `spawn_callers.challenge`

Params (`CallerChallengeParams`):

```json
{ "caller_id": "sc_0123456789abcdef" }
```

| Field | Type | Bound |
| --- | --- | --- |
| `caller_id` | string | Exactly 19 bytes: `sc_` followed by 16 lowercase hex characters (`manage.rs` `random_hex(8)`). Anything else is `invalid_input` (-32602). |

Result (`CallerChallenge`):

```json
{
  "version": 1,
  "challenge_id": "ch_00112233445566778899aabbccddeeff",
  "nonce": "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8",
  "issued_at_ms": 1767225600000,
  "expires_at_ms": 1767225630000
}
```

| Field | Type | Bound |
| --- | --- | --- |
| `version` | integer | Exactly `1`. It is the transcript version byte. |
| `challenge_id` | string | Exactly 35 bytes: `ch_` followed by 32 lowercase hex characters (16 CSPRNG bytes). |
| `nonce` | string | Exactly 43 characters of base64url without padding: 32 CSPRNG bytes (`getrandom`). |
| `issued_at_ms` | integer | Unix ms, Desktop clock, from 0 to 2^53 - 1. |
| `expires_at_ms` | integer | `issued_at_ms + 30000`. |

The result carries no connection id. The client builds the transcript
from these fields and its own `caller.json`.

Server rules:
- It answers every well-formed `caller_id`, whether or not a record exists,
  is enabled or has expired.
- It records `{challenge_id, nonce, caller_id, scope, issued_at_ms,
  expires_at_ms}` in the calling connection's single challenge slot. The
  scope is the current `Binding`: `site_url`, `workspace_id`, `user_id`
  and `key`.
- It runs the peer check once, as defence in depth (`peer::caller`).
  On macOS and Linux this is the existing check: `peer_pid` is `Some` and
  the process has this user's uid, else `peer_unreadable` or
  `peer_uid_mismatch` (reason `management_peer`). On Windows the
  `cfg(windows)` counterpart answers `peer_check_unavailable` until a
  Windows peer check exists (section 1); no challenge is issued, so no
  connection is ever bound there.
- It refuses with:
  - `binding_unavailable` when `connection_id` is `None`;
  - `already_bound` on a bound connection;
  - `challenge_outstanding` while the slot holds an unexpired challenge;
  - `auth_locked` once three attempts on the connection have ended
    without a binding (section 4). The challenge that would be the
    fourth is refused; the third is issued and can bind;
  - `challenge_capacity` when the per-`caller_id` (8) or global (256)
    limit is reached;
  - `not_signed_in` without a current `Binding`.
- Issuing a challenge marks the connection **external** for the rest of its
  life (section 4).

### `spawn_callers.authenticate`

Params (`CallerAuthenticateParams`):

```json
{
  "challenge_id": "ch_00112233445566778899aabbccddeeff",
  "authenticator": "FZa8qpzP0ygcFxQ6K7xFTlx6zo8aorEW_JipkqHZaBA"
}
```

| Field | Type | Bound |
| --- | --- | --- |
| `challenge_id` | string | Exactly 35 bytes, of the form `ch_` followed by 32 lowercase hex characters. |
| `authenticator` | string | Exactly 43 characters of base64url without padding, decoding to 32 bytes. |

Any other shape is `invalid_input` (-32602), and the slot's challenge is
still consumed (it counts as a failed attempt). Unknown fields are
refused. A body carrying `caller_secret` or `caller_name` is consumed and
counted the same way, and then answered `legacy_credential_refused`
(section 2) instead of `invalid_input`.

Result (`CallerBound`):

```json
{
  "caller_id": "sc_0123456789abcdef",
  "caller_name": "flows-runner",
  "authority_generation": 7
}
```

| Field | Type | Bound |
| --- | --- | --- |
| `caller_id` | string | 19 bytes, as above. |
| `caller_name` | string | 1 to 200 characters from `A-Z a-z 0-9 _ -` (`manage::segment`). |
| `authority_generation` | integer | 1 to 2^53 - 1. |

Server steps, in order. Every step runs under the live lock unless
marked otherwise.

1. **Take.** The server removes the calling connection's slot. If the slot
   is empty, or its `challenge_id` differs, a dummy record is used. From
   this point the challenge is consumed, whatever happens next. A real
   challenge taken here counts as one attempt unless step 6 binds.
2. **Compute (outside the lock).** The server reads the registry record
   for the slot's `caller_id`. It builds the transcript (section 3) from
   its own values. It then always computes two HMAC-SHA256 values: one
   for each of the record's live `SecretDigest` entries (at most two
   during a rotation overlap), padded with a fixed dummy key when there are
   fewer than two or no record. When the slot was empty, the server uses a
   dummy transcript of the same length.
3. **Compare.** It compares both MACs with the authenticator in constant
   time (the existing `registry::same` pattern) and folds the results with
   bitwise operations, with no early exit.
4. **Decide.** The checks run after all MAC work. Any of these is
   `auth_failed`:
   - an absent or mismatched challenge;
   - `now >= expires_at_ms`;
   - no record;
   - no MAC match;
   - a match only against a digest whose `retires_at_ms <= now`.

   The audit keeps the real class: `caller_unknown`, `secret_mismatch`,
   `challenge_expired`, `challenge_consumed` or `secret_retired`.
5. **Live.** Only after the MAC has verified, a record that is disabled or
   expired is refused as `caller_disabled` or `caller_expired`. The client
   has proven possession of the secret at this point, so this reveals
   nothing to a stranger.
6. **Bind.** Under the live lock the server checks three things: the
   connection's entry is still open (not the closed marker), the record's
   `authority_generation` equals the value read in step 2, and the
   `Binding.key` equals the slot's scope key. It then stores the binding
   `Bound {binding_id, caller_id, caller_name, authority_generation,
   identity_generation, binding_key}` in that connection's entry.
   - `binding_id` is `bd_` followed by 32 hex characters (16 CSPRNG
     bytes). It is internal and never on the wire.
   - If the connection closed in the meantime, the call is refused with
     `connection_closed`.
   - If the generation moved, the call is refused with `auth_failed`.

### Existing methods

| Method | Allowed on |
| --- | --- |
| `spawn_callers.list` | NotExternal-only |
| `spawn_callers.provision` | NotExternal-only |
| `spawn_callers.rotate` | NotExternal-only |
| `spawn_callers.set_enabled` | NotExternal-only |
| `spawn_callers.revoke` | NotExternal-only |
| `spawn_callers.decide` | NotExternal-only |
| `spawn_callers.identity` | binding-required |
| `spawn_callers.inbox.open` | binding-required |
| `spawn_callers.inbox.next` | binding-required |
| `spawn_callers.inbox.ack` | binding-required |
| `spawn_callers.inbox.fail` | binding-required |
| `spawn_callers.inbox.close` | binding-required |
| `spawn_callers.disconnect` | binding-required |
| `spawn.invoke`, `spawn.status`, `spawn.release` (relay) | both |
| `spawn.targets` (relay) | both, unchanged (read-only) |
| `spawn_callers.challenge`, `spawn_callers.authenticate` (new) | external, unbound connection only |

NotExternal-only methods:
- They keep every existing `management_peer` check.
- They also refuse with `management_peer` on any connection marked
  external (challenged, bound, stale or locked).
- They recheck this under the live lock at their side-effect step. A
  binding therefore never authorizes management, and provision, rotate,
  set_enabled, revoke and decide cannot run on a caller connection.

Binding-required methods:
- They take no `caller_name` and no `caller_secret`.
- `identity` and `disconnect` take no params (`NoParams`).
- `inbox.open` takes `{invocation_id}`, `inbox.close` takes
  `{iterator_id}`, `inbox.next` takes `{iterator_id, timeout_ms}`, and
  `inbox.ack` and `inbox.fail` take `{item_id, metadata}`.
- `disconnect` moves the binding to `stale` (section 4). The
  connection stays open but can neither call nor authenticate again.

`spawn.*` methods:
- On a NotExternal call they take the existing session path, byte for byte.
- On a bound connection they act as the bound caller (relay change R1 in
  the removal list).

### Tri-state resolution

Every call is resolved from `CallContext` and the connection's auth
state, before any params are read. The result is captured for the whole
call and never re-resolved. A params value that is not an object is
answered `invalid_params` before the binding decision, except on
`authenticate`, which consumes the challenge first; the closed-connection
refusal still comes first:

| Condition | State | Outcome |
| --- | --- | --- |
| `connection_id` is `None` (core-internal; the id is server-only provenance) | NotExternal | Session or internal path, unchanged. A binding-required method is refused `caller_unbound`. |
| Connection never challenged | NotExternal | `spawn.*` and management take the existing session path. A binding-required method is refused `caller_unbound`. |
| Connection bound, generation and scope current | Bound | Service caller. |
| Connection challenged but not bound, or locked | Known-but-unbound | `caller_unbound`, or `auth_locked`. |
| Connection bound, generation or scope moved | Stale | `binding_stale`. |
| Bound caller now disabled, expired or revoked | Stale | `binding_stale`. The advance tore the binding down (section 4). |
| Connection closed (the `Closed` marker; an in-flight or late call) | Torn down | `connection_closed`. Never NotExternal (section 4). |

No row reads a name. A session whose name equals a caller's name never
becomes that caller, and a bound caller never becomes a session.

Legacy credentials:
- A request whose raw params object contains the key `caller_secret` or
  `caller_name` is refused `legacy_credential_refused`. This happens before
  deserialisation and before any other check, in every state, on every
  method above, with one exception: `spawn_callers.authenticate` first
  takes and counts the connection's outstanding challenge (step 1 of
  section 4), so a refused attempt consumes it like any other shape and
  the same body retried without the key is `auth_failed`.
- The core applies the same check to `spawn.*` (R1).
- There is no Desktop-only mock method. The only compatibility path is
  the preview-only `service-callers-dev-attest` build, which still accepts
  the phase-1 name and secret on a connection that never challenged. The
  default build refuses them as above, and the path is removed before any
  Desktop release.

## 3. The canonical transcript

### Key

The key is `K = SHA-256(UTF-8 bytes of the caller.json "secret" string)`:
32 bytes.
- The secret is a uniform 256-bit capability (`rsc_` plus 43 base64url
  characters of 32 CSPRNG bytes), so one SHA-256 is an adequate key
  derivation; no password hashing is needed.
- The registry already stores `SecretDigest.sha256`, which is the
  lowercase hexadecimal encoding of `K`: 64 characters `0-9a-f`.
  Verification hex-decodes that string to the same 32 bytes and keys the
  HMAC with those 32 bytes. The 64 ASCII characters are never used as the
  key. Desktop therefore verifies without the plaintext secret, which it
  does not keep.
- The stored digest becomes key-equivalent. That adds no exposure: the
  Desktop registry is already a trusted secret store, and the same 0600
  registry file holds the caller's Relaycast token, which is a stronger
  credential.

### MAC

The MAC is `authenticator = HMAC-SHA256(K, transcript)`. The wire carries
it in base64url without padding. Verification is constant-time and the
challenge is consumed atomically (section 4); the nonce comes from a
CSPRNG.

Why symmetric HMAC-SHA256 and not an Ed25519 signature:
- It is symmetric. Both sides already hold the key material, so there is
  no new key type, no key generation step and no asymmetric requirement.
- `sha2` and `hmac` are standard and constant-time to verify. Node's
  `crypto.createHmac` is its exact counterpart.
- It is a PRF. A MAC over a fresh server nonce proves possession without
  revealing anything reusable.
- An Ed25519 signature would let Desktop hold only a public key. The
  registry already holds the Relaycast token, though, so it would not
  reduce exposure. The contract authority chose HMAC (section 9).

### Byte layout

The layout is a version byte followed by fields in a fixed order.
- Every field is length-prefixed with a big-endian u16 byte count, the
  fixed-width integers included.
- Strings are their exact UTF-8 bytes, with no normalisation.
- The server builds every value from its own state. The values are never
  taken from the request.
- The connection is not a transcript field. The server binds the
  challenge to its connection internally, by keeping it in that
  connection's slot; `challenge_id`, the nonce, the caller and the scope
  make the public transcript.

| # | Field | Prefix | Value | Bound |
| --- | --- | --- | --- | --- |
| 0 | version | none | `0x01` | 1 byte |
| 1 | domain | u16 | `agent-relay/service-caller/v1` (ASCII) | 29 bytes |
| 2 | challenge_id | u16 | `ch_` followed by 32 hex characters, ASCII | 35 bytes |
| 3 | nonce | u16 (`0x0020`) | raw nonce bytes | 32 bytes |
| 4 | caller_id | u16 | `sc_` followed by 16 hex characters, ASCII | 19 bytes |
| 5 | site_url | u16 | `Binding.site_url`, which is `caller.json` `workspace.site_url` | 1 to 2048 bytes |
| 6 | workspace_id | u16 | `Binding.workspace_id`, which is `caller.json` `workspace.workspace_id` | 1 to 256 bytes |
| 7 | user_id | u16 | `Binding.user_id`, which is `caller.json` `account.user_id` | 1 to 256 bytes |
| 8 | issued_at_ms | u16 (`0x0008`) | u64 big-endian | 8 bytes |
| 9 | expires_at_ms | u16 (`0x0008`) | u64 big-endian | 8 bytes |

Notes:
- The client takes fields 5 to 7 from `caller.json`. Desktop writes those
  fields from the same `Binding`, so they match byte for byte. A
  difference means a different account or workspace, and it fails as
  `auth_failed`.
- A server value outside its bound is `not_signed_in`, and no challenge
  is issued.

### Worked example

These are fixed inputs, for cross-language fixtures only:
- secret `rsc_` followed by the base64url of 32 zero bytes;
- `challenge_id` `ch_00112233445566778899aabbccddeeff`;
- nonce bytes `00 01 … 1f`;
- `caller_id` `sc_0123456789abcdef`;
- `site_url` `site_url_fixture`;
- `workspace_id` `ws_fixture`;
- `user_id` `user_fixture`;
- `issued_at_ms` 1767225600000;
- `expires_at_ms` 1767225630000.

The exact transcript bytes, field by field (hex; prefix, then value):

| # | Field | Prefix | Value bytes | Total |
| --- | --- | --- | --- | --- |
| 0 | version | | `01` | 1 |
| 1 | domain | `001d` | `6167656e742d72656c61792f736572766963652d63616c6c65722f7631` | 31 |
| 2 | challenge_id | `0023` | `63685f3030313132323333343435353636373738383939616162626363646465656666` | 37 |
| 3 | nonce | `0020` | `000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f` | 34 |
| 4 | caller_id | `0013` | `73635f30313233343536373839616263646566` | 21 |
| 5 | site_url | `0010` | `736974655f75726c5f66697874757265` | 18 |
| 6 | workspace_id | `000a` | `77735f66697874757265` | 12 |
| 7 | user_id | `000c` | `757365725f66697874757265` | 14 |
| 8 | issued_at_ms | `0008` | `0000019b76daa800` | 10 |
| 9 | expires_at_ms | `0008` | `0000019b76db1d30` | 10 |

Total: 188 bytes.

Command:

```sh
python3 -I -c 'import hashlib,hmac,base64,struct as s;L=lambda b:s.pack(">H",len(b))+b;Q=lambda n:L(s.pack(">Q",n));B=lambda b:base64.urlsafe_b64encode(b).rstrip(b"=").decode();k="rsc_"+B(bytes(32));K=hashlib.sha256(k.encode()).digest();t=b"\x01"+L(b"agent-relay/service-caller/v1")+L(b"ch_00112233445566778899aabbccddeeff")+L(bytes(range(32)))+L(b"sc_0123456789abcdef")+L(b"site_url_fixture")+L(b"ws_fixture")+L(b"user_fixture")+Q(1767225600000)+Q(1767225630000);m=hmac.new(K,t,hashlib.sha256).digest();print("secret",k);print("key_hex",K.hex());print("nonce_b64url",B(bytes(range(32))));print("transcript_len",len(t));print("transcript_hex",t.hex());print("authenticator_hex",m.hex());print("authenticator_b64url",B(m))'
```

Printed output:

```text
secret rsc_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA
key_hex db5641ac1182faf857d441d9a339e9f35fcb4d8eac3c88b0ddc26999ab3f1384
nonce_b64url AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8
transcript_len 188
transcript_hex 01001d6167656e742d72656c61792f736572766963652d63616c6c65722f7631002363685f30303131323233333434353536363737383839396161626263636464656566660020000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f001373635f303132333435363738396162636465660010736974655f75726c5f66697874757265000a77735f66697874757265000c757365725f6669787475726500080000019b76daa80000080000019b76db1d30
authenticator_hex 1596bcaa9ccfd3281c17143a2bbc454e5c7ace8f1aa2b116fc98a992a1d96810
authenticator_b64url FZa8qpzP0ygcFxQ6K7xFTlx6zo8aorEW_JipkqHZaBA
```

`key_hex` is exactly the `SecretDigest.sha256` string the registry would
store for this secret; hex-decoding it gives the 32 key bytes.

### Substitution vectors

These use the same key. Each row changes one field and leaves the rest as
above. Version `0x02`, domain `…/v2`, the last hex digit of
`challenge_id` changed to `0`, nonce bytes `01 … 20`, the last character
of `caller_id` changed to `e`, the last character of `site_url`,
`workspace_id` and `user_id` changed to `f`, and each time increased by
1 ms.

```sh
python3 -I -c 'import hashlib,hmac,base64,struct as s;L=lambda b:s.pack(">H",len(b))+b;Q=lambda n:L(s.pack(">Q",n));B=lambda b:base64.urlsafe_b64encode(b).rstrip(b"=").decode();K=hashlib.sha256(("rsc_"+B(bytes(32))).encode()).digest();f=dict(version=1,domain=b"agent-relay/service-caller/v1",challenge_id=b"ch_00112233445566778899aabbccddeeff",nonce=bytes(range(32)),caller_id=b"sc_0123456789abcdef",site_url=b"site_url_fixture",workspace_id=b"ws_fixture",user_id=b"user_fixture",issued_at_ms=1767225600000,expires_at_ms=1767225630000);T=lambda f:bytes([f["version"]])+L(f["domain"])+L(f["challenge_id"])+L(f["nonce"])+L(f["caller_id"])+L(f["site_url"])+L(f["workspace_id"])+L(f["user_id"])+Q(f["issued_at_ms"])+Q(f["expires_at_ms"]);M=lambda f:B(hmac.new(K,T(f),hashlib.sha256).digest());S=dict(version=2,domain=b"agent-relay/service-caller/v2",challenge_id=b"ch_00112233445566778899aabbccddeef0",nonce=bytes(range(1,33)),caller_id=b"sc_0123456789abcdee",site_url=b"site_url_fixturf",workspace_id=b"ws_fixturf",user_id=b"user_fixturf",issued_at_ms=1767225600001,expires_at_ms=1767225630001);print("base",M(f));[print(n,M(dict(f,**{n:v}))) for n,v in S.items()]'
```

```text
base FZa8qpzP0ygcFxQ6K7xFTlx6zo8aorEW_JipkqHZaBA
version ytYdc6SC4A5naYk1moQty9PkZ_8CR9GCkxEjy2NpDVk
domain 1hm1pWOYedETLDYKFVoPW62dSjR_fl6oXR7do_ofRuk
challenge_id wImhtl7U6PxbjNh7e4SvQ-nPiaf5y9kgBypFzpvzC2o
nonce Svpce1v9liBMFExSt88igkQ0x2cfWrJi5zUHBekKlco
caller_id NzR8x7ElcNUXO1s6783ZAjNgNQUXLNivHt2XGSRPi8M
site_url woUTvsNxPRuO1ikHE9YrUlI7pkfz0WAqOZysSKRS7jA
workspace_id QI721dcTqq5gbHd_PM3yGKa5a_qOirYPXuQ2V5t5nzU
user_id MJzaetgUe1Vi_YETwSu_jtpdYTtkAgKE7ilLeFRXE7A
issued_at_ms 4qaJpKyxJbKGiQ7R96TSx0k_8J1XOm6vpbVaJf4tF80
expires_at_ms RtEUtwmq3jOyjCZ9if7Uhadgy57dXXpo5m7_NsiGks0
```

Phase 2 commits these values as a JSON fixture,
`desktop-core/tests/fixtures/service-caller-auth-v1.json`. The fixture
has no connection field. Rust and a Node reference runner both read it,
on macOS, Linux and Windows (section 8).

## 4. State machine

All challenge and binding state lives in `Live`, behind the existing live
mutex, keyed internally by the server's `CallContext.connection_id`:
`connections: HashMap<u64, ConnectionAuth>`. The key never leaves the
server. Nothing in the map is persisted. A core restart loses every
challenge and binding, and connection ids start again at 1.

`ConnectionAuth` holds an in-flight count and one state: `Untouched`,
`Challenged {slot, attempts}`, `Bound(b)`, `Stale`, `Locked` or `Closed`.
- Every call that carries a `connection_id` increments the count on its
  connection's entry when it is resolved, creating an `Untouched` entry
  if there is none, and decrements it when it returns. Both happen under
  the live lock.
- An `Untouched` or `Closed` entry is removed when its count reaches
  zero. A connection with no entry is untouched and has no call in
  flight.

### Challenge

```text
(none) --challenge--> issued
issued --authenticate, MAC ok and live--> consumed(success)
issued --authenticate, anything else-----> consumed(failure)   attempts += 1
issued --now >= expires_at_ms------------> expired             attempts += 1
issued --connection_closed---------------> consumed(failure)
```

- A challenge is removed from the slot before verification, which makes
  the transition atomic under the live lock. Two concurrent
  `authenticate` calls cannot both take it: the second finds the slot
  empty and gets `auth_failed`.
- An expired challenge stays in the slot until the next `challenge` or
  `authenticate` on that connection, or until the 30 s sweep, which then
  counts it as an attempt. Using it is `auth_failed`. Expiry is `now >=
  expires_at_ms`, so `expires_at_ms - 1` passes.
- Limits:
  - 1 outstanding challenge per connection;
  - 3 attempts per connection over its life. `attempts` counts challenges
    that were consumed by a failed `authenticate` (including
    `invalid_input`) or that expired, with no binding made. Issued
    challenges are not counted. The third issued challenge is therefore
    outstanding with `attempts == 2` and can still bind. When `attempts`
    reaches 3 the entry becomes `Locked`, and the next `challenge` (the
    fourth) is refused `auth_locked`, as is any `authenticate`, until the
    connection closes;
  - 8 outstanding per `caller_id` across connections, known or unknown
    ids;
  - 256 outstanding in total.

### Binding

```text
untouched --challenge--> challenged(attempts = 0)
challenged --authenticate ok--> bound(g)
challenged --attempt ends without binding, attempts < 3--> challenged (may challenge again)
challenged --third attempt ends without binding--> locked
bound(g) --generation advance / scope change / disconnect--> stale
untouched --connection_closed--> untouched (entry removed when its in-flight count is zero)
challenged | bound | stale | locked --connection_closed--> closed (marker kept until in-flight calls drain)
```

- `bound(g)` holds `authority_generation = g` and the core identity
  generation, and both are rechecked.
- A connection binds at most once in its life. A stale or locked
  connection cannot authenticate again. The runtime opens a new
  connection.
- External is monotonic: no transition leads from `challenged`, `bound`,
  `stale`, `locked` or `closed` back to untouched while any call on the
  connection can still run (see teardown below).

### Generation advance

`CallerRecord` gains `authority_generation: u64`. It is persisted in the
registry, starts at 1, and a missing field reads as 1. It advances by one
on each of these:

1. rotation applied (the approval of a pending rotation, or a direct
   rotate);
2. retirement of an overlapped secret. The first check after
   `retires_at_ms` removes the digest and advances the generation;
3. `set_enabled`, either way;
4. revoke or delete (the record is gone, which is terminal);
5. caller expiry (`expires_at` passed). This advances once, at the first
   check after it;
6. account or workspace change. `registry::load` returns
   `AccountMismatch` and deletes, or returns `WorkspaceMismatch`; or
   sign-out happens. Every binding is torn down, and the registry key no
   longer matches.

An advance holds the live lock from the registry write until it has torn
down every binding of that caller. The torn-down bindings become `stale`,
their iterators close and their leases are released. Any side effect that
takes the lock afterwards sees the binding gone. A change of the core's
`identity_sync_generation` (`Host::generation()`) makes every binding
stale at its next check.

### Recheck before every side effect

Each gated step runs `check_bound` under the live lock and commits under
the same lock. A call resolved as Bound captures that binding's
`binding_id` at call entry. The checks are:
- `connections[connection_id]` is `Bound(b)` (not `Closed`, `Stale`, or
  absent), and `b.binding_id` equals the captured `binding_id`;
- `b.authority_generation` equals the record's;
- `b.identity_generation == Host::generation()`;
- `b.binding_key == Binding::current().key`.

`check_bound` has only two outcomes: proceed as the captured caller, or a
typed refusal (`connection_closed` for a `Closed` marker or an absent
entry, `binding_stale` otherwise). It has no NotExternal outcome.

The gated steps are:
- `spawn.invoke` and `spawn.release`: in `admit`, immediately before the
  Relaycast request;
- `spawn.status`: in `admit`;
- `inbox.open`: before the iterator is inserted;
- `inbox.next`: at the moment a lease is created, after any wait, not at
  call entry;
- `inbox.ack` and `inbox.fail`: before the durable settlement write;
- `inbox.close`, `identity` and `disconnect`;
- management methods and NotExternal `spawn.*` calls that carry a
  `connection_id`: at their side-effect step, the check is that the
  connection's entry is still `Untouched`. The call's own in-flight count
  keeps that entry alive, so the check always sees the real state. A
  connection challenged after such a call began refuses it
  (`management_peer` for management, `caller_unbound` for `spawn.*`).

### Teardown against in-flight calls

The live lock linearizes teardown and every side effect.

1. `connection_closed(context)` takes the live lock. An `Untouched`
   entry stays `Untouched` (its in-flight NotExternal calls keep the
   unchanged session path) and is removed at zero. Contract ruling
   (relayflows-node-spawn, 2026-11-08): untouched legacy calls may keep
   their existing path after a disconnect; the `Closed` marker and its
   semantics apply only to a connection that became external (one that
   was challenged or bound). For such an entry it does the following, and
   releases the lock:
   - sets the entry's state to `Closed`, which drops the challenge or the
     binding but keeps the marker. The in-flight count carries over, and
     a count of zero removes the entry at once;
   - closes every iterator whose internal connection id is that id;
     `inbox::Iterator` already carries the internal `connection_id` and
     gains the `binding_id`;
   - releases their leases (the item becomes deliverable at once);
   - wakes every waiting `next` on those iterators.
2. A woken `next` returns `connection_closed` with no item consumed.
3. A call in flight on that connection reaches its gated step later. It
   finds `Closed` instead of `Bound(b)` with its captured `binding_id`, and
   is refused `connection_closed`, so its side effect never happens.
4. When the last in-flight call on a `Closed` entry returns, it removes
   the entry. Relay #2111 stops reading the connection before it raises
   `connection_closed`, so no new call can carry that id after the close,
   and connection ids are not reused within a core process.
5. A side effect that committed before teardown took the lock is ordered
   before the close. Its leftovers are cleaned up by step 1: a lease
   handed out is released, and an iterator opened is closed.
6. A Relaycast request (`invoke`, `release`) that passed its gate before
   teardown completes. `record` writes its outcome to the caller's ledger,
   which is caller-scoped, not connection-scoped. A later binding of the
   same caller can therefore open, read and release it.
7. If the connection drops during `authenticate`, the challenge is already
   consumed (step 1 of authenticate). The bind step (step 6) finds the
   `Closed` marker and refuses `connection_closed`, and no binding exists.

Why no external call can fall through to NotExternal or the session path
after teardown:
- Resolution happens once, at call entry, from the entry under the live
  lock, and the result is captured for the whole call; it is never
  re-resolved. A call whose connection was external at entry is captured
  as Bound, Known-but-unbound, Stale or Torn down, never NotExternal.
- A captured Bound call reaches every side effect only through
  `check_bound`, whose outcomes are "proceed as this `binding_id`" or a
  typed refusal. After teardown the entry is `Closed`, so it is refused.
  A later binding on another connection has a different `binding_id`, so
  it cannot satisfy the check either.
- A call that resolves after teardown finds the `Closed` marker, which
  resolves to Torn down (`connection_closed`), not untouched. The marker
  outlives every call that could observe it, because it is removed only
  when the in-flight count is zero.
- After the marker is removed, no call can arrive on that connection: the
  core has stopped reading it and does not reuse its id, and no call was
  left in flight (the count was zero). So an absent entry is never seen
  by a call of a connection that was external.
- In the other direction, a call resolved NotExternal on a connection that
  becomes external meanwhile is refused at its gate (the recheck above).
  Its own in-flight count keeps the entry, then `Closed`, from being
  removed under it, so it never acts with session authority on a
  connection that was external.

The `connection_id` used here is server-internal throughout. Nothing in
this argument depends on a client knowing or supplying it.

`connection_closed` must return promptly (relay #2111). It does no I/O
under the lock and audits after it releases the lock.

`SpawnCallers::connection` must not block on I/O either. It resolves a
bound connection from memory only: the binding in the connection's entry,
rechecked (`check_bound`) against the registry as this process last read
or wrote it (`registry::cached`). It reads no file and writes no registry.
Every registry writer (provision, rotation, revocation, enable or disable,
a decision, an expiry or retirement advance, a rollback put back) saves
through `registry::save`, which refreshes that copy before it returns, so
a retired secret or an expired, revoked or disabled caller is refused on
the next call. An advance the clock makes due but no write has recorded
yet (a digest past `retires_at_ms`, an expiry not yet marked) stales the
binding as the write would, and the write is left to the next check that
holds the files lock. A record still marked for rollback answers
`rotation_rollback_pending` without staling the binding. Only a refusal
writes an audit line.

## 5. Error table additions

These rows add to the table in
[service-callers.md section 6](service-callers.md#6-authentication-on-each-call).
The wire is `RpcError {code, message: <class>, data: {kind, retryable,
reason?}}`, with code -32000 unless stated.

| Class (wire `message`) | When | `kind` | `retryable` | `reason` |
| --- | --- | --- | --- | --- |
| `auth_failed` | unknown caller, wrong authenticator, expired, consumed, absent or foreign challenge, retired secret, scope mismatch | `refused` | false | `service_unknown` |
| `caller_unbound` | a binding-required method on an untouched, challenged or internal connection; a NotExternal `spawn.*` call whose connection became external before its gate | `refused` | false | `service_unknown` |
| `binding_stale` | a bound connection after a generation advance, an identity generation change, a scope change or `disconnect` | `refused` | false | `service_expired` |
| `connection_closed` | a call that reaches its gated step, or resolves, after teardown (the `Closed` marker) | `refused` | false | none |
| `already_bound` | `challenge` or `authenticate` on a bound or stale connection | `conflict` | false | none |
| `challenge_outstanding` | `challenge` while the slot holds an unexpired challenge | `busy` | true | none |
| `challenge_capacity` | the per-`caller_id` (8) or global (256) outstanding limit | `busy` | true | none |
| `auth_locked` | `challenge` (the fourth) or `authenticate` after three attempts on one connection ended without a binding | `rate_limited` | false | none |
| `binding_unavailable` | `challenge` or `authenticate` with `connection_id` `None` (core-internal) | `refused` | false | `management_peer` |
| `legacy_credential_refused` | params contain `caller_secret` or `caller_name` | `refused` | false | none |

Classes that are never on the wire, and are audited only as the real cause
of `auth_failed`: `challenge_expired`, `challenge_consumed`,
`secret_mismatch`, `secret_retired` and `caller_unknown` (the existing
names).

- `caller_disabled` and `caller_expired` keep their existing rows and
  appear only after a verified MAC.
- `invalid_input` (-32602) covers shape and bound violations of the new
  params. It reveals only the public format.
- Timing class: every `auth_failed` path does the same work, namely two
  HMAC computations and two constant-time compares, before it answers.
- `peer_check_unavailable` is the Windows policy at this head and
  nothing else: every service-caller call there answers it, before any
  other class (section 1). On macOS and Linux no call reports it; the
  existing `peer_unreadable` and `peer_uid_mismatch` classes apply as
  defence in depth.

## 6. Generated shapes

All of these live in `desktop-core/src/contract/service_callers.rs` and
are registered in `desktop-core/src/contract.rs`.

New methods (`contract.rs`):
- `SpawnCallersChallenge = "spawn_callers.challenge", CallerChallengeParams
  => CallerChallenge`
- `SpawnCallersAuthenticate = "spawn_callers.authenticate",
  CallerAuthenticateParams => CallerBound`

Changed:
- `SpawnCallersInboxAck` and `SpawnCallersInboxFail` now return
  `InboxSettled` instead of `Done`.
- `SpawnCallersIdentity` and `SpawnCallersDisconnect` now take
  `NoParams` instead of `CallerAuthParams`.

| Rust (`service_callers.rs`) | Swift (`CoreContract.swift`, generated) | TS (`@agent-relay/core-bridge`) |
| --- | --- | --- |
| `CallerChallengeParams { caller_id }` | `CallerChallengeParams { callerId }` | `CallerChallengeParams { caller_id }` |
| `CallerChallenge { version, challenge_id, nonce, issued_at_ms, expires_at_ms }` | `CallerChallenge { version, challengeId, nonce, issuedAtMs, expiresAtMs }` | `CallerChallenge` (same snake_case keys) |
| `CallerAuthenticateParams { challenge_id, authenticator }` | `CallerAuthenticateParams { challengeId, authenticator }` | `CallerAuthenticateParams` |
| `CallerBound { caller_id, caller_name, authority_generation }` | `CallerBound { callerId, callerName, authorityGeneration }` | `CallerBound` |
| `InboxSettled { item_id, message_id, action, state, settled_at_ms }`, serialized with the snake_case keys `item_id`, `message_id`, `action`, `state`, `settled_at_ms` | `InboxSettled { itemId, messageId, action, state, settledAtMs }` (camelCase properties; coding keys map to the snake_case wire keys) | `InboxSettled { item_id, message_id, action, state, settled_at_ms }` |
| `InboxSettleAction` (`ack`, `fail`) | `InboxSettleAction` | `InboxSettleAction = 'ack' \| 'fail'` |
| `InboxSettleState` (`acked`, `failed`) | `InboxSettleState` | `InboxSettleState` |
| `CallerErrorClass` adds the ten wire classes of section 5 (`other` fallback kept) | `CallerErrorClass` | `CallerErrorClass` |

No generated shape has a connection id field.

Removed:
- `CallerAuthParams`;
- `RedactedSecret` and its `Debug` uses;
- every `caller_secret` and `caller_name` param field (section 10).

`InboxOpenParams`, `InboxIteratorParams`, `InboxNextParams` and
`InboxSettleParams` keep only their non-credential fields.

Generation:
- `scripts/generate-contract.sh` emits the Swift types with the same type
  names and camelCase properties, as the file already does.
- The TS types are hand-written in relay's `packages/core-bridge`, beside
  `spawn.ts`, and follow its convention: same type names, snake_case wire
  keys.

### Settlement receipt

`InboxSettled` is returned by `spawn_callers.inbox.ack` and
`spawn_callers.inbox.fail`:

```json
{ "item_id": "item_0123456789abcdef0123456789abcdef", "message_id": "…", "action": "ack", "state": "acked", "settled_at_ms": 1767225600000 }
```

| Field | Value |
| --- | --- |
| `item_id` | The settled delivery's `item_id` (`item_` followed by 32 hex characters). |
| `message_id` | The delivery's `message_id`. |
| `action` | `"ack"` or `"fail"`, the method that settled the item. |
| `state` | `acked` after `ack`, `failed` after `fail`. |
| `settled_at_ms` | When the first settlement was committed, in Unix milliseconds. |

The receipt is a record of the first settlement, not of the call:
- An identical repeat (same action, same metadata, same caller) returns
  the byte-identical original receipt, including `settled_at_ms`. That
  holds on the same connection, on a later connection of the same caller,
  and after a core restart.
- The receipt is kept in the item's tombstone for 7 days. The tombstone is
  written durably before the first receipt is returned; if that write
  fails, the call returns `store_failed`, nothing is settled and the item
  stays deliverable.
- Nothing synthesizes a receipt. A timeout, a lease expiry, an empty
  `next` or a lost connection settles nothing.

The rules from Amendment A are unchanged:
- Settlement is idempotent per `(caller_id, item_id)`.
- Any bound connection of the same caller may settle, with or without an
  iterator.
- The opposite action, or the same action with different metadata, is
  `settlement_conflict` (kind `Conflict`, not retryable) and changes
  nothing.
- An item of another caller is `item_not_owned`.
- An unknown item, or one whose tombstone is older than 7 days, is
  `not_found`.
- Errors return no receipt and change nothing.

## 7. caller.json

- The schema is `agent-relay-service-caller/2`. A phase-1 reader
  refuses it, so a phase-1 runtime can never send the new file's secret as
  `caller_secret`. Desktop's reader refuses a `/1` file with a fixed error
  and never upgrades it in place.
- `auth` is an object naming the transcript of section 3 and its two
  calls:
  `{"scheme": "challenge-hmac-sha256/v1", "challenge": "spawn_callers.challenge", "authenticate": "spawn_callers.authenticate"}`.
  A reader refuses any other value.
- `owns_only` (bool) joins `harnesses` and `directory_scope` as scope.
- `inbox` names the result-inbox route on `socket`:
  `{"transport": "core-bridge", "open": "spawn_callers.inbox.open", "next": "spawn_callers.inbox.next", "ack": "spawn_callers.inbox.ack", "fail": "spawn_callers.inbox.fail", "close": "spawn_callers.inbox.close", "disconnect": "spawn_callers.disconnect"}`.
  A reader refuses any other value.
- `secret` keeps its format (`rsc_` plus 43 base64url characters). It is
  now only the input to `K = SHA-256(UTF-8 bytes of secret)`.
- The other fields are unchanged. A top-level field the schema does not
  list is refused. The exact field list is in
  [service-callers-handoff-v2.md](service-callers-handoff-v2.md).

No credential, key or secret field of `caller.json` is ever sent on the
wire.
- `secret` never leaves the reading process; neither does `K`.
- `caller_id` is a nonsecret field, and it is intentionally sent, once per
  attempt, as the `challenge` param. It is a registry id and not a
  credential.
- `workspace.site_url`, `workspace.workspace_id` and `account.user_id` go
  only into the local MAC computation.

The file stays a durable, Desktop-owned handoff:
- It is created 0600 with `fs::private` and `fs::no_follow`, through a
  temp-file rename. On Windows it relies on the per-user profile's access
  list ([service-callers.md section 3](service-callers.md#3-registry)).
- Rotate replaces it. Revoke deletes it.
- Flows is not authorized to unlink, move or rewrite it. Flows reads it;
  which Flows process reads it, and how that process holds the derived
  key, is Flows' architecture and not a Desktop security boundary
  (section 1).

## 8. Test matrix

Status: this matrix is the target for the gated contract; the Windows
named-pipe tests and the Node runner land with Relay R1 and the Desktop
re-pin, and are not present at this head.

Rust tests in `desktop-core/src/service_callers/tests.rs`, unless noted:

| Test | Assertion |
| --- | --- |
| `transcript_matches_the_v1_fixture` | The Rust transcript and authenticator for the section 3 inputs equal the committed hex and base64url, and the fixture has no connection field. |
| `key_is_the_hex_decoded_secret_digest` | Hex-decoding the stored lowercase `SecretDigest.sha256` gives the 32 bytes of `SHA-256(UTF-8 secret)`, and a MAC keyed with the 64 ASCII hex characters does not verify. |
| `node_reference_runner_matches_the_v1_fixture` (relay `packages/core-bridge/src/auth.test.ts`) | A Node `crypto.createHmac` runner over the same connection-free JSON fixture produces identical bytes and authenticator for the base row and every substitution row. It runs on macOS, Linux and Windows. |
| `authenticate_binds_the_connection_and_returns_the_generation` | A valid authenticator returns `CallerBound`, and the next `identity` with no params succeeds on that connection. |
| `replay_of_a_consumed_challenge_is_auth_failed` | Resubmitting the same `challenge_id` and authenticator after a success, or after a failure, returns `auth_failed`. |
| `each_substituted_transcript_field_is_auth_failed` | For each of the 10 transcript fields, an authenticator computed with only that field changed (the fixture rows) returns `auth_failed`. |
| `authenticator_from_another_connection_is_auth_failed` | A challenge and authenticator issued on connection A and presented on connection B return `auth_failed` (A's `challenge_id` is not in B's slot), and A's challenge is unaffected. |
| `expiry_boundary_passes_at_ttl_minus_one_and_fails_at_ttl` | With the host clock at `expires_at_ms - 1` authenticate binds. At `expires_at_ms` it returns `auth_failed`. |
| `concurrent_double_submit_binds_exactly_once` | Two parallel `authenticate` calls with the same valid challenge give exactly one `CallerBound` and one `auth_failed`. |
| `unknown_caller_and_wrong_authenticator_are_indistinguishable` | Unknown `caller_id`, wrong MAC, expired, consumed and absent challenge produce byte-identical errors and the same HMAC call count. |
| `challenge_answers_unknown_caller_ids` | `challenge` for a well-formed unknown id returns a normal `CallerChallenge`. |
| `third_challenge_after_two_failed_attempts_still_binds` | After two failed attempts, the third `challenge` is issued and a correct `authenticate` on it returns `CallerBound`. |
| `fourth_challenge_after_three_ended_attempts_is_auth_locked` | After three attempts ended without a binding (any mix of failed, `invalid_input` and expired), `challenge` and `authenticate` on that connection return `auth_locked`, and a new connection can still bind. |
| `second_outstanding_challenge_is_refused` | `challenge` while one is unexpired returns `challenge_outstanding`, and it does not count as an attempt. |
| `oversized_or_malformed_auth_params_are_invalid_input_and_consume` | Every bound violation returns -32602, the slot's challenge cannot be used afterwards, and it counts as one attempt. |
| `reconnect_gets_a_new_connection_and_no_binding` | After close, a new connection (including one from the same PID) has a different internal id, and `identity` is `caller_unbound`. |
| `pid_is_not_part_of_the_binding` | Changing the reported `peer_pid` on a bound connection does not change the resolution, and a reused PID on a new connection is unbound. |
| `unix_peer_check_stays_defence_in_depth` | On macOS and Linux a peer with another uid is `peer_uid_mismatch` at `challenge`; a matching peer binds. |
| `rotation_invalidates_a_live_binding_before_the_next_side_effect` | After an applied rotate, the next `inbox.next` lease, `ack` and `spawn.invoke` admit on the old binding return `binding_stale`, and no lease or write happens. |
| `revocation_invalidates_a_live_binding_before_the_next_side_effect` | The same holds after revoke, and the iterators of the binding are closed. |
| `disable_and_account_change_invalidate_bindings` | `set_enabled(false)` and an account switch each make every bound call `binding_stale`. |
| `overlap_retirement_ends_a_binding_made_with_the_old_secret` | At the first check after `retires_at_ms` the old digest is removed, the generation advances once, every binding of the caller is `binding_stale`, and a new binding with the new secret keeps that generation. |
| `caller_expiry_advances_the_generation_once` | The first check after `expires_at` (the periodic sweep included) advances the generation once and marks the record `expiry_advanced`; every binding is `binding_stale`; a later check does not advance again; a rotation renews `expires_at`, clears the mark, and the next expiry advances once more. |
| `failed_rotation_rollback_reconciles_before_any_check` | A rotation whose `caller.json` write and rollback write both fail returns `rotation_rollback_pending` (`store_failed` kind); every check writes the old record back before it reads authority and fails while it cannot, so no binding, not even the overlapped old secret, succeeds; then the old secret binds at the old generation. |
| `failed_rotation_rollback_remains_closed_after_restart` | After the same double failure, a cold core over the same state refuses the old secret's bind while the old record cannot be written back. |
| `a_rotation_rollback_pending_across_restart_is_reconciled_from_disk` | The rollback marker is in the registry before the rotation's writes; after the double failure a cold core writes the old record and generation back before its first check when `caller.json` holds the old secret, and keeps the new record when `caller.json` holds the new one. |
| `every_authenticate_failure_reads_the_registry_alike` | A wrong authenticator, an unknown caller, and a consumed, absent or expired challenge each read the registry the same number of times. |
| `management_rechecks_its_connection_at_the_side_effect` | A management call whose connection is challenged, or closes, while it is in flight returns `external_connection` or `connection_closed` before its registry write; a provision leaves the record pending and deletes the agent it registered. |
| `generation_race_side_effect_after_advance_is_refused` | A settlement that reads the record before an advance, but takes the lock after it, is refused, and the item stays unsettled. |
| `connection_drop_during_authenticate_leaves_no_binding` | `connection_closed` between take and bind gives `connection_closed`, consumes the challenge, and leaves only the `Closed` marker, which is removed when the call returns. |
| `connection_drop_during_next_releases_the_lease_at_once` | A pending `next` is woken with `connection_closed`, its iterator is closed, and the item is deliverable to another connection immediately, not after 60 s. |
| `teardown_admission_race_refuses_the_late_side_effect` | A call captured as bound that reaches its gate after `connection_closed` finds the `Closed` marker, returns `connection_closed` and performs no side effect. |
| `closed_marker_is_kept_until_in_flight_calls_drain` | While a captured bound call is in flight after close, the entry stays `Closed`; it is removed only when that call returns. |
| `no_external_call_resolves_not_external_after_teardown` | Every method called on a closed external connection, in flight or newly resolved, returns `connection_closed` and never takes the session path; a captured `binding_id` never matches a later binding of the same caller on another connection. |
| `not_external_call_on_a_connection_that_becomes_external_is_refused` | A `spawn.invoke` resolved NotExternal on a connection that is challenged before its gate returns `caller_unbound` and sends no Relaycast request. |
| `relaycast_request_in_flight_at_teardown_is_recorded_to_the_caller` | An invoke past its gate when the connection closes lands in the caller's ledger, and a new binding can release it. |
| `same_name_session_never_resolves_as_the_caller` | A session process whose name equals a caller's name takes the session path on an untouched connection and gets `caller_unbound` on inbox methods. |
| `bound_connection_never_takes_the_session_path` | `spawn.invoke` on a bound connection whose peer is inside a session tree acts as the caller, never the session. |
| `legacy_session_path_is_unchanged` | `spawn.invoke`, `status` and `release` with no auth from a session peer produce the same requests and results as before. |
| `legacy_caller_secret_is_refused_on_every_method` | Any of the 15 methods with a `caller_secret` or `caller_name` key returns `legacy_credential_refused` before any other check, except that `authenticate` first consumes and counts the outstanding challenge (section 2); a clean retry of the same body is `auth_failed`. |
| `management_is_refused_on_an_external_connection` | Provision, rotate, set_enabled, revoke, decide and list on a challenged or bound connection return `management_peer`. |
| `internal_calls_cannot_challenge` | `challenge` with `connection_id` `None` returns `binding_unavailable`. |
| `disconnect_tears_down_the_binding` | After `disconnect`, every call on the connection is `binding_stale`, and `challenge` is `already_bound`. |
| `settlement_receipt_states` | First `ack` returns `{item_id, message_id, action: "ack", state: "acked", settled_at_ms}` and first `fail` the `failed` receipt; the identical repeat returns the byte-identical receipt on the same connection, on a later connection of the same caller and after a restart; the opposite action or different metadata is `settlement_conflict`; another caller is `item_not_owned`; unknown or past-tombstone is `not_found`; a failed durable write returns no receipt and leaves the item deliverable. The serialized receipt keys are exactly `item_id`, `message_id`, `action`, `state` and `settled_at_ms`. |
| `values_appear_only_where_allowed` | After a full run, including failures: the secret, `K` and every digest appear in no request, response, error, log, audit line or delivery; `authenticator` appears only in `authenticate` requests; `nonce` only in `CallerChallenge` responses; `challenge_id` only in `CallerChallenge` responses and `authenticate` requests. |
| `no_response_or_fixture_carries_a_connection_id` | No response, error, delivery or fixture contains a connection id; only audit lines do. |
| `audit_lines_carry_only_caller_connection_generation_and_class` | Every auth audit line's identifying fields are exactly `caller_id`, the internal `connection_id`, `generation` and the class. |
| `restart_drops_every_challenge_and_binding` | After a core restart, a pre-restart `challenge_id` on a connection with the same internal id is `auth_failed`. |

The Windows policy at this head (section 1) is tested on every platform
through the `peer::UNAVAILABLE` failpoint, and on Windows itself by the
`cfg(windows)` unit test in `peer.rs`:

| Test | Assertion |
| --- | --- |
| `without_a_peer_check_every_method_and_connection_is_peer_check_unavailable` | With the failpoint set, `challenge`, `authenticate`, a bound and a per-request `identity`, `list` from the app and from the probe, and `connection` on a connection bound before, all answer `peer_check_unavailable` (`management_peer`) and make no Relaycast request. |
| `the_windows_peer_check_reports_itself_unavailable` (`peer.rs`, `cfg(windows)`) | `peer::caller`, `peer::management` (with and without a secret) and `peer::available` answer `PeerCheckUnavailable`. |

Target, once a Windows peer counterpart exists (section 9): Windows
named-pipe tests, `cfg(windows)`, over a real
`\\.\pipe\agent-relay-<hash>` with the owner-only DACL (in
`desktop-core/src/service_callers/tests.rs`, run in the gate's Windows
jobs):

| Test | Assertion |
| --- | --- |
| `windows_peer_check_is_not_a_gate` | `peer::caller` on Windows admits a pipe client whose process cannot be read; no service-caller call reports `peer_check_unavailable`. |
| `windows_named_pipe_challenge_and_authenticate_bind` | A pipe client answers the challenge with the fixture's construction and gets `CallerBound`; the next `identity` succeeds, with no `peer_check_unavailable` anywhere. |
| `windows_named_pipe_replay_is_auth_failed` | Replaying a consumed `challenge_id` and authenticator on the same pipe connection, or on a second pipe connection, returns `auth_failed`. |
| `windows_named_pipe_revocation_tears_down_the_binding` | Revoke while a pipe connection is bound makes its next `inbox.next`, `ack` and `spawn.invoke` `binding_stale`, closes its iterators and releases its leases at once. |
| `windows_named_pipe_close_tears_down_the_binding` | Dropping the pipe client wakes a pending `next` with `connection_closed` and releases its lease immediately. |

The cross-language fixture (section 3, no connection field) is checked by
the Rust test and by the Node runner on all three platforms, so a
Windows Flows runtime and the Windows core agree byte for byte.

The two existing redaction tests
(`no_log_audit_or_error_contains_a_caller_secret_or_token` and
`no_response_serializes_a_caller_secret_or_token`) are rewritten to send
no secret. They then assert the refusal class and the absence of `K`.

## 9. Decisions (relayflows-node-spawn ruling, 2026-11-08)

The contract authority ruled on the open questions and the key
construction. This section records the ruling; the rest of the document
applies it.

Key. Symmetric HMAC-SHA256, not Ed25519. `K = SHA-256(UTF-8 bytes of the
caller.json secret)` is acceptable because the secret is a uniform
256-bit capability and the Desktop registry is already a trusted secret
store holding the stronger Relaycast token. `SecretDigest.sha256` is
lowercase hex; verification hex-decodes it to the same 32 key bytes. A
CSPRNG nonce, constant-time compare and atomic consume are kept
(section 3).

1. **Windows is supported**, not `peer_check_unavailable`. The confirmed
   policy is user-authored local Flows on macOS, Windows and Linux. The
   owner-restricted named-pipe DACL plus challenge possession is the
   boundary; any PID or token check is defence in depth, not a platform
   gate (sections 1 and 2, Windows tests in section 8). Not yet
   implemented: until the core has a Windows peer counterpart, every
   service-caller method on Windows answers `peer_check_unavailable`
   (section 1).
2. **Receipt keys are snake_case on the wire**: `item_id`, `message_id`
   and `settled_at_ms`. Generated Swift properties may remain camelCase
   (section 6).
3. **Every connection authenticates independently**, with its own
   3-attempt budget. There is no binding transfer between connections
   (section 4).
4. **Outstanding challenge limits**: 8 per `caller_id` across all
   connections, 256 in total (sections 2 and 4).
5. **Relay R1 first, then the pin.** The core asks the plugin for the
   connection's resolution (`SpawnCallers::connection(&CallContext) ->
   ConnectionCaller { NotExternal, Bound(ExternalCaller), Refused(RpcError)
   }`) instead of `resolve(workspace, name, secret)`, and refuses
   `caller_secret` / `caller_name` keys on `spawn.*` as
   `legacy_credential_refused`. This lands in AgentWorkforce/relay first,
   then the `agent-relay-node` pin moves, as AGENTS.md requires.
6. **A repeat returns the original receipt.** There is no separate
   "already settled" state: an identical repeat returns the byte-identical
   first receipt, from the durable 7-day tombstone (section 6).

Blocking amendments, applied before implementation:
- A. The trusted-proxy model (Amendment C) is replaced by the project's
  decision of 2026-11-08: user-authored local Flows are in the user's trust domain on
  every platform, and proxy, sandbox or same-UID isolation is not a gate
  (section 1, section 7).
- B. `connection_id` is server-only `CallContext` provenance. It is not in
  `CallerChallenge`, `CallerBound`, the transcript or the fixture; the
  server binds its per-connection challenge slot internally. Audit may
  record it (requirements 2, 4 and 9; sections 2, 3 and 6).
- C. The lock counts attempts that ended without a binding, not issued
  challenges. The third issued challenge can bind; the fourth challenge
  is refused `auth_locked` (section 4, section 8).
- D. Where each value may appear is stated precisely (requirement 9,
  section 8 `values_appear_only_where_allowed`).
- E. `caller_id` is a nonsecret field intentionally sent in `challenge`;
  no credential, key or secret field is sent (section 7).
- F. Teardown keeps a `Closed` marker until in-flight calls drain, and
  every bound side effect checks its captured `binding_id`; the argument
  that no external call falls through to NotExternal is in section 4.
- G. Ordering: see below.
- H. `caller_secret` removal, tri-state resolution, generation recheck,
  settlement receipts, transcript fixtures and the test matrix are kept
  and updated, with Windows named-pipe fixtures and the connection-free
  cross-language fixture added (sections 2 to 8, section 10).

Ordering note G. This proposal is the gate for publishing
`@agent-relay/core-bridge` 17.2.2 and for any Desktop user release. It is
not the gate for the first preview end-to-end run. That run may use the
current per-request `caller_secret` after relay #2111, under quarantine:
preview only, never in a published core-bridge or a Desktop user release.

## 10. Removal list

Phase 2 deletes everything below. Line numbers are at Desktop 58098e33
and relay d01c830b (#2111).

### Desktop: Rust

| File | What |
| --- | --- |
| `desktop-core/src/contract/service_callers.rs` | `RedactedSecret` and its doc line (9); the management doc comment (83); `caller_secret` fields at 89, 103, 125, 144, 157, 167 (`CallerListParams`, `CallerProvisionParams`, `CallerRotateParams`, `CallerEnabledParams`, `CallerIdParams`, `CallerDecideParams`); `CallerAuthParams` (`caller_name`, `caller_secret` at 236); `caller_name`/`caller_secret` in `InboxOpenParams` (259), `InboxIteratorParams` (272), `InboxNextParams` (280) and `InboxSettleParams` (344); the redacting `Debug` (355). |
| `desktop-core/src/service_callers/methods.rs` | `self.authenticate(peer_pid, &params.caller_name, &params.caller_secret, …)` at 151, 172, 222, 334, 430, 458, and `ServiceCallers::authenticate` itself. |
| `desktop-core/src/service_callers/manage.rs` | `self.manager(peer_pid, &params.caller_secret, METHOD)` at 107, 122, 320, 451, 481, 518. The `secret_given` presence check moves to the raw-key `legacy_credential_refused` check. |
| `desktop-core/src/service_callers/peer.rs` | The `secret_given` parameter and its refusal (44 to 52, 95 to 97). The Windows policy is `peer::available`, which every Windows `caller` and `management` answers with `PeerCheckUnavailable` today; once a Windows peer check exists it becomes the defence-in-depth counterpart of section 2. |
| `desktop-core/src/service_callers/registry.rs` | `RegistryFile::authenticate(name, secret, now)` and `CallerRecord::check_secret` on plaintext. They are replaced by MAC verification against the stored digests. |
| `desktop-core/src/service_callers/spawn.rs` | `resolve(&self, workspace, name, secret: &Secret)` and `secret.expose()` (187, 202). |
| `desktop-core/src/service_callers/tests.rs` | `caller_secret` request construction at 424, 518, 538, 607, 617, 896, 965, 2604. The tests at 879 and 1783 are rewritten (section 8). |

### Desktop: Swift (generated; regenerate, do not edit)

| File | What |
| --- | --- |
| `macos/Sources/RelayDesktopCore/CoreContract.swift` | `callerSecret: String?` and its coding keys in `CallerAuthParams` (967), `CallerDecideParams` (1018), `CallerEnabledParams` (1078), `CallerIdParams` (1163), `CallerListParams` (1255), `CallerProvisionParams` (1305), `CallerRotateParams` (1422), `InboxIteratorParams` (4518), `InboxNextParams` (4572), `InboxOpenParams` (4610), `InboxSettleParams` (4716), `SpawnInvocationParams` (10138), `SpawnInvokeParams` (10177) and `SpawnReleaseParams` (10242). |

### Desktop: docs

| File | What |
| --- | --- |
| `docs/relay-core.md` | Line 593: the `{"caller_name","caller_secret",…}` row, "authenticated on every call". |
| `docs/service-callers.md` | Section 1, the #2103 list: `caller_secret` on the spawn params and "Without it every `caller_secret` is refused". Section 4: "the call carries a `caller_secret`". Section 6: the classes `secret_mismatch` / `secret_on_management` as wire-reachable. Section 10: the two redaction test names. This proposal already replaced the per-request sentences. |

### Relay (AgentWorkforce/relay; change first, then move the pin) — R1

| File | What |
| --- | --- |
| `crates/relay-node/src/relay/contract/spawn_control.rs` | `caller_name` and `caller_secret` fields (16, 35, 56) of `SpawnInvokeParams`, `SpawnInvocationParams`, `SpawnReleaseParams`; `RedactedSecret` (64); the redacting `Debug` (80, 96, 107); the tests `debug_never_prints_the_caller_secret` (174) and `debug_shows_an_absent_caller_secret_as_none` (211). |
| `crates/relay-node/src/relay/daemon/plugins.rs` | The `SpawnCallers` docs (108, 110, 253) and `resolve(workspace, name, secret: &Secret)`. They are replaced by the connection resolution (section 9, decision 5). |
| `crates/relay-node/src/relay/spawn/control.rs` | `params.caller_secret.take()` at 393, 478, 599. Path selection moves to the connection resolution. |
| `crates/relay-node/src/relay/spawn/control/tests.rs`, `crates/relay-node/src/relay/spawn/control/tests/callers.rs` | Every `caller_secret` request (2 and 16 occurrences). |
| `packages/core-bridge/src/spawn.ts` | `caller_secret?` (52, 73, 86); the key lists (159, 170, 176); `callerSecret()` (217 to 221) and its calls (272, 313, 339). |
| `packages/core-bridge/src/spawn.test.ts`, `packages/core-bridge/src/paths.test.ts` | Every `caller_secret` case (21 and 3 occurrences). |
| `packages/core-bridge/README.md` | Lines 130 and 162: `caller_secret` as the service-caller credential. |
| `CHANGELOG.md` | Line 35 describes the optional `caller_secret`. Add a superseding entry; history is not rewritten. |

### Logs and audit

- `audit.rs` `Line` gains the server-internal `connection_id` and
  `generation`. No line ever
  holds a name, secret, digest, key, nonce, `challenge_id` or
  authenticator.
- No `tracing` call in `service_callers` takes a params struct by
  `Debug`. Once the fields are gone, `RedactedSecret` has nothing left to
  redact.
