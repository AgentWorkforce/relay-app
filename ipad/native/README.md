# Agent Relay Mobile

The native SwiftUI app runs on iPhone and iPad. It signs in through Agent Relay Cloud and connects directly to Relaycast over HTTPS and WebSocket. Its two sections show the Relay roster and direct messages; workspace channels are not shown on iOS, where they were too noisy. The compact header keeps the Agent Relay logo on the left; iPad keeps it in the left rail. It can start an agent on any online machine that advertises the chosen agent type, then wait for its readiness receipt. It also shows the saved Cloud session overview and can request a new one. Agent badges use the CLI's artwork when its type is known, and the interface follows the device's light or dark appearance. It does not need a Mac bridge, local network access, or Tailscale. An internet connection is required.

The app gets the signed-in person's messaging identity from Agent Relay Cloud,
the same identity used by the dashboard and desktop app. DMs sent from any of
those surfaces therefore share one Relaycast conversation history on iPhone,
iPad, macOS, and the web. Earlier messages sent from an installation-specific
mobile agent remain in that agent's old conversations. APNs refreshes an active
message screen immediately; a slower reconciliation read remains for missed
notifications.

From **Agents**, **Search all summaries** opens workspace-wide semantic search.
Queries wait briefly while a person is typing, then show the matching summary
passage, agent, and update time. Selecting a result opens that agent's existing
conversation when one is available. Search reports empty, offline, and server
error states in place so the roster remains usable.

People in the roster and Messages list show their Cloud display name and profile
photo, including the header of an open conversation. The app loads public profile
fields from `GET /api/v1/workspaces/{workspaceId}/messaging/people` with its existing
Cloud credential and joins them to human peers by exact handle. Profile data stays
in memory and is cleared on workspace changes and sign-out. Photos use anonymous
HTTPS requests without Cloud credentials or cookies; unsafe URLs, unavailable
photos and older Cloud servers fall back to the person's initials. Agent artwork
is unchanged. This is an iPhone/iPad client integration; the macOS and Linux clients
already have their own Cloud people/avatar flow and are unaffected.

## Install

The App Store Connect listing is **Agent Relay Code** (`com.agentworkforce.relayipad`). Its Home Screen label is **Agent Relay**. The same TestFlight invitation works on iPhone and iPad.

