# Hosted MCP runtime and native chat identity

Agent Relay retains the hosted MCP endpoint. Its credential helper supplies the
app's current login and a desktop-integration marker; it does not run an MCP
server or choose a chat from the helper process, environment, or a transport ID.

Provider maintenance reloads the existing reachable Codex app-server after a
configuration change. It enumerates loaded chats and requests their own MCP
status on that same connection. A successful reload acknowledgement alone does
not establish that any chat has the tools. Healthy unchanged configuration is inspected
without repeatedly reloading every server; incomplete verification retries after
a bounded cooldown. Status reads use the last completed
result and do not block the settings UI on runtime IPC.
The native proxy [connects to the existing control socket](https://github.com/openai/codex/blob/970111f6cb62cc02fe36a82ea7d71b37a2889c97/codex-rs/stdio-to-uds/src/lib.rs#L12)
and propagates connection failure; it does not bootstrap a new daemon.

Codex caches helper headers per connection. Its native HTTP transport refreshes
the helper once after a same-origin POST receives 401/403, retrying only when
headers change. This obtains renewed app credentials without replacing the chat;
unchanged or refused credentials remain an error. See the
[pinned transport implementation](https://github.com/openai/codex/blob/970111f6cb62cc02fe36a82ea7d71b37a2889c97/codex-rs/rmcp-client/src/http_headers.rs#L216).
Native status inspection intentionally does not run the helper and may report
authentication as unknown. A connected tool catalog is therefore separate from
app login health and per-call sender verification.

The compatible hosted server accepts Codex's reserved per-call `_meta.threadId`.
It resolves that ID against identities synced by the signed-in desktop account
and workspace, then verifies the immutable Relaycast identity. MCP reconnects
do not change the sender, and separate chats sharing a transport do not share an
actor. Missing, conflicting, or duplicate mappings are rejected instead of
creating a hosted participant or borrowing the transport's previous binding.
Managed calls cannot select another actor through tool arguments.

Desktop identity sync negotiates support before including native session IDs.
The credential helper requires the hosted server's native-identity capability
before exporting authentication. Deploy the compatible hosted authentication service and its migration
before releasing this desktop build. An older server remains usable for its
existing non-desktop integrations; the new desktop helper fails closed.

## Boundaries still requiring work

| Runtime | Automatic reload and identity |
| --- | --- |
| Ordinary registered Codex chat in the reachable existing app-server | Native reload and chat-specific tool inspection; stable identity requires the compatible hosted server and successful desktop sync. |
| Private Codex app-server or standalone runtime without a reachable control channel | Configuration is installed for future loads; the current chat is not verified or restarted. |
| Registered Claude Code chat | Each hosted tool call carries a per-call proof from the PreToolUse hook (below); identity requires the compatible hosted server. Automatic reload is not verified: a chat loads changed configuration in a new session or after an MCP reload. |
| Unregistered Claude Code chat | No proof is signed, so actor operations, including sending, fail closed. |
| Spawned workers | The Cloud worker ownership protocol is separate from ordinary desktop identities. A native worker binding must verify that provenance before it can participate in this identity path. |

These boundaries are not evidence that the user's session needs restarting:
they are missing integration capabilities. Do not simulate success with a new
sidecar, a model instruction to run `join_relay`, or a connection-wide chat ID.

Codex supplies `threadId` from its runtime outside model-visible arguments:
[pinned call-site source](https://github.com/openai/codex/blob/970111f6cb62cc02fe36a82ea7d71b37a2889c97/codex-rs/core/src/mcp_tool_call.rs#L550)
and [reserved metadata injection](https://github.com/openai/codex/blob/970111f6cb62cc02fe36a82ea7d71b37a2889c97/codex-rs/core/src/mcp_tool_call.rs#L1424).

## Claude Code chat identity

Claude Code caches `headersHelper` output per MCP connection, and one process
can change chats (`/resume`, `/clear`) while keeping the connection, so headers
cannot name the chat. Agent Relay installs a PreToolUse hook beside the hosted
entry in each shared Claude login's `settings.json`:

```json
{"matcher": "^(mcp__agent-relay__.+|mcp__plugin_.+_agent-relay__.+)$",
 "hooks": [{"type": "command", "command": "<probe> claude-attestation-hook", "timeout": 5}]}
```

The matcher covers the user-scope `agent-relay` server and the same server
bundled in a plugin. Install, repair and uninstall add and remove only this
group's Agent Relay command; other hooks and settings stay. Claude Code's setup
reports **Setup needed** until both the MCP entry and the hook are present.

For each call the hook reads Claude's `session_id`, `tool_name` and
`tool_use_id`, and asks the core over the session socket
(`POST /claude/attestation`). The core signs only when all of these hold:

- Cloud advertised `capabilities.claude_session_attestation: 1` for the
  signed-in account and identity-sync generation;
- the process asking runs inside that Claude Code chat (from its ancestry and
  Claude's own session registry, never from the hook's input alone);
- that chat is registered on this installation and present;
- the tool belongs to the hosted `agent-relay` server.

The proof is a compact JWS (`alg: EdDSA`) with protected header
`{"alg":"EdDSA","kid":"<kid>","typ":"agent-relay-attest+jws"}` and payload
`{"v":1,"kind":"claude","sid","src","tuid","tool","iat","exp"}`, where `sid` is
the lowercase session UUID, `src` the installation's identity-sync source ID,
`tuid` Claude's `tool_use_id` (equal to the call's
`_meta["claudecode/toolUseId"]`), `tool` the name without its `mcp__…__`
prefix, and `exp` is `iat + 60`. The hook returns
`hookSpecificOutput.updatedInput`: the call's own arguments with
`_agent_relay_attestation` set to the proof, replacing any value the model
supplied. It never sets `permissionDecision`. When the core is unreachable, the
chat is not registered, or signing fails, the hook prints nothing and exits 0;
the call proceeds without a proof and Cloud refuses actor operations.

Each installation has one Ed25519 key, created by the core in
`claude-attestation-key.json` (mode 0600) under its app data directory. The
private key never leaves the machine and is never logged. `kid` is base64url
(no padding) of the first 16 bytes of SHA-256 over the raw 32-byte public key.
After Cloud advertises the capability, desktop identity sync sends
`"attestation_key": {"kid", "alg": "Ed25519", "public_key": "<base64url>"}` and
names each registered Claude Code chat with
`"native_session": {"kind": "claude", "id": "<session UUID>"}`. Until then
neither field is sent, the core signs nothing, and Claude Code's setup reports
**Messaging unavailable**. Deploy the hosted verification before relying on
this desktop build for Claude messaging.

## Development Codex control socket

Dev Codex profiles use `~/.agent-relay-dev/<app-data-hash>/codex` for both
`CODEX_HOME` and `CODEX_SQLITE_HOME`. The ordinary Codex daemon and the existing
Agent Relay proxy compute the same default control socket from this short path.
Each dev install has a separate owner-checked namespace; symlinked profiles or
auth/config files are rejected. Normal Codex profiles and credentials are not
copied or changed. Other dev provider profiles remain under app data.

## macOS service updates

Updating a signed app reloads its selected fallback LaunchAgent through the
service's own handoff. The plist and approval preferences remain intact;
transient bootstrap failures retry only while the service label is absent.
The app records the new binary revision after the handoff succeeds. Linux
systemd services and Windows logon tasks are unaffected by this macOS fix.
