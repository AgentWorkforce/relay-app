# Native mobile continuation across harnesses

Claude continuation is documented in [mobile-claude-control.md](mobile-claude-control.md).
The next adapters use their own core setting, `mobile_native_continuation`,
on unless turned off once agent hosting is enabled and the signed-in account
has claimed this computer. In desktop Settings it shares the one **Continue
sessions from your phone** switch with Claude's `mobile_claude_handoff`; an
agent changing settings over the session socket may still set them apart.

## Current adapter scope

| Session | Native input and correlated mobile answer | Approval handling |
| --- | --- | --- |
| Claude Code terminal | Shipped in v2026.10.60 through its native plugin | Mobile-origin requests use verified owner tickets; desktop requests stay on desktop |
| Pi | Updated Relay extension in the same running session | Existing Pi policy and extensions remain in charge; no new mobile approval bridge |
| OpenCode terminal | Updated Relay plugin capability protocol 1; native user message ID and exact assistant parent correlation | Existing session permissions; prompts remain in OpenCode |
| Managed OpenCode V1 server | Authenticated live binding; compatible V1 health/version and native prompt/readback | Existing session permissions; no launch-policy changes |
| Codex app window | Verified owning window only, using native user input with a stable client message ID | Compatible live windows offer single-request approve/deny for command and file requests from mobile-origin turns; other prompts remain in Codex |

These new adapters forward the answer to the mobile-origin turn. Unlike the
Claude plugin, they do not subscribe the phone to subsequent desktop-origin
answers. They do not implement background transfers or session merges. Mobile
approval forwarding is limited to the Codex command/file requests described
below. The computer must stay awake and the provider session
must remain running. Windows reports this experiment unavailable until the
corresponding native transport has been verified.

Pi keeps a neutral delivery marker with its native input because its extension
API does not accept an external message ID. The marker is for durable reply
correlation; the input does not contain peer-message attribution or instructions
claiming user authority. OpenCode and Codex use provider message IDs instead.

Only the verified signed-in owner's direct messages use these native paths.
This mobile setting does not change other agents' or channels' routes. The
separate **Native input from the spawning agent** setting remains independent;
it does not give a spawning agent the owner's mobile approval authority.
Current account, workspace,
registration, opt-in and runtime proof are checked again before admission.
Disabling the setting prevents new native admissions; a message a provider has
already accepted into its queue may still finish. Uncertain input is observed
or reported uncertain, never resubmitted through a different route. Provider
receipts retain the originally selected native mode across retries and restarts.

## Codex mobile command and file requests

The existing Codex window remains the approval owner. After a native mobile
message starts a verified turn, a bounded observer attaches to that exact
window and turn. Only current command and file requests with supported
single-request decisions receive approve/deny references in the owner's mobile
conversation. Desktop-origin turns and other approval types stay in Codex.
Commands are shown in full within a bounded notice; file requests list each
affected path and operation. Requests that cannot be represented completely,
or whose details would expose recognized secrets, stay on the computer.
Reply with the supplied `approve cxm-<reference>` or `deny cxm-<reference>`
command. An expired or withdrawn reference is rejected without starting a
new chat turn.
The phone describes this capability only while the core advertises the exact
`mobile_native_continuation_approvals = "command_file"` value.

Every decision checks the current account, workspace, computer owner,
registration, opt-in, connection and pending request. A private durable record
is written before sending the decision so retries or restarts cannot apply it
twice. A transport acknowledgement is not proof that Codex applied the
decision; the request may already have been handled in the desktop window.
An uncertain decision is never resent. Disconnecting, changing ownership or
turning the setting off withdraws the mobile reference without automatically
denying the desktop prompt. Missing transport support or observer capacity
leaves approvals in Codex.

## Remaining harnesses and required evidence

| Route | Required before native continuation can be enabled |
| --- | --- |
| Codex terminal queue / Conductor | Stable native message-ID receipt mapping without the current peer-text marker; proof that the owning client displays the turn and keeps its approvals |
| Codex shared app-server without a verified window | Proof of a subscribed approval-owning client; a loaded thread alone can outlive its UI |
| Grok / GrokOpen | Version-specific proof that existing-session input preserves live policy: current upstream source limits client `--always-approve` injection to new sessions, but `session/load` can refresh a live session from leader permission defaults. Direct `session/prompt` to a resident session is a candidate that avoids that refresh; durable prompt/answer collection and permission-event handling still need implementation and verification |
| Muse terminal | A native user-input surface; the current courier calls peer messaging. Paseo-hosted Muse must be verified separately against its native prompt and session/trust binding |
| Devin | Verified existing-session ACP request/response ownership and correlated answers; the lifecycle-hook inbox is peer context, and a write-only pipe is not a complete continuation contract |
| Claude Cloud | A remote-session input, result and approval contract; the local Claude plugin cannot authenticate a cloud session |
| OpenClaw | Immutable verified-owner routing to the intended existing chat, separate from name-based trusted-sender rules, with native approval capability |
| Oh My Pi | Verification against a real Oh My Pi session that the shared extension's owner-native input starts the turn without the peer marker and keeps Oh My Pi's policy; until then its extension advertises no native capability and the core never sends it native input |
| Hermes | Selection of the person's existing chat identity; the current trusted-sender route still creates a separate per-sender session |

No unsupported route is promoted merely by removing its Relay prefix. The
core advertises capability only for the adapter and live runtime actually
verified. These remaining routes retain their existing peer behavior.

## Acceptance test for each new adapter

1. Use a disposable workspace and an existing session with its normal model,
   tools and permission settings. Record its session ID and a context phrase.
2. Enable the new setting. Reload the Relay extension/plugin where required;
   leave a Codex app window open on the target conversation.
3. Open the existing mobile conversation. Reading alone must do nothing.
4. Send a harmless context question. Verify the existing session receives a
   native user turn, its answer reaches the phone, and the ID/policy did not
   change. A delivery receipt proves input acceptance, not task completion.
5. Exercise a normal provider approval. For compatible Codex windows, verify
   command/file requests from the mobile turn can be approved or denied once
   from the phone. Resolve another request in the desktop window first and
   confirm a stale mobile decision cannot affect a replacement request. Check
   that desktop-origin and unsupported prompts remain on the computer. Other
   adapters keep their existing approval UI. Do not use unrestricted launch
   flags to make this test pass.
6. Test a busy session, disconnect/reconnect, duplicate delivery and opt-out.
   There must be at most one provider submission. An uncertain result must
   remain uncertain rather than start a replacement session or peer turn.
7. With the separate spawning-agent native-input setting off, send the same
   text from another agent and a channel. It must retain peer attribution and
   cannot use the owner's input or approval authority. Where the separate
   requester grant is supported, enabling it must preserve that native route
   without granting the requester's messages owner-only mobile permissions.

Automated adapter and core tests cover local protocol, authority and replay
boundaries. A physical phone with production Cloud must still be tested per
provider before claiming the full app journey has been validated.
