# caller.json schema `agent-relay-service-caller/2`

The handoff file Desktop writes for an approved service caller. The code
is `desktop-core/src/service_callers/handoff.rs`; this list matches its
`CallerFile` exactly.

## Fields

Every field is required except `rotated_at`. A top-level field not in
this list is refused.

| Field | Type | Value |
| --- | --- | --- |
| `schema` | string | `"agent-relay-service-caller/2"` |
| `auth` | object | `{"scheme": "challenge-hmac-sha256/v1", "challenge": "spawn_callers.challenge", "authenticate": "spawn_callers.authenticate"}` |
| `caller_id` | string | Registry id. Sent only as the `challenge` param. |
| `caller_name` | string | The caller's Relaycast agent name. |
| `publisher` | object | `{"id", "name"}`: the caller's Relaycast agent id and name. |
| `workspace` | object | `{"site_url", "base_url", "workspace_id", "relaycast_workspace_id"}` |
| `account` | object | `{"user_id"}` |
| `secret` | string | `rsc_` plus 43 base64url characters. Only in this file. |
| `created_at` | string | RFC 3339 |
| `rotated_at` | string, optional | RFC 3339; absent until the first rotate |
| `expires_at` | string | RFC 3339 |
| `harnesses` | array of strings | Harness ids it may spawn |
| `directory_scope` | array of strings | Resolved folder roots it may spawn in |
| `owns_only` | bool | Whether it may release only agents it started |
| `socket` | string | The core bridge endpoint (Unix socket path, or the Windows named pipe) |
| `inbox` | object | `{"transport": "core-bridge", "open": "spawn_callers.inbox.open", "next": "spawn_callers.inbox.next", "ack": "spawn_callers.inbox.ack", "fail": "spawn_callers.inbox.fail", "close": "spawn_callers.inbox.close", "disconnect": "spawn_callers.disconnect"}` |
| `desktop_version` | string | The core version that wrote it |

Scope is `harnesses`, `directory_scope` and `owns_only`. Binding is
`caller_id`, `workspace` and `account`: the challenge transcript uses
`caller_id`, `workspace.site_url`, `workspace.workspace_id` and
`account.user_id` ([service-callers-auth.md](service-callers-auth.md)
section 3). `inbox` is the result-inbox route on `socket`, not a file
path. The file never holds the Relaycast token.

## The file

- Default path `<app data>/ServiceCallers/handoff/<caller_id>.json`, or an
  absolute `handoff_path` in an existing folder this user owns and no one
  else can write to.
- Created 0600 through a temporary file and a rename, refusing a symbolic
  link at the path. Rotate replaces it; revoke deletes it.

## Reading

Desktop's reader checks `schema` first, then the whole shape, then `auth`
and `inbox`. It fails closed with fixed text that never holds a value
read from the file:

| Case | Error |
| --- | --- |
| Missing or not a private regular file | `caller file is missing or unreadable` |
| `schema` is `agent-relay-service-caller/1` | `caller file schema agent-relay-service-caller/1 is no longer accepted; rotate the caller in Agent Relay to write an agent-relay-service-caller/2 file` |
| Any other unknown `schema` | `caller file schema is not supported by this version` |
| `auth` differs from the value above | `caller file auth scheme is not supported by this version` |
| Not JSON, no `schema`, a missing or mistyped field, an unknown top-level field, or `inbox` differing from the value above | `caller file is not a valid agent-relay-service-caller/2 file` |

A `/1` file is never upgraded in place. A rotate writes a `/2` file. The
reader's `Debug` output shows `secret` as `<redacted>`.
