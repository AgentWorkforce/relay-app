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
 "hooks": [{"type": "command", "command": "<probe> claude-attestation-hook", "timeout": 20}]}
```

The matcher covers the user-scope `agent-relay` server and the same server
bundled in a plugin. Install, repair and uninstall add and remove only this
group's Agent Relay command; other hooks and settings stay. Claude Code's setup
reports **Setup needed** until both the MCP entry and the hook are present.

For each call to a tool that acts as the chat, the hook reads Claude's
`session_id`, `tool_name` and `tool_use_id`, and asks the core over the session
socket (`POST /claude/attestation`). Tools that never act as the chat are not
signed and the hook prints nothing for them: directory reads
(`list_relay_agents`, `get_relay_agent_card`), shared-session reads, team
activity and screens (`show_ui`, `show_ui_guide`). Any other tool, including
one Cloud adds later, is signed. The core signs only when all of these hold:

- Cloud advertised `capabilities.claude_session_attestation: 1` for the
  signed-in account and identity-sync generation;
- the process asking runs inside that Claude Code chat (from its ancestry and
  Claude's own session registry, never from the hook's input alone);
- that chat is registered on this installation and present, and Cloud accepted
  the exact native sender and installation key in an identity sync less than
  30 minutes ago for the current account, workspace and source;
- the tool belongs to the hosted `agent-relay` server.

The proof is a compact JWS (`alg: EdDSA`) with protected header
`{"alg":"EdDSA","kid":"<kid>","typ":"agent-relay-attest+jws"}` and payload
`{"v":1,"kind":"claude","sid","src","tuid","tool","iat","exp"}`, where `sid` is
the lowercase session UUID, `src` the installation's identity-sync source ID,
`tuid` Claude's `tool_use_id` (equal to the call's
`_meta["claudecode/toolUseId"]`), `tool` the name without its `mcp__…__`
prefix, and `exp` is `iat + 60`.

How the proof reaches Cloud depends on Cloud:

- **Handoff** (Cloud advertises `capabilities.claude_attestation_handoff: 1`):
  before answering the hook, the core sends the proof to
  `POST /workspaces/{workspace}/messaging/desktop-identities/attestations` as
  `{"attestation": "<proof>"}`, signed in as the person, and waits for Cloud's
  2xx for the rest of its budget (below). The hook then prints nothing, so the call goes out exactly
  as Claude made it. Cloud matches the stored proof to the call by
  `params._meta["claudecode/toolUseId"]`, which Claude Code sets and the model
  cannot. A native `join_relay` keeps the name the model passed; Cloud accepts
  it when it is the chat's verified desktop name. When the core reports HTTP 503
  with `handoff_failed`, the hook denies the call and asks Claude to retry. Claude
  Code's auto mode refuses any call whose input a hook changed, so with the
  handoff every hosted tool works in auto mode.
- **In the arguments** (an older Cloud without the handoff): the hook returns
  `hookSpecificOutput.updatedInput`, the call's own arguments with
  `_agent_relay_attestation` set to the proof, replacing any value the model
  supplied; a native join also drops `name` and `description`. Auto mode
  refuses these calls.

The call waits for Cloud to store its proof, because Cloud looks for the proof
when the call arrives. The three waits are nested so each side is still
listening when the side it asked answers: the core answers within 17 seconds,
the hook waits 18 seconds for it, and Claude Code gives the hook 20. A refresh
of the chat's identity may use at most 7 of the core's seconds, leaving the
handoff at least 10. Cloud usually stores a proof in under 3 seconds and has
taken up to 6. A core that still has not answered when the hook stops waiting
may still be handing the proof to Cloud, so the hook denies the call and asks
for a retry instead of sending it on ahead of its proof. If the current identity
still lacks a fresh acceptance, the hook also denies the tool and asks for a
retry. The hook also denies a join it cannot complete. For other calls, when no
core is listening, the chat is unregistered, or signing otherwise fails before
handoff, the hook prints nothing and exits 0. Cloud refuses unsigned managed
actor operations.

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

### Claude aliases and hosted dispatch

Cloud advertises `capabilities.claude_active_alias: 1` alongside
`claude_session_attestation: 1` after it supports the following contract.
For a renamed chat, Desktop exports the original address and its own
credential for existing links, and the current alias and its own credential
with the sole native Claude SID. Retired aliases still route inbound locally.
An unrenamed chat still has one identity row. Without the new capability,
Desktop sends the original row and the existing version 1 proof.

With the capability enabled, Desktop includes `claude_attestation_version: 2`
in the sync body and both handoff and argument proofs use `v: 2`.
They add `agent_id`, `link_address`, `workspace_id`, `user_id`, and
`credential_sha256` to the version 1 payload. `link_address` is the original,
stable address from the verified local registration. `credential_sha256`
is the unpadded base64url encoding of SHA-256 over the UTF-8 tool-use ID,
one zero byte, and the UTF-8 token from the native row. The token itself is
sent only in identity sync. Desktop uses the PUT receipt’s `synced` count
and `skipped[].agent_id` to identify accepted rows. The receipt must echo
`claude_attestation_version: 2` when that version was stored. Desktop checks the
current binding again before returning a proof and after a successful handoff.

The hosted consumer must store the proof version atomically with the source’s
identity snapshot. At dispatch, it must reject any version 1 proof for a source
synced with version 2, even if the older proof has yet to expire. Older Desktop
sources continue to use version 1. The consumer must compare the signed user
and workspace to the authenticated request. At tool dispatch it must resolve exactly one native
row for the signed source and SID and verify the signed agent ID and digest
against that row’s authenticated credential. This applies to both
`_agent_relay_attestation` and the proof consumed from handoff. A changed
row, token, source or owner requires a fresh proof. The handoff must preserve
this binding until tool use and retain the existing tool and Claude
`claudecode/toolUseId` matching, expiry and replay checks. A managed join
may recognize the signed stable address for this same chat, or the verified
current address. The resolved native credential remains the actor and the
original address remains available to explicit legacy links.

The Desktop fixture can be exported by setting
`RELAY_CLAUDE_ALIAS_FIXTURE` to an output file when running the
`rename_exports_the_stable_link_and_only_the_current_native_credential`
Rust test. It contains invented chat identities, a disposable public key,
and proofs for both transports. A consumer test can set its verifier clock
to the fixture proof’s `iat` and exercise alias rotation between proof and
dispatch, rejection of a different chat’s address, and continued delivery
to the original address. Enable the capability after those consumer paths
are verified.

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
