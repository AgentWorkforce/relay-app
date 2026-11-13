# Starting a Capy thread through the relay core

The relay core can start a headless Capy thread without receiving the
workspace's Capy key. Callers use typed relay-core methods; the core uses the signed-in person's Agent Relay Cloud session, and Cloud
makes the Capy request with the encrypted workspace key.

## Ownership and authorization

The Capy service-user key stays in Agent Relay Cloud under **Settings →
Workspace → Capy**. Cloud stores the encrypted key and never returns it to the
desktop app or relay core. Thread creation, binding, delivery subscriptions and
grants therefore remain in the same server-side trust boundary.

The workspace routes require a person connection whose current workspace is
the workspace in the path. Only a current workspace owner or admin may list
projects, start a thread or read its spawn status.

## Shipped Cloud API

Relay Desktop uses these session-authenticated routes under `/api/v1`:

- `GET /workspaces/{workspaceId}/capy/projects?cursor=` returns
  `{"projects":[{"id","name"}],"truncated","nextCursor"}`. When
  `truncated` is true, the app passes `nextCursor` to the next request.
- `POST /workspaces/{workspaceId}/capy/thread-spawns` accepts
  `clientRequestId`, `projectId`, `task`, `model`, optional
  `pullRequestAuthor` (`user` or `capy`), and optional `reportTo`. The
  relay core always sends `model`; see **Model** below.
- `GET /workspaces/{workspaceId}/capy/thread-spawns/{threadId}` returns the
  current thread and relay-binding status. The path uses Capy's thread id, not
  the client request id.

The create and status responses are:

```json
{
  "thread_id": "jam_…",
  "title": "…",
  "status": "…",
  "binding": { "state": "pending" }
}
```

After the first turn binds the thread, `binding` is
`{"state":"bound","agent":"capy-…"}`. Bound means the binding is active,
its delivery subscription is installed, and its grant is live.

Cloud returns safe JSON errors as `{"error":"…","retryAfterSeconds":…}`.
Relevant statuses are 401, 403, 404, 409 when Capy is not connected, 429 and
502. Neither Cloud nor the desktop logs the Capy key or returns a raw provider
response.

## Model

Capy has no default model: a thread started without one is accepted, binds
to the relay, and then fails inside Capy with `model_not_selected: No model is
selected and the caller supplied no default`. The relay core therefore
requires a non-empty `model` on `capy.threads.spawn`, as Cloud's
`spawn_capy_thread` MCP tool does, and refuses a missing or blank one with
`invalid_params` (`Give a Capy model in "model", for example
"anthropic/claude-sonnet-5-5".`) before it calls Cloud, so a refused call
never reaches Capy.

## Idempotency and first-turn binding

The caller keeps one `clientRequestId` per thread it means to start and sends
the same id again after a failed call, a timeout or a restart; a new id starts
a new thread.

Cloud derives Capy's required request id from the workspace, signed-in person
and client request id:

```text
agent-relay-<sha256(workspaceId + NUL + userId + NUL + clientRequestId)>
```

Cloud always creates an organization-visible thread, does not export the
service-user key, and prefixes the task with an instruction to call
`bind_capy_thread` before doing the work. Replaying a create with the same
client id therefore returns the same Capy thread.

## Relay core

The relay core exposes `capy.projects.list`, `capy.threads.spawn` and
`capy.threads.get`. It supplies the current workspace from its Cloud session,
reduces responses to the shared contract models and never exposes credentials.

Starting Capy threads is an agent operation: no window, menu or button in the
macOS, Linux or Windows app starts one. A workspace with no Capy connection is
refused with a pointer to **Agent Relay Cloud → Settings → Workspace → Capy**;
other Cloud errors stay bounded to project access, write access, rate
limiting, reachability and binding status.