After installing, tap **Sign in**, then **Continue sign-in**: the Agent Relay page opens inside the app (`ASWebAuthenticationSession`, sharing Safari's cookies, never switching to Safari). Sign in there and approve this device; the app closes the page itself once Cloud reports the approval. **Cancel** leaves sign-in waiting, and **Continue sign-in** reopens the page. The app stores the Relaycast workspace key, the person's Cloud-issued messaging token, and a Cloud refresh token in the iOS Keychain. Sign out in Settings to remove them. Users who signed in before build 8 need to tap **Enable summaries** once in Settings to grant the Cloud credential. When a generated summary is unavailable, the app shows the agent's published objective, its native session title, and the original user request from uploaded session events, when available. Native titles arrive with the roster and do not request generation. OpenClaw and Hermes gateway agents have a title but no linked single conversation; their agent IDs are not used to request History summaries.

In **Agents**, the roster shows reachable peers by default; choose **All** when you intentionally need inactive history. Both filters exclude your signed-in identity and old forwarding names from renamed agents. Other people remain available to message. Temporary Relaycast database overloads are retried automatically for safe reads using the server's requested delay. Tap **Start an agent** to choose an online machine and one of the agent types it advertises. The first time you use a project on that machine, save its name and full folder path there, starting with `/` (`~` is not expanded); the app keeps a project list per machine for future starts. Folders on an external drive may not be readable by the machine's background agents. A project is optional for a general task. Enter a unique agent name and task, then start it. The app waits for the machine's readiness receipt, or for the machine to list the agent as running, before claiming the agent is running; if neither arrives within about two minutes it says the start is unconfirmed so you can check **Agents** before trying again. A failed start explains the cause and keeps the machine's message under **Details**. A live fleet agent has a **Release** button in its detail view; the app confirms the stop and checks the node's receipt. To branch off a Claude or Codex agent's conversation, tap the fork button beside **Call** in its detail view, optionally say what the fork should do next, and tap **Fork session**: the agent's computer starts a new agent in the same folder with a copy of the whole conversation (Claude's `--fork-session`, Codex's `thread/fork`), and the original keeps running. Only the computer's signed-in owner can fork its sessions, and the computer must run an Agent Relay that advertises forking; otherwise the sheet says to update it. Pin an agent from its row to keep it at the top of that section; pins are saved per workspace and section on this device. Agent rows show the machine as `@machine`. In a conversation, tap the paperclip to upload files and include them with a message; tap a received or sent attachment to open its download link. Chat messages clear from the composer immediately, show a pending bubble, and can be edited again if delivery cannot be confirmed. Your latest message shows **Sent**, then **Delivered** once Relaycast lists the agent as a reader of it, which the broker records when it puts the message into the agent's session. New incoming messages show a blue dot on iPhone and iPad; opening the chat clears it, and the device remembers that state after restart. The app reconciles relay presence every 30 seconds while open, when returning to the foreground, and when switching tabs. **Messages** shows agent conversations only while their peer is reachable on the current relay roster; cached metadata and the **All** roster filter cannot restore a departed agent. People’s conversations remain available while they are offline. Existing messages are retained on Relaycast and return if the agent rejoins. Machine menus combine hostname variants (case, punctuation, and `.local`) and filter all matching agents together. In **Messages**, swipe left on a conversation and tap **Hide** to remove it from the list on this device without deleting anything; it comes back when a new message arrives. **Show hidden** at the end of the list shows hidden conversations, and swiping left there offers **Unhide**. Conversation groups have local date and time dividers; swipe left to reveal each message's exact local time. VoiceOver also announces the time with each message. Web links in chat are tappable. Scrolling any list or the conversation dismisses the keyboard, as does tapping empty space in the conversation or the **Hide keyboard** button beside the message composer, the terminal composer and the search field; on iPhone an open conversation also shows **Hide keyboard** in the navigation bar while you type, and the composer stays above the keyboard. The new agent and new message fields have a **Hide keyboard** button in the keyboard toolbar. Tap **Agent overview** to collapse it; when the agent's session isn't in Cloud History it says so instead of offering **Generate**.

Workspace **channels** have no screen in the iOS app: the Channels section was removed because it was too noisy. Channel notifications still arrive as banners; tapping one opens the app and nothing more. Channels remain in the desktop apps and on the web.

Every build can start a **voice call** with an agent: tap **Call** in the conversation header, which stays next to **Detach** while you view or drive the agent's terminal. Calls use **GPT-Live** voice from OpenAI. The first call asks for consent to send call audio to OpenAI; choosing **Not now** doesn't start the call, because voice calls need GPT-Live. For each call, the app asks the Agent Relay Cloud voice broker to start a session (the broker holds the OpenAI key), then talks to GPT-Live over WebRTC. GPT-Live handles the conversation and hands each request to the app, which sends it to the agent as a DM and passes the agent's reply back for GPT-Live to say. GPT-Live bills silence, so the session closes after a minute of inactivity or when the app leaves the foreground; tap **Resume** to continue. On-device voice is not a choice; it is only the automatic fallback when GPT-Live is unavailable, out of monthly minutes or can't reconnect, so the call keeps going, and the call screen shows **On-device voice (fallback)**. The next call tries GPT-Live again. In the fallback, each spoken turn is transcribed on the device and sent to the agent as a DM, and the agent's replies are read aloud with the best installed voice for the device's language (premium, then enhanced, then the default voice; download better voices in iOS Settings > Accessibility > Spoken Content > Voices). Earlier Debug builds let people choose on-device voice; that choice is cleared, so those people get GPT-Live after agreeing to it. Choose **Open mic** or **Push to talk**; speak over the agent to interrupt it, or say "stop" or "end call". A request that sounds like it could delete, deploy, merge or spend something waits for **Send to agent** on screen. The `RelayVoiceCallEnabled` Info.plist value, which `project.yml` sets to `YES` for Debug and Release, keeps a switch for turning voice calls off in a future build. See [`docs/specs/ios-voice-call.md`](../../docs/specs/ios-voice-call.md).

Summary search is controlled by the `RelaySummarySearchEnabled` Info.plist value. `project.yml` sets it to `YES` for Debug and `NO` for Release, so **Search all summaries** is available during development but hidden from production builds.

For development, open `AgentRelayiPad.xcodeproj` in Xcode and run the `AgentRelayiPad` scheme. Its `AgentRelayUnitTests` target covers app logic such as hidden conversations. The project is generated from `project.yml` with XcodeGen. SwiftTerm supplies the terminal emulator.

## TestFlight releases

Use the **iOS TestFlight** workflow's `inventory_only` option to read Apple's
current versions, processing states and **Internal Devices** build membership
without an upload or any tester changes. The read-only mode reports explicit
membership separately from the group's access to all builds. Its short-lived
API token permits only GET requests, and the report excludes credentials and
tester identities.

Increment `CFBundleVersion` and `CURRENT_PROJECT_VERSION` in `project.yml`, then run the **iOS TestFlight** workflow from GitHub Actions. The workflow uses a macOS 26 runner with a current iOS SDK, signs and uploads the app to App Store Connect, and requires the dedicated `APPLE_MOBILE_API_KEY_BASE64`, `APPLE_MOBILE_API_KEY_ID`, and `APPLE_MOBILE_API_ISSUER` repository secrets. The team API key needs Admin access for Apple's cloud-managed distribution signing. Check App Store Connect processing and the **Internal Devices** group before telling testers to update.


Voice calls are on in every TestFlight and App Store build, which also declare background audio; the workflow fails if an archive doesn't. An `app-store` upload also needs `voice_privacy_ready: true`, confirming the privacy policy and App Store privacy answers cover call audio sent to OpenAI. Choose `internal` (the default) for internal TestFlight-only builds, or `app-store` for a build eligible for App Store review and TestFlight. Internal-only uploads cannot be converted into review-eligible builds. The `app-store` path uses `ExportOptions-AppStore.plist`; uploading does not submit or publish the app. Build 33 is a draft release candidate: do not upload it until the direct account-deletion URL in Settings is implemented and tested, the live build-number inventory is checked, and a maintainer approves the upload.

Settings and the sign-in screen link to the privacy policy and support page. Settings > **Delete account** opens `https://agentrelay.com/cloud/account/delete` in an in-app Safari view (`SFSafariViewController`), so account deletion, like account creation, happens without leaving the app. That page must be deployed and verified before release; sign-out only removes this device's credentials. The draft remains blocked until that production page completes the deletion flow. `PrivacyInfo.xcprivacy` declares app-scoped UserDefaults access; it does not replace the App Store privacy questionnaire or assert that no data is collected.

## Experimental Claude continuation

On the Mac or Linux computer running Claude, enable **Continue agents from mobile
(Experimental)** in Agent Relay Settings → Relay once. This candidate requires
Claude Code 2.1.310 or later in the 2.1 release series. Restart Claude or run
`/reload-plugins` once in an already-open session after enabling the setting.
Keep that session and Agent Relay running. Send your message in the existing
mobile conversation; reading it sends nothing. The session stays in place with
its existing conversation and permissions. Native tool prompts can be approved
or denied by the verified machine owner for that single request only.

Connecting or a queued message is not a delivery confirmation. A failed or
uncertain connection asks you to check message status before trying again.
The offline `MobileHandoffUITests` exercise these states without a production
account; the Mobile CI workflow saves their screenshots with the test results.

For this first experimental candidate, first enable **Accept spawns from Agent
Relay** in Settings and wait for the computer to be claimed by your signed-in
account. Mobile Claude continuation uses that existing ownership proof. Enabling
continuation before the claim is ready is refused before any plugin or setting
change; a peer message cannot establish the claim.

After your first mobile message, replies stay synced to your phone, including
replies to later messages you type in that same desktop conversation. They stay
bound to the verified owner and current control generation. Before mobile
engagement, desktop replies are not forwarded. Permission prompts for turns you
start on the desktop continue to use Claude's native desktop prompt.

The native bridge currently supports macOS and Linux only. Windows shows the
setting as unavailable until Claude's native connection is verified with named
pipes. The plugin connects directly from Claude; tool subprocesses are refused.
