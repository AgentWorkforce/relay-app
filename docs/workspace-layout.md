# Workspace layout

Sessions is the desktop landing page on macOS, Linux and Windows, including
when the signed-in window is rebuilt after an update or sign-in. The window
is the sessions table plus Settings (and, on Linux and Windows, the Messages
page); the earlier My agents, Team and Projects pages are gone.

In desktop Settings one row, **Continue agents from mobile**, covers
both native routes from the phone and is on unless turned off: Claude Code
through `mobile_claude_handoff` and the other adapters through
`mobile_native_continuation` (a connected Pi session with the updated Relay
extension, an OpenCode session with a compatible Relay connection, or a Codex
app window). The switch writes both settings; an agent changing settings over
the session socket may still set them apart. Both take effect only once
machine ownership is verified for the signed-in account, and until then the
row is off with a status line saying it is waiting for that. Older cores omit
the row; the `*_supported` capabilities disable it on unsupported platforms.
An upgraded computer that had either earlier provider-specific permission
turned off keeps it off; the next person-initiated change sets every
permission that core exposes to the chosen state.

These owner messages use the existing conversation without changing its
permissions. Compatible Codex app windows can send command and file approval
requests for mobile-origin turns to the verified owner's phone. Other prompts
remain on the computer, which must stay awake with the session running.
Messages already accepted into a provider's queue may
finish after opt-out. Pi retains a neutral delivery-ID suffix for durable
receipt matching; it adds no peer attribution to owner input. Mobile shows
**Queued** only for explicit desktop `mobile_native_continuation_enabled`
metadata. The notice describes Codex command/file approvals only when the live
connection advertises `mobile_native_continuation_approvals = "command_file"`;
otherwise it says approvals remain on the computer. Opening the
conversation does not start a turn or enable the feature.

Claude's route (`mobile_claude_handoff`) is shared by macOS and GTK
(Linux/Windows), on by default once ownership is verified, and unavailable on
older cores. The core then installs the owned `agent-relay-mobile`
skills-directory plugin in the configured Claude directory on startup and on
each discovery pass; turning the row on explicitly installs at once, and an
installation failure stores no choice.
If that login's `skills` directory is a symbolic link, Relay resolves it and
installs in the real directory only when both the link and target belong to the
current user and the target is not writable by other users. Links at or inside
the managed `agent-relay-mobile` directory are still refused.
Claude Code 2.1.310 or later in the 2.1 release series is required. After
enabling, restart Claude or use `/reload-plugins` once in an existing session;
later sessions load it automatically. Messages from the verified machine owner become native user
turns in the same running session. Reading a mobile conversation does nothing.
The plugin does not poll: a `curl` it starts holds a doorbell open on the
session socket, Agent Relay adds a line when a message waits, and the plugin
then collects it as Claude itself. While a message waits and Claude is idle,
each delivery retry rings again, so a missed ring costs one retry. A failed
collection or answer is retried with backoff; without `curl` the plugin asks
every two seconds ([details](mobile-claude-control.md#how-a-message-reaches-the-plugin)).
This candidate does not invoke `/background`, kill Claude, or reconstruct its
permissions. Unsupported versions or unverifiable native processes are refused.

The native plugin uses the session socket; kernel process identity selects the
session and immutable Relaycast identity selects the owner. Native tool prompts
use the existing sanitized approval notices. An owner approval allows only that
request with its original tool input, never changes permission rules, and expires
after five minutes. A disconnected HTTP poll can retry the same request;
disabling the setting, replacing the native process or changing its binding
invalidates it. Native question
flows remain in Claude. Mobile status is display-only metadata, and queued sends
remain queued until a delivery receipt arrives. Claude itself opens the native Unix-socket connection; tool subprocesses are
refused. Windows keeps the setting visible but disabled until Claude's native
transport is verified with named pipes. There is no subprocess fallback. No
separate mobile takeover or return controls are introduced.

The shared core's `src/relay/diagnostics.rs` (relay's `crates/relay-node`) retains explicit,
credential-redacted diagnostics in rotating `core.log` files. macOS uses
`~/Library/Logs/Agent Relay` (or `Agent Relay Dev`); Linux and Windows use
the resolved app data directory's `logs` folder. Rotation happens during
operation at 1 MiB with three backups; stderr remains available to terminal
and systemd journal consumers. Raw child stdio and request/message bodies
are not captured.

The Sessions helper writes `collector_performance` JSON records to its existing
`collector.log`, beside the installation's `history.db`, with a 10 MiB rotation
threshold and three backups. macOS and Linux rotate the log during operation.
Windows rotates it at the next start; performance summaries pause when the log
has no room for the complete record under 10 MiB, including the final summary.
Other existing diagnostic lines are unaffected by this summary limit. A record
is emitted after a completed pass once a minute, plus a nonempty final window
on a clean stop when space permits. These local diagnostics work
with networking blocked and can be read or collected by an organization's log
tools. The record contains fixed operation names and numeric measurements, plus
the build version and `sessions` role. It contains no session identities, paths,
transcript content, or credentials.

`process_cpu_ms` measures CPU consumed by all threads in the Sessions helper over
`window_elapsed_ms`, including time between passes. `process_cpu_percent_one_core`
is the average for that observed window, where 100% means one fully occupied CPU
core. Both CPU fields are null when a platform sample is unavailable. This minute
average does not measure short peaks. The phase fields ending in `elapsed_ms`
measure wall time for delivery, compaction, retention and census, so they can
include disk and lock waits. Census outcomes distinguish incremental updates,
full recounts and failures, including the number of identities examined.

The reproducible census experiment uses synthetic history only. Run
`cargo test --manifest-path desktop-core/Cargo.toml --lib census::experiment -- --ignored --nocapture`
to compare the legacy rewrite, membership diff and retention-journal update on
5,000 sessions with small retention batches. Each pass checks the census against
an authoritative identity walk. The output separates census wall time, identities
judged, full recounts and SQLite change counts. Missing or expired journal
coverage still falls back to a full recount; journal and ordinary-feed positions
are committed with the membership changes. The collector performance
module also contains an ignored logging-overhead experiment.

Codex desktop routing lives in the core's `src/relay/codex/ipc.rs`. Its Windows
transport, `ipc_windows.rs`, connects to the same user's `codex-ipc` named pipe
using the bounded I/O implementation in `relay/transport/windows.rs`.

Existing Pi sessions appear in the session table after loading the bundled
extension with `/reload`. Enable **Receives messages** to register one. Messages
wait for Pi to be idle, and completed final answers return through Relay on
both platforms. See [Pi delivery](pi-existing-session-injection.md).

Pi queues preserve arrival order while busy, and channel batch answers reply to
the last post. A failed extension installation remains visible in status details
and Settings → Relay on both platforms; fix the conflict or permissions and
restart Agent Relay to retry, then run `/reload` in Pi.

Existing Oh My Pi (`omp`) sessions use the same extension, installed in
`~/.omp/agent/extensions`. Oh My Pi loads extensions only when a session
starts: new sessions connect on their own, and one already open connects after
`omp --continue`. They are listed as **Oh My Pi** and work like Pi sessions on
both platforms. See [Oh My Pi delivery](omp-existing-session-injection.md).

Spawn recovery and startup diagnostics run in the shared core on both platforms.
Known spawn origins survive legacy store merges; Codex origins omitted by an old
writer can be restored from matching local receipts and Relaycast identity.
Failed launches report structured, redacted harness reasons and retire ended
intents after acknowledged cleanup. OpenCode warns when model startup remains
unverified; Grok reports clearly missing default credentials as unavailable.

Replicas spawning is an agent operation through Cloud, like Capy. The shared
core exposes environment listing, durable spawn/status, and release methods
([contract](relay-core.md)); both desktop clients receive the same cloud roster.
There are no new session-table controls. The existing local
[operator bridge](../integrations/replicas/README.md) can remain during migration.

Cursor Cloud is also an agent operation through Cloud, not a local spawn
target and not the discontinued `cursor-agent` CLI integration. The shared core
exposes model and agent listing, idempotent launch, status and results,
follow-up injection, and reversible archive/unarchive methods. Agent Relay
Cloud keeps the per-user Cursor key, registers the hosted session with
`session_kind: cursor-cloud`, and translates Relay DMs into Cursor's native
steer-or-queue follow-up. Both apps show the Cursor mark and **Cursor Cloud**
wording for those remote roster rows; neither app adds a platform-only form.
See [Cursor Cloud agents](cursor-cloud.md).

Spawned-agent settings render the core's harness descriptors on both platforms.
Muse appears when its trusted CLI and relay delivery are available, with its
sandbox on by default and the same full-access wording on macOS and Linux.
The shared core records Muse's detached host before launching its child so a
restart during startup can recover or stop the exact owned process group.

Native Codex spawns remain messageable after Codex unloads an idle thread:
the shared core resumes the registered, unarchived thread before starting
the follow-up turn. Ordinary stopped sessions still wait for their host.

Spawned Muse and Devin expose a per-launch MCP tool for Relay messages. It uses
the authenticated local session socket and accepts only the recipient, message
text, and optional reply parent. Shell sandboxing stays enabled by default;
Relay replies do not require granting shell access to the socket.

The macOS, Linux and Windows windows open directly to the session list on this computer.
One 64-point header contains the 24-point Agent Relay mark and 19-point canonical wordmark, **Search or filter sessions…**, the icon-only **Table display** control,
and the signed-in person's avatar menu. The
avatar menu contains **Settings…**, **Log out…**, and **Update available** when
applicable. Open Settings with **⌘,** on macOS or **Ctrl+,** on Linux and Windows. The logo returns to Sessions. There is no persistent navigation rail.
Native window controls and dragging remain platform-owned.

| Region | Contents |
| --- | --- |
| Global header | Sessions logo, search, Table display icon, avatar menu |
| Sign-in notice | Under the header, only while connected but without a Cloud session the core may use: **Agent Relay needs you to sign in again.** / **Messages, session summaries and subscriptions wait until you do.** and a primary **Sign in again** (**Waiting for the browser…** while the browser approval runs), on macOS, Linux and Windows |
| Table | Session (including Active indicator), Project, Updated, Receives messages, Upload (status icon and switch) |
| Status bar | Total indexed sessions on this machine and **Live Agents on Relay: N**, with a green dot for a nonzero live count and a grey dot for zero |

Selecting **Live Agents on Relay** opens Sessions with `is:active is:on-relay`, replacing
previous search filters and returning from any open transcript.

Provider setup is offered in the post-sign-in chooser and Settings › Providers,
without duplicate Capy or Devin banners above Sessions. Capy's chooser row uses
**Set up in Capy** rather than a disabled checkbox. That action, also on its
provider page, opens the installed app through `capy://app/new?prompt=…`,
falling back to `https://capy.ai/new?prompt=…`, with the initial prompt
`Set me up on Agent Relay: https://github.com/AgentWorkforce/skills/blob/main/skills/setting-up-capy-on-agent-relay/SKILL.md`.
It copies the same text as a fallback. The person sends the prefilled prompt
and approves the plugin in Capy.

Session messaging controls live on their provider pages: Grok, OpenCode,
Devin, Cursor and Codex, with OpenClaw and Hermes gateway controls on their
own pages too. Cursor has a provider sidebar entry when installed, detected
from `.cursor` on all platforms or its macOS application. Its provider setup
installs authenticated MCP and, on macOS and Linux, its local messaging hooks.

The single 64-point header and table share 24-point horizontal gutters.
The search panel's **Messages** section offers **Receives messages** and
**Does not receive messages**. These mutually exclusive choices add readable
chips to the search bar; removing the chip includes both message states again. They match
confirmed local registrations (including offline sessions) or their absence,
independently of activity, and combine with all other filters without changing
grouping or sorting. There are no separate session navigation tabs. Other computers
and teammates remain available in Cloud. Search is hidden in Settings and while
reading a transcript. Settings has a contextual 244-point sidebar. **Back to sessions** sits in the
global header immediately after the wordmark, in the same position as on a transcript.

Group by offers None (default), Harness, Project, Status, and Week. Week groups
start on Monday in UTC, matching Cloud. Status means the actual upload state;
the desktop does not infer Cloud's work-completion status from process liveness.
Search matches local titles, projects, paths, harnesses, IDs, and relay names,
including the earlier name of a renamed agent: `agent:` finds it by either.
A partial or exact agent name or address finds its session; a term written
`@reviewer` searches agent names and addresses only.
Project uses the Git repository name when local metadata is available, so
linked worktrees and nested directories stay grouped with their repository.
Broker controller directories under the configured relay home are
infrastructure, not projects, and display **—**; unrelated directories keep
their folder name as a fallback.
The shared core resolves project metadata off the async runtime, with at most
four dedicated filesystem workers. Local session lists, upload history and
previews share a 32,768-directory FIFO cache with a five-minute TTL, including
missing names and timed-out lookups. Each lookup has a 250 ms deadline covering
both worker admission and all filesystem operations (including canonicalization
and Git marker checks); lists resolve concurrently rather than paying that
budget for every row. A deadline returns the folder name without filesystem
access. Hung reads keep their worker slot until they actually return, so repeated
refreshes cannot exhaust async workers or create unbounded blocking threads.
Late results cannot replace the cached fallback, and dedicated workers do not
hold up runtime shutdown. Both desktop clients use this same core behavior.
Session labels prefer RelayHistory's harness-provided catalog title, with the
existing prompt-based fallback when no title is available, on both platforms.
Sessions spawned through Agent Relay show their task as the session title and
original request, without the relay delivery header and report instructions.
Relayed messages use the same task text on macOS, Linux and Windows.

Title editing lives only in the open session's transcript header on macOS,
Linux and Windows. The sessions table shows clickable titles without rename
controls or title-preload requests. A quiet, always-visible **Rename** button
with an edit icon makes the header title editable in place. The text keeps its
font, alignment and height; there is no input border or background box.

Enter or **Done** saves; Escape or **Cancel editing** discards the draft.
Moving focus outside the whole editor saves after a short debounce, so moving
between the text and its controls does not submit prematurely. Returning focus,
cancelling, submitting or leaving the session retires that pending save.
Unchanged titles do not issue a write, and typing makes no network requests.

The sparkle button, **Use generated title**, fills the draft with the saved
pipeline title and returns focus to the text. Opening a session preloads its
existing summary, cached by harness and session identity with bounded reads.
Preloading and using the suggestion never request a new generation. The sparkle
appears while editing once a saved title is available; applying the suggestion
does not save it until the person commits or leaves the editor.

The shared core reports native support through `sessions.title.capability`;
`sessions.title.set` changes the original harness title only where a supported
route is available. An unavailable Rename button explains the harness or session
limitation in its tooltip. There is no fallback label stored only in Relay.
Saves update the displayed title optimistically. A failure rolls back the title,
shows a warning with the full error in its tooltip, and reopens the rejected
draft for correction and retry.
Interrupted requests are retired on reconnect, and account changes clear cached
suggestions and pending edits. Confirmed titles remain visible while discovery
catches up, for at most one minute before refreshing authoritative state, so
later title changes in the harness are not hidden indefinitely. A title change
does not rename the session's Relay messaging address.

Running agents and historical sessions share one table. **Filters → Activity →
Active** adds `is:active` to search; no separate cards duplicate the same sessions.
All filters apply together to the whole list. Active means local discovery reports
`live`, independent of upload selection or relay registration. Inactive means no
live local process was discovered; it does not imply the work was completed.

Search accepts free text, `title: Foo`, `title:"Fix login"`, `project:cloud`,
`harness:codex` (or `source:`), `path:`, `id:`, and `agent:`. Combine qualifiers
with spaces (AND), quote phrases, and prefix a term with `-` to exclude it.
`is:active`, `is:inactive`, `is:on-relay`, and `is:off-relay` filter activity/registration;
`upload:uploaded`, `upload:local`, `upload:in-progress`, and `upload:discovering` filter
upload state. Specific upload states such as `paused` and `queued` also work.

Dates refer to the session's Updated time in UTC. `updated:2026-10-11` covers
that whole day; `updated:>=2026-10-01` and `<`, `>`, `<=` comparisons work too.
`updated:2026-10-01..2026-10-11` includes both days; either end can be `*` or
empty. `after:` and `before:` exclude the named day. Invalid dates show a syntax
hint. The menu offers Today, Last 7 days, and Last 30 days, and edits the visible
query so constraints stay inspectable. Clear removes the whole query. The help
popover documents the syntax. Search covers loaded history; older sessions load automatically as you approach
the end of the list, including when filters leave the viewport underfilled.
Sessions stay current through the core's `uploader` events, with a read every
60 s as a safety net; there is no manual refresh or pagination control. Both platforms keep Filters open while you combine
choices; Done, Escape, or clicking outside dismisses it. Background refreshes
preserve the open menu and focused control. Group by closes after a selection.
Shared fixtures exercise both search matching and how menu choices edit the
query, including replacing source aliases and clearing date constraints.

History (`uploader.sessions.list`) joins local relay discovery (`sessions.list`)
by the core's upload key. A session appears once when both lists contain it.
The key is the harness and the session's own id (`codex:…`, `claude:…`,
`grok:…`, `opencode:…`, `muse:…`, `devin:…`), so a running Codex, Claude Code,
Grok, OpenCode, Muse or Devin session on the relay is the same row as its
transcript, and its transcript shows its real messaging state. Claude app,
OpenClaw, Hermes, Pi and Oh My Pi relay rows have no key and are listed on their own.
Discovered sessions awaiting upload indexing remain visible, labeled Indexing.
Ended registrations remain visible so they can be taken off the relay. No roster
records enter this table. Multi-machine and workspace browsing is
available in Cloud at `/cloud/dashboard/sessions`.

The table lists top-level sessions. Grok sessions explicitly marked as
subagents stay in local history but do not appear as separate rows, even when
Grok has not recorded their parent. The shared core applies this rule on both
macOS and Linux, including after refreshing previously cached sessions.
An independently selected Grok subagent is removed from the selection before
uploads resume after upgrading; a child with a recorded parent still shares
through that parent.

Two history layouts arrive with RelayHistory 0.45.0 on both apps. Grok Bot
(the Anysphere desktop app, distinct from the Grok CLI) is a `grok-bot` source:
each bot is one session read from the app's local transcript store, which keeps
roughly the last 200 entries or 7 days per bot, so only that tail is recovered.
A Codex thread whose rollout file Codex has pruned is recovered from Codex's own
SQLite history and keeps its replies, commands and file changes; its catalog row,
not a transcript file, says when it changed, so selected mode follows it the way
it follows a session in a shared database.

RelayHistory adds OpenClaw (an `openclaw` source) on both apps: each
OpenClaw agent's sessions (its main chat, Telegram and other channel chats,
Agent Relay conversations, cron and subagent runs) are read from under
`~/.openclaw` (or `OPENCLAW_STATE_DIR`), and follow the session-sharing mode
like every other source. RelayHistory 0.47.0 reads every OpenClaw from 2026.3:
the agent's `openclaw-agent.sqlite` from 2026.8.1, and the `sessions/*.jsonl`
transcripts with `sessions.json` before that. A session OpenClaw has imported
from its JSONL store into the database is read once. From RelayHistory 0.49.0
an OpenClaw session is one conversation key, as OpenClaw's own session list
shows it: every reset and every isolated cron or heartbeat run of that key is
part of it (one row per cron job, not one per run), and heartbeat polls are
not prompts. Session IDs from the agent database are `<agent>:<key>` (for
example `main:main`, `main:cron:<job>`).

RelayHistory 0.48.0 adds two sources on both apps, which follow the
session-sharing mode like every other source. Antigravity (an `antigravity`
source) reads each conversation under the App, CLI and IDE brain directories
(`~/.gemini/antigravity/brain`, `~/.gemini/antigravity-cli/brain`,
`~/.gemini/antigravity-ide/brain`), preferring a conversation's full transcript
over its truncated compact file; user and assistant text become messages and
every native step is kept as a marker (it records no tool-call IDs, model or
usage). Oh My Pi (an `omp` source) reads its sessions under
`~/.omp/agent/sessions` (or its profile and XDG data directories), with
messages, tool calls and results and observed usage; a session in a directory
Pi and Oh My Pi share is read once.

Grok Bot (`grok-bot`) sessions use their roster names and individual profile
avatars on macOS, Linux and Windows. RelayHistory owns these metadata reads; the desktop
renders recorded shapes/colors using Grok Bot's native idle animation, or a
locally recorded raster image. Reduced motion keeps the avatars still. Missing
or unsupported artwork falls back to the Grok Bot app icon. Each native chat
retains its own session row and UUID for identity.

Each row has a **Receives messages** orb and a read-only **Upload** status.
The Session column's second line leads with the agent's relay name once the
session is registered, then the observed model, harness name and reasoning
effort, then the session's working folder. The open transcript's header shows
the title on its own line; the second line names the harness, then the agent's
relay name (**@name**) while it receives messages, then an **Online**, **Connecting…** or
**Offline** badge (none while it is off the relay), as Cloud's session header
does. The working folder, or the project when no folder is known, is that
line's tooltip. Pi sessions and agents draw Pi's
block mark wherever harness marks appear.
Select rows with the left checkboxes to reveal bulk actions above the table.
The orb is gray when off/offline, yellow while registering or connecting, and
green only once the session is running and its push channel is live. Clicking
it opts into receiving messages or immediately shows offline while unregistering;
connecting can be cancelled. Hover and accessibility labels explain the state,
action, or why a session cannot receive messages. Receiving messages also enables
transcript upload under the core's existing policy, for every session with an
upload key.
Registered Grok CLI sessions join their local transcript and use Grok's title.

A terminal Claude Code session reads the relay messages delivered to it only
when its current turn ends. The core watches the end of the session's
transcript, and when a delivered message (a direct message or a subscribed
channel's posts, under any of a renamed agent's names) has sat unread with no
progress for 10 minutes (`RELAY_DESKTOP_CLAUDE_INBOX_STALL_SECS` changes it),
it adds a warning to Activity and reports the agent's `inbox_stall` (how many
messages wait, and for how long) in `agents.list`, and `inbox_stalled`,
`undrained_count` and `oldest_undrained_age_ms` on its `GET /agents` rows. The
warning clears once the session reads them; a transcript that cannot be read
for a while changes nothing. After the core restarts it recovers what to watch
from each running session's transcript: messages enqueued after the session's
last progress. Delivery itself is unchanged. The same on Linux and Windows.

Bulk upload requests update optimistically per session and keep other rows usable.
Repeated clicks are serialized per session and settle on the latest choice;
different sessions proceed independently. Failed changes roll back only the
affected row and show its error. Listings from before a selection change are
ignored and refreshed. Signing out discards pending UI state and late replies.
Deleting cloud copies requires confirmation and stops future uploads while preserving local history.

Every other control that changes the core follows the same pattern, on both
platforms: the intent is recorded in a pending slot before the call, the view
renders from it, the core's answer (or the status read after it) replaces it,
and a failure reverts to what the core reports and shows the existing per-row
error. A newer request supersedes an older one (the last click wins), so an
older answer changes nothing. This covers the upload policy (the chosen row
moves and stays usable, with a small spinner), pause and resume, retry and
compact (the stage reads *Retrying uploads…* / *Compacting the upload
journal…*), core settings switches (the shown settings are the confirmed ones
with pending changes written over them), renaming an agent and registering a
session under a name (a provisional registration without an address), sending a
message (an echo under *You* with the caption *Sending…* until the history lists
it; the box clears at once and is restored on failure), Incoming and Claude
delivery changes, launch at login and, on macOS, contractor seat dismissal and grant revocation. Sign-in
and log-out carry a starting and a disconnecting state on both platforms.

Before sign-in the window matches agentrelay.com/login: a dark ribbon-art panel
on the left and a dark form on the right with the Agent Relay lockup, white
**Continue with Google** and **Continue with Microsoft** controls, a dashed
separator above **Works with all your agents**, and the website’s eight-agent
strip with the Antigravity mark. Footer links are omitted. Both native apps
draw moving ribbons and travelling pulses locally, pause for reduced motion, and
share the same agent-strip artwork. Startup,
browser approval and workspace selection remain within that welcome layout.
During startup, **Rounding up your agents** stays on one line beside compact pulsing bars at the content's right edge.
The subtitle rotates every four seconds (unless reduced motion is enabled),
starting with a randomly chosen message. Sign-in appears as soon as
the service supplies its status; session discovery runs in the background.
After 20 seconds without a ready service, the panel offers **Try again** and
**Copy diagnostics** instead of continuing the animation.
The sign-in panel has no tool-connection checkbox. Each provider button opens
its own Cloud OAuth route with the same PKCE loopback handoff and requested
workspace. Sign-in does not wait for the tool-settings read. Provider setup
remains available after sign-in, and existing integration choices are preserved
on macOS, Linux and Windows.
Signing in shares nothing by itself: the install starts in **Only sessions I
choose**, so a session uploads once its Upload switch is turned on, and keeps
uploading as it changes. Automatic uploads of new sessions, or of all past and
future ones, are chosen in Settings › Session sharing.

Devin Cloud sessions started by an automation (including Devin reviews) are
excluded from session discovery and automatic relay activation on macOS, Linux
and Windows. An automation classification in any live install's catalog wins
when catalogs disagree. Previously registered reviews are also omitted from
session listings; their stored registrations remain available for removal.
A Devin Cloud session with no activity in the last 24 hours is not
auto-activated, and an automatically activated one leaves the relay after the
usual grace period, since nobody is in it to talk to and each agent on the
relay heartbeats Relaycast. It is still listed, and a person can add it by
hand; a message to it still posts into it.

## Settings

Settings is grouped by task: Preferences (General, including Appearance), Sessions
(Opening sessions, Session sharing, Messages), and Account & app
(Account, Updates, Diagnostics). Account contains only profile and
workspace details; log files and expandable diagnostic reports live in Diagnostics. Session deletion lives below the sharing policy and upload details.
Update availability uses a compact sidebar dot; the update action uses an explicit
white-on-deep-blue foreground/background pair. Settings use a 720-point reading
column, wrapping descriptions, reserved action widths, and quiet outline icons.
The avatar carries a small update indicator; the menu links directly to Updates.
Updates includes **Include beta releases**, off by default on macOS, Linux and Windows.
Enabling it selects the newest numbered beta or stable GitHub release by
version; stable users continue to read `/releases/latest`. Changing channels
clears pending release information and checks immediately. Disabling beta
keeps the current installation until a newer stable version is available.
Beta builds publish as `vYYYY.MM.N-beta.NUMBER` GitHub prereleases; the final
`vYYYY.MM.N` stable build updates both audiences. Release titles and notes
provide Stable/Beta labels and GitHub prerelease filter links.
Detailed upload stages and journal compaction live under **Session sharing →
Upload details**, keeping the startup view focused on session selection. When a
problem that is not retried on its own stops uploads, session rows read **Upload
stopped** (not **Waiting to retry**) and the upload stage offers **Retry uploads**. Uploads
that keep failing on a server error or a dropped connection read **Waiting to
retry** while they wait out a backoff of at most five minutes, and the upload
stage offers **Retry uploads** too, to try again at once; a rate limit only
waits. A Retry or Compact that could not reach the background service is sent
on its own once the service answers again, and its "reconnecting" message
clears without a click.

General › Startup holds **Open at login** and **Keep agents connected after
quitting**. Open at login controls both the menu-bar or tray app and the upload
service: on starts both at login, off schedules neither. On Linux and Windows
the app changes the systemd unit or logon task registration without stopping
current uploads; starting the headless core preserves the GUI login entry.
On macOS, startup off uses an explicitly bootstrapped session job whose plist
lives outside `~/Library/LaunchAgents`, so manually opening the app still runs
the core without scheduling it at the next login. Startup on restores the
persistent LaunchAgent. Failed changes restore the previous registration where
possible and show an error; GTK serializes rapid changes so the last choice wins.
On upgrade, GTK preserves either existing startup opt-in and brings the GUI
entry and installed service into agreement once, without stopping uploads.
If migration fails, the switch still shows an enabled service as on and a
visible mismatch offers repair; refreshes never retry indefinitely. An explicit
choice supersedes migration. When no service manager/unit is available, the
GUI login entry remains usable and opening the app starts its hosted upload
core. An unreadable registration (such as a timed-out scheduler) is reported as
an error instead of being mistaken for an absent service.
Keep agents connected after quitting is on by default and leaves the current
session's core running. Off stops it when the app quits, while preserving the
Open at login choice for the next login.
The Mac's Quit control reads "Quit Agent Relay (agents stay connected)" while
keeping agents connected is on. Quitting while signed in with it off asks
**Quit and take your agents offline?**: **Keep Agents Connected** turns the
setting back on and quits, **Quit Anyway** quits and stops the service, and
**Cancel** stays open. An update's restart and a logout or shutdown quit
without asking.

General › Session history holds **Keep session history**
(`history_retention_days`): 7, 30 or 90 days, or Forever (`0`), 30 unless
changed. RelayHistory keeps the policy in the signed-in install's own store
(`history.retention_days` in its `history.db`). The core writes a choice there
and reads the store's value back every 30 seconds, on a thread of its own
(`relay::history_retention`). A store in step with this choice (the same
database, by path and change-feed epoch) gives its value, so a choice another
client saved shows here; a store not yet in step (a workspace signed in to
later, a store a session reset recreated) gets this computer's choice; and a
choice not yet saved in a store (made while signed out, or a write that
failed) stays pending and is written again rather than read over. The core
also keeps the choice in `history-retention.json` and in memory, so neither
Settings nor the session list waits on the database. The collector, between delivery passes
(`desktop-core`'s `history::retention`), publishes how far uploads were durably
acknowledged as the store's retention consumer and, once this computer's
choice is saved in that very store (`history_retention::applied_to`), lets
RelayHistory evict expired history in small batches: a session is never evicted before it
uploads, nothing is evicted before the first acknowledgement, and eviction
leaves no deletion for the uploader to send, so uploaded copies stay. A choice
shorter than 30 days, or any limit while signed out or uploads are paused,
notes that unuploaded sessions are kept until delivery succeeds.

Relay settings retain workspace connection, Claude Code direct delivery, Grok
leader mode, OpenCode plugin configuration, **OpenClaw agents**
(`openclaw_gateway`; with an OpenClaw older than 2026.8.1, which cannot limit
relay sessions, the row says so: `openclaw_unlimited_version`), **Agent Relay
tools** under it while it is on (Connect signs OpenClaw in to the hosted Agent
Relay MCP in the browser, with **Open Again** while it waits;
`openclaw.relay_tools.*`), **Trusted senders** (`trusted_relay_senders`:
relay names or addresses whose messages reach the person's own OpenClaw and
Hermes; above the field, **Trust people in your workspace**
(`trust_workspace_members`, on by default) lists the workspace's people
read-only, each labeled **Workspace member**, and the field is headed **Other
names and agents**; unfinished names stay in place when the switch or member
list changes), and **Hermes** (`hermes_agent`), both on wherever the
agent is set up and listed only while its local gateway answers
([openclaw-hermes-delivery.md](openclaw-hermes-delivery.md)), the Devin lifecycle hooks,
**Cursor sessions** (`cursor_direct_delivery`, shown where Cursor CLI or the
Cursor editor's agent has run, off unless turned on: **Turn On** adds Agent
Relay's `postToolUse`, `stop` and `beforeSubmitPrompt` hooks to
`~/.cursor/hooks.json`, so a Cursor editor or Cursor CLI session gets its
messages at its next tool call or when its turn ends, and a Cursor editor
chat that has finished its turn gets them as soon as they arrive; the wording is
`CursorHooks` on macOS and `CURSOR_HOOKS_*` on Linux and Windows, where the
core reports it unavailable; [cursor-copilot-existing-session-injection.md](cursor-copilot-existing-session-injection.md)),
**Show me relay messages in full** (`show_relay_messages`, off by
default: relay messages are hidden from Codex app chats; a terminal Codex or
Claude Code session, which only accepts visible text, shows a two-line pointer
naming the sender and the command the agent runs to read the message; the
agent still gets each message in full and can reply: the setting only
changes what the person sees), **New agents join
#general** (`auto_join_general`, off by default; applies to agents registered
from then on), **Changed by agents** (shown once an agent has changed a
setting through `PATCH /settings`: the latest five, newest first, each with
the agent, the setting, its values before and after, and the time; an agent
cannot turn on or add to a setting that widens who can reach or act as the
person, `person_only_setting`), and
incoming-message settings. With a signed-in Devin CLI the session inventory
additionally polls the org's Devin cloud sessions via the `devin-cloud`
connector (cadence-gated, five minutes) so remote sessions list beside local
ones; a connector failure surfaces as a diagnostic and never fails local
discovery. Those cloud sessions also appear in the sessions table, hosted
by *Devin Cloud*, and one auto-activates like a local session while the
sign-in resolves and the session was active in the last 24 hours (an older
one is still listed and can be added by hand, and an auto-activated one
leaves the relay after the usual grace period) — a message to it is
posted into the session through the same connector's send route. With a
Cursor API key (`CURSOR_API_KEY`, or the first line of
`~/.config/relayhistory/cursor-api-key`, which the background service
reads without a shell environment) the inventory also
lists Cursor Cloud Agents through the `cursor-cloud` connector on the same
schedule. They keep source `cursor` with their `bc-` agent ids, so they never
collide with local Cursor sessions, and only those ids are hydrated through
the connector: a local Cursor session never becomes a cloud placeholder.
Transcripts carry user and assistant text only, timed per run.

The shared core's peer-message note allows work already authorized in the
receiving session to continue within the harness's permissions. A peer message
does not grant new approval or authorize permission/configuration changes. This wording
is shared by hidden Codex messages and terminal `read` on all platforms;
Claude's native peer-message restrictions remain in force.
Claude Code cloud sessions on claude.ai are listed and uploaded the same way
(and, when active in the last seven days, listed as `claude-cloud` agents
hosted by `claude-web`, delivered with `claude --print --cloud` and answering
through the hosted MCP; see `integrations/claude-code-web`)
once the person turns on **Claude Code cloud sessions** in *Claude Code
folders* (`claude_cloud_sessions`, off by default; an agent may turn it off
but not on), through relayhistory's `claude-web` connector, with the
claude.ai login of Claude Code in `~/.claude`: `~/.claude/.credentials.json`
while its token is current, otherwise (macOS) Claude Code's
`Claude Code-credentials` login Keychain item, handed to the connector as a
private 0600 file under the install's data folder for the length of one pass.
macOS's prompt to read that item appears only when the person acts: turning
the setting on (the row explains it and asks for **Always Allow**) or opening a
cloud session. Capture passes read the item quietly — with Keychain user
interaction off, so they never prompt — on a thread of their own, waiting at
most 3 seconds; without access Claude is reported unavailable with the
Keychain OSStatus and a fix (`-25337` says to turn the setting off and on and
choose **Always Allow**), every other source carries on, and the next pass
reads again. Settings shows the source as **Working**, **Checking**,
**Blocked**, or **Needs attention**, the plain cause and fix, and the last
successful pass. Each failed pass and helper failure is also written to the
credential-redacted `core.log`. The credential is never
refreshed or changed; an expired one is not used (*Claude Code's sign-in has
expired; open Claude Code to refresh it*). Remote Control sessions that mirror
a local terminal are skipped (their local transcript is uploaded). Each
connector fails on its own: a Claude failure never stops Devin or local
discovery, is recorded per connector in `remote-diagnostic.json`, and the
connector is left alone until its next listing is due. Sharing **New sessions**
withholds a cloud session first listed after setup that started before it;
**Selected** uploads only the chosen ones. The cutoff is the install's first
upload job, so changing one session's sharing (which replaces the job) does not
move it. A cloud session goes with the `~/.claude` folder: turning that folder
off in **Claude Code folders** stops listing and uploading them, and while
`~/.claude` is not a folder listed there (Claude Code runs from another
`CLAUDE_CONFIG_DIR`) cloud sessions are neither listed nor uploaded.
Cloud history is downloaded at a pace that leaves the machine and the
provider account alone (one person's 256 claude.ai sessions came to about
330 MB): only sessions the install will upload are downloaded, newest first,
at most 20 or one minute per pass. A throttled or failing provider
(HTTP 429, 5xx) is left alone for a wait that doubles up to an hour; a
failing session waits up to a day, and one that cannot be read (denied, too
large, gone) is parked until the provider lists it as changed. Downloads stop
while the disk has less than 2 GB free or once they have grown the database
by 2 GB; `remote-diagnostic.json` says so under `pacing`, with how many
sessions are still waiting. The state is `history.remote-pacing` beside the
database.
Account settings retain identity and sign-out. Under **Workspace**, when Cloud
reports the workspace's Agent Relay plan, a plan row names it (**Intro plan**)
with its caps against what the workspace uses as far as the core has counted
(**12 of 15 agents · 1 of 2 humans · up to 5 devices · 7-day session
history**; a cap not counted yet reads "up to", no cap reads "unlimited") and
**Manage plan**, which opens the plan's own link (Cloud Billing when Cloud sent
none). A core or Cloud that reports no plan shows no row. General includes Appearance with Match system,
Light, and Dark. General only shows background-service controls when approval or
repair is needed; it never offers to stop the service. Message links and menu-bar/tray controls continue to work.

Messages settings also show **Claude Code folders**, with why each is watched:
the default, a sibling `~/.claude-<name>` login, one named in
`AGENT_RELAY_CLAUDE_CONFIG_DIRS` (colon-separated), one added with **Add Folder…**,
or one a running Claude Code session uses through `CLAUDE_CONFIG_DIR` (such as
`~/.seclaude`), which is added when seen. Each has a Shared switch. Turning a
folder off (`settings.set` with `claude_config_dir: {path, shared: false}`)
keeps its sessions off the relay and out of uploads (for the `~/.claude`
folder, its claude.ai cloud sessions too); choices, and the **Claude Code
cloud sessions** opt-in below the folders, are saved in
`~/.agentworkforce/relay/claude-config-dirs.json`.

Wherever a Claude login
directory exists, Messages includes a *Ask Claude chats to join* row that adds or removes Agent Relay's
SessionStart hook, on by default once after sign-in (`settings.set` with
`claude_session_hook`). An explicit saved Off choice stays off.
The hook is written into each Claude login's `settings.json` the desktop
knows about. When a session is not on the relay, the hook has Claude ask the
person whether to put it on the relay and under what name, and POST /register
only if they say yes. The person sees a short notice that nothing is
registered unless they say yes. The hook never registers the session. A
`claude -p`, Agent SDK or structured pipe run has nobody to answer, so the hook
stays quiet there. A session already on the relay gets a short preamble with
its relay name, how to reach peers and the messaging norm.

Messages settings also have **Spawned agents** on both apps (see
[native-spawn.md](native-spawn.md)). **Accept spawns from Agent Relay**
(`settings.set` with `accept_spawns`, on by default) makes this computer a
spawn target, `<machine prefix>-desktop`, that starts Codex, Claude, OpenCode, Grok, Muse and Devin headless
when their CLIs are installed. Devin runs on a
persistent private ACP host and can take another message while idle. Its full
access switch defaults off. A launch-specific MCP server permits only Agent Relay
sends through the authenticated local session socket; other approval requests are refused. The bundled
Devin app CLI still needs its own authenticated CLI session (`devin auth status`).
Both apps use the shared core's durable spawn failure receipts and redacted
startup diagnostics. Hosted status keeps the same reason across broker versions
and restarts; the core log also records failures without an activity subscriber.
A status line under it says whether Agent Relay can spawn here, is
connecting, or why not, and how many places are in use; it is a warning when
another account owns this computer's spawn node, which then takes no spawns
(see [native-spawn.md](native-spawn.md#whose-computer-it-is)). While it is on,
**Allow teammates to spawn here** (`spawn_allow_teammates`, off by default)
reads "Off: only you can start agents on this computer. On: workspace owners
and admins can too.", with the claim under it ("Claimed by you in Agent Relay
Cloud." or "Not claimed yet: no agent can be spawned on this computer through
Agent Relay Cloud until it is." with why, and a primary **Retry** button when
the claim failed), and
**Max spawned agents** is an optional number field (1–32). Empty means
**No limit**, the default; `spawn_max_agents: 0` clears the cap through the API.
The old untouched default of 12 was stored as absent and now becomes unlimited;
explicit caps, including 12, are preserved. When a cap is set, it limits how many
run at once: an idle agent keeps its place until it is released, and a spawn
beyond the cap is refused, never queued. Save with **Save** or Enter; leaving the field or refreshing Settings does not apply an unfinished edit. Failed saves show the error and restore the confirmed cap. A **full access** switch on each provider’s settings page, for each
harness the node offers, chooses whether spawned agents run without their
sandbox; Codex and Claude approval requests go to the machine owner alone, as Relay
messages they answer. The rows, their labels and wording come from
the core (`SpawnStatus.harness_descriptors`), so both apps show the same ones;
each is `spawn_harnesses.<id>.full_access`, on by default for Codex, Claude, OpenCode and Grok
(also `spawn_full_access_codex` and `spawn_full_access_claude`; OpenCode has the
map key only). Billing and repository access are set with `settings.set` only
([node-lifecycle.md](node-lifecycle.md)).

A spawned agent's first message names the agent that asked for it (as
Relaycast recorded the invocation) and gives it this computer's session-socket
commands to report back and list agents, with no token in the message. A
spawned Claude or Codex that sends its requester nothing gets its first turn's
final answer sent to them, only while that name still belongs to the agent
that asked; other harnesses are told the requester receives only what they
send. A confined OpenCode, which may not run shell commands, gets no commands
([native-spawn.md](native-spawn.md#reporting-back-to-the-requester)).

Spawned OpenCode agents can reply through this computer's session socket: the
spawn loads Agent Relay's OpenCode plugin into their private server itself,
whether or not the person installed it globally, without touching OpenCode's
own configuration directory. The plugin only tags shells there; the core verifies the exact private server and
sealed session binding before accepting a reply. On macOS the reply hint uses
the Agent Relay helper so the kernel can read the shell's session tag.

Native layout review fixtures (test identity and simulated update):
[Sessions on macOS](images/settings-redesign/sessions-dark.png),
[Sessions on Linux](images/settings-redesign/sessions-linux-dark.png),
[General](images/settings-redesign/general-dark.png),
[Account](images/settings-redesign/account-dark.png),
[Updates at 900 points](images/settings-redesign/updates-dark.png), and
[Opening sessions in light mode](images/settings-redesign/opening-light.png).
Regenerate with `SETTINGS_RENDER_DIR=/tmp/relay-settings-renders swift test --package-path macos --filter SettingsLayoutTests`.

Messages settings also have **Claude Code message approvals** on both apps,
collapsed by default. Expand the section to manage session and member permissions.
Each Claude Code session can use direct delivery or **Ask first**. An optional
member restriction lets only explicitly allowed sender addresses arrive
directly; every other message is held locally with **Approve once**, **Always
allow** and **Dismiss** actions. These choices are enforced before the
session's inbox and are separate from Claude Code's login-wide
`crossSessionInbound` setting.
When Claude Code itself will hold a delivered message for approval, Recent
Messages shows it with a clock (orange on macOS, the warning style on GTK) and
**Held for approval in Claude Code: …**, and the composer's note under a
message the person sent reads **Held for approval in Claude Code**, on both
apps.


Codex app-server and desktop-window messages stay pending while the current
turn runs. After it finishes, delivery starts a separate turn; peer messages
and incoming cloud messages use the same idle check on macOS and Linux.
Waiting does not consume a send attempt. Live terminal sessions continue to
use Codex's own queue, which waits for the current turn to finish. Conductor's
existing Stop-hook delivery remains a separate host-specific route.

With **Settings › Relay › Codex mid-turn delivery** on (`codex_stop_hook`, on by
default wherever Codex is installed, on macOS, Linux and Windows alike), a
message for a terminal or app Codex session that is running a turn does not
wait for it: Agent Relay's Codex hook hands it over at the turn's next tool
call or as the turn ends, and the activity shows it waiting for the turn until
then. A turn that ends without the hook taking it sends the message through the
routes above on the next pass. See [native delivery](native-delivery-spec.md),
section 3.1.2.

The shared probe recovers a rotated Cloud workspace key automatically when
Relaycast rejects it. It refreshes the app's retained Cloud session, resolves the
same workspace and origin, and verifies the replacement with one safe read before
saving it. Sign-out or a changed workspace cancels recovery; failed writes are never
replayed automatically. This behavior is shared by macOS and Linux.

## Interaction

The transcript replaces list-level search with its own back/refresh
controls. Search, switches, keyboard focus and scrolling remain native. Filters and
grouping use compact Cloud-style triggers with checkmarked popover choices. Disabled relay switches explain whether the session
must be started or a workspace connected. Unsupported upload rows explain that
no local transcript is available. Global errors remain available in Diagnostics;
per-session relay errors appear beside the affected title.

Agents on the relay show their harness mark. A Capy thread (registered with
`session_kind: capy` by Agent Relay Cloud when the thread binds) shows the Capy mark, is
listed as running in the cloud, and is grouped under a Capy machine.

Starting Capy threads is an agent operation; neither desktop app offers it.
The relay core retains the Capy project, spawn and binding-status methods for
agents; the workspace Capy key remains in Cloud.

## Other platforms

On macOS the main window hides the native title and title-bar background, letting
the page backgrounds extend behind the standard traffic-light controls. Content
keeps its native safe area so controls remain clear. Linux and Windows retain
the native title bar and controls; the macOS window style is platform-specific.

What a build talks to and is called is fixed when it is built, on every
platform; nothing at run time changes it. On macOS `scripts/build-app.sh`
takes the profile as options: `--site-url` (the Agent Relay site, default
`https://agentrelay.com`), `--url-scheme` (the scheme of the links it opens,
default `agentrelay`), `--product-name` (default `Agent Relay`), `--bundle-id`
(default `com.agentrelay.desktop`; the core's LaunchAgent label `<id>.core`,
socket and data directory follow it, so builds with different ids run side by
side), `--releases-repository` (the GitHub repository checked for updates,
default `AgentWorkforce/relay-desktop-releases`) and `--icon` (an SVG or
1024×1024 PNG in place of the repository's mark). `--dev-site-url` is the
shorthand for Agent Relay Dev: a loopback site with the `agentrelay-dev`
scheme, name and bundle id. The bundle records the site, scheme and repository
in `Contents/Resources`, which `AppInfo.swift` reads; the probe in `--probe`
must be built for the same site and scheme (`scripts/build-probe.sh`). Only a
loopback site makes a development build (no launch at login, the green menu
bar mark); a build for any other site is a release build like production's.
The sign-in screen's **Development** badge, on every platform, names the site
of a core for any site but production (the core's reported site, else the
build's own), and a refused link is worded the same on every platform
(`AppModel.linkRefusal`, `models::link_refusal`). A build for another site
must name its own `--bundle-id`, `--url-scheme` and `--releases-repository`
(the script refuses production's, which would share the production app's
LaunchAgent, socket and data directory, or install production over the build
through an update), and `AppUpdateInstaller` accepts only an app with the
running bundle's name and identifier. Linux and Windows take the site,
scheme, product name and releases repository through `scripts/build-gtk.sh`
(or `build-windows.sh`) and `scripts/build-probe.sh`; `gtk/build.rs` bakes
the repository, the asset prefix (`Desktop-<id>`, from `--profile`) and the
product name into the app (`gtk/src/build_profile.rs`), so their updater,
like the Mac's, reads the profile's repository and installs only release
images named with the profile's prefix (`gtk/src/updates/channels.rs`).

Linux and Windows provide the same local session table and Settings navigation
in GTK 4 (`gtk/src/ui`, one app built for both). Every platform
uses a 980 × 650 minimum window. Appearance is persisted in native preferences
on macOS, `~/.config/agent-relay/desktop.json` on Linux and
`%APPDATA%\agent-relay\desktop.json` on Windows.

A per-user Linux install (`~/.local/lib/agent-relay/<version>`) lists itself
in the desktop's app launcher through
`~/.local/share/applications/agent-relay.desktop`, which runs that install's
`agent-relay-desktop` by its full path; the app is never installed as
`agent-relay`, the npm Agent Relay CLI's name. Updates and every app start
keep it, and launch at login when it is on, on the running
version, `agent-relay-desktop --install-launcher` writes it for a scripted
install, and an install from the `.deb` removes it so the package's own entry
is listed. An entry the app did not write is kept. macOS needs none: the app
is a bundle in `/Applications`.

A per-user Windows install
(`%LOCALAPPDATA%\Programs\AgentRelay\<version>\AgentRelay-Windows-<arch>`)
lists itself in the Start menu through
`%APPDATA%\Microsoft\Windows\Start Menu\Programs\Agent Relay.lnk` and handles
`agentrelay://` links through `HKEY_CURRENT_USER\Software\Classes\agentrelay`,
both kept on the running version the same way;
`agent-relay.exe --install-launcher` writes the shortcut for a scripted
install. The release's `AgentRelay-Windows-<arch>-Setup.exe` installs into
that same folder, writes the same shortcut and runs `--install-launcher`,
and its uninstaller removes every version, the shortcut, the handler and the
logon task. A Windows PC or server without the app runs the bare core from
`windows/install.ps1` instead: `%LOCALAPPDATA%\Programs\AgentRelay\core\versions\<version>`
with a `current` junction, as the same **Agent Relay (<account>)** logon task
(`agent-relay-probe.exe serve --headless`), the counterpart of
`linux/install.sh` and its `~/.local/lib/agent-relay` layout; the app takes
the task over when it is installed later, and its uninstaller removes that
core with the rest. A coding agent reaches the setup routes there through
`agent-relay-probe.exe socket-request`, the named pipe's only client
([agent-driven setup](agent-driven-setup.md#scriptable-windows-installation)).

### macOS and GTK counterparts

A user-facing change on one platform makes the matching change here on the
other in the same commit:

| macOS | Linux and Windows |
|---|---|
| `macos/Sources/RelayDesktop/WorkspaceShell.swift`, `Navigation.swift` | `gtk/src/ui/shell.rs` |
| `RelayModel.swift` | `gtk/src/ui/relay.rs` |
| `MainWindowView.swift` (sessions), `SessionsTable.swift`, `UploadStagesView.swift` | `gtk/src/ui/sessions_page.rs` |
| `MessageSheet.swift` | `gtk/src/ui/messages.rs` |
| `SettingsView.swift`, `AboutView.swift` | `gtk/src/ui/settings_page.rs`; About's build identity (`BuildIdentitySection`): `gtk/src/ui/settings_build_identity.rs` |
| `SignInView.swift`, `AppModel.swift` (sign-in, links) | `gtk/src/ui/app.rs`, `gtk/src/ui/app/links.rs` |
| `MenuPanelView.swift` | `gtk/src/ui/tray.rs` |
| `BrandMark.swift` (`MenuBarLabel`, the menu-bar icon's profile badge) | `gtk/src/ui/tray/badge.rs` (the tray icon's profile dot) |
| `UIZoom.swift`, View menu zoom in `RelayDesktopApp.swift`, `ZoomSettingsRow` in `SettingsView.swift` | `gtk/src/ui/zoom.rs`, `UiZoom` in `gtk/src/prefs.rs`, `css_at` in `gtk/src/ui/theme.rs` |
| `AppUpdater.swift`, `RelayDesktopCore/AppUpdate.swift` | `gtk/src/updates.rs` and About in `ui/settings_page.rs` |
| `AppInfo.swift` (site profile: product name, mark), `BrandMark.swift` | `gtk/src/build_profile.rs`, `gtk/src/ui/widgets/assets.rs` (`brand`, `wordmark`, `header_brand`) |
| `RelayDesktopCore/*` presentation logic (presence, upload states and stages, wording, filters, diagnostics) and `shared/RelaySessionOverview` | `gtk/src/models/`, and the core's `src/session_model/` for what the core's catalog query also uses |

Wording matches the Mac's word for word unless the platform differs (menu bar
vs tray, LaunchAgent vs systemd user service vs logon task, DMG vs
tarball/`.deb` vs zip).

## Site profiles

A Desktop build's Cloud site, link scheme and branding (product name,
bundle identifier, mark, support link, update feed, telemetry) are
fixed when it is built, from a **site profile**. The default profile,
`agentrelay`, is the repository itself: every value below defaults to it,
and a build or release given no profile is the default one, as before. A
customer's profile is a GitHub Environment on this repository named
`desktop-<id>`, where `<id>` is an opaque id (lowercase letters and digits,
at most 40, with single hyphens between them and none at either end), whose secrets carry the values; nothing
of the customer is committed here. Desktop Release and Desktop Promote take
the id as their `profile` input and run every job that builds or publishes
under that environment ([.github/README.md](../.github/README.md#site-profiles)).
Outside an artifact's own contents the id is the profile's only name: the
release log says `Profile <id>`, the candidate manifest records
`"profile": "<id>"`, and every artifact and release asset is named
`Desktop-<id>-<version>-<platform>-<arch>.*` where the default profile's is
`AgentRelay-<version>-...` (`asset_prefix` in `scripts/site-origin.sh`); the
Mac app records its prefix in `Resources/asset-prefix.txt` (`build-app.sh
--asset-prefix`) and looks for its own images when it checks for updates. A
profile's release notes are the `## Unreleased` section of `CHANGELOG.md`: its
version numbers are its own repository's, so a `## v<version>` section here
is another release's notes. The release title is the version alone. The site-profile action
(`.github/actions/site-profile`) checks the values, masks what it derives
from them and exports them for the steps; the product name is never written into a
command line or a log line. A profile is never the default product: the action
refuses a profile whose releases repository, site, link scheme or bundle
identifier is the default profile's (the `DEFAULT_*` values in
`scripts/site-origin.sh`; repositories and bundle identifiers compared without
case), so a profile's release can never publish into the public feed.

The profile's values are environment **secrets** (`secrets`), not variables:
the runner prints a composite action's inputs and a step's `env` before the
step runs and masks only secrets from the start of a job, so a value kept in
a variable would be in the run log. Create each with `gh secret set <NAME>
--env desktop-<id>`.

| Secret | Meaning | Default profile | Consumed by |
|---|---|---|---|
| `AGENT_RELAY_SITE_URL` | The Cloud site the core signs in to, an `https` origin | `https://agentrelay.com` | `build-probe.sh`, `build-gtk.sh`, `build-linux.sh`, `build-windows.sh`, `package-linux-probe.sh` (`--site-url`), `build-app.sh`; the core's build (`AGENT_RELAY_SITE_URL`) |
| `AGENT_RELAY_URL_SCHEME` | The scheme of the links the apps open and register handlers for | `agentrelay` | the same scripts (`--url-scheme`); the Linux launcher entry's `MimeType` and the installer's `ProtocolKey` are generated from it |
| `DESKTOP_PRODUCT_NAME` | The product name the apps, launcher entry, Windows installer and executable properties show. It names the Windows desktop shortcut and the installer, so it may not contain `: * ? < > \| { } ,`. | `Agent Relay` | `build-gtk.sh`, `build-linux.sh`, `build-windows.sh`, `package-macos-probe.sh`, `build-app.sh` (`--product-name`); `gtk/build.rs` and `desktop-core/windows_resources.rs` (`DESKTOP_PRODUCT_NAME`); the Mac app reads it from `CFBundleDisplayName` (`AppInfo.displayName`). The window title, the brand's accessibility label and, for any name but the default's, the wordmark's place in text say it |
| `DESKTOP_BUNDLE_ID` | The macOS bundle identifier, reverse-DNS | `com.agentrelay.desktop` | `build-app.sh` (`--bundle-id`). Reserved as the base of the Windows AppUserModelId and the Linux desktop entry id: the core names both (`install::windows::APP_USER_MODEL_ID`, `install::LAUNCHER_ENTRY`) for every build until it takes a build-time identifier, so they stay the default's |
| `DESKTOP_COEXIST` | Optional, and the one environment **variable** here, not a secret (`gh variable set DESKTOP_COEXIST --env desktop-<id>`): a secret whose value is `true` would mask every `true` in the job, and the runner drops a job output it masks. Only the jobs that build or package an app pass it. `true` builds the Mac app to coexist with Agent Relay: no `AgentRelayProduct` marker, so it neither replaces an installed Agent Relay or other product nor is replaced by one, and runs its own core (socket `core.<bundle id>.sock`, data directory `<bundle id>`, LaunchAgent `<bundle id>.core`); see Install identity below. For an internal or dev profile. `false` or unset keeps the takeover; any other value is refused, and so is setting it for the default profile | not set (the default profile always takes over) | the `site-profile` action (exported as `COEXIST`, `yes` or empty); `build-app.sh` (`--coexist`) in the Desktop Release macOS build. GTK has no product takeover and ignores it |
| `DESKTOP_COBRAND_NAME` | Optional. The customer a self-hosted site profile's app is made for, one line of at most 80 characters with no control characters (trimmed). With a `cobrand.svg` or `cobrand.png` in the branding archive (`DESKTOP_BRANDING_URL`), the macOS sign-in window, the signed-out menu and the installer image say "for <name>" with the customer's mark under the Agent Relay lockup; the product stays Agent Relay, and a build named Agent Relay draws the wordmark without its profile badge. The name and the mark need each other: either alone is refused. Refused for the default profile, the hosted app, which is never co-branded. Never printed | not set (no co-brand) | the `site-profile` action (masked, exported as `COBRAND_NAME`); `build-app.sh` and `package-dmg.sh` (`--cobrand-name`, with `--cobrand-mark`) in the Desktop Release macOS build, which record `Resources/cobrand-name.txt` and `Resources/cobrand.svg` or `cobrand.png` (`AppInfo.coBrand`) and draw the line in the disk image's background. Linux and Windows do not draw it yet |
| `RELEASES_REPOSITORY` | The `owner/name` GitHub repository the profile's releases publish to; every platform's updater reads it, fixed when the app is built. Required for a `github:` feed (and not the default profile's); must be unset for a `cloud` feed | `AgentWorkforce/relay-desktop-releases` | the Resolve version, publish and promote jobs (`RELEASES_REPOSITORY`); `build-gtk.sh`, `build-windows.sh`, `build-app.sh` (`--releases-repository`); `gtk/build.rs` (`RELEASES_REPOSITORY`); `release-notes.py --repository` |
| `DESKTOP_SUPPORT_URL` | Optional. The support link: today only the Windows installer's `AppSupportURL` carries one; reserved for the apps' help links, which do not exist yet | the releases repository's page; for a `cloud` feed, `<site>/cloud/dashboard/downloads` | `build-windows.sh` (`--support-url`) |
| `RELEASE_FEED` | Optional. Where the profile's updater reads releases, fixed when the app is built: `github:<owner>/<name>`, which must name `RELEASES_REPOSITORY` (unset means `github:` and `RELEASES_REPOSITORY`), or `cloud`, the profile's own site, which needs `DESKTOP_FEED_PUBLIC_KEY` and `DESKTOP_FEED_PUBLISHER_TOKEN` and takes no `RELEASES_REPOSITORY`. Unset builds what a profile built before there was a feed. A `cloud` feed is published to through the publisher bridge (see Publishing to a cloud feed below) | `github:AgentWorkforce/relay-desktop-releases` | the `site-profile` action (exported as `RELEASE_FEED`; the version, publish and promote jobs take their cloud branch on `cloud`); `build-gtk.sh`, `build-windows.sh`, `build-app.sh` (`--release-feed`); `gtk/build.rs` (`RELEASE_FEED`, `build_profile::release_feed()`); the Mac app's `Resources/release-feed.txt` (`AppInfo.releaseFeed`) and the Windows installer's `/DReleaseFeed`, written only when not the default's |
| `DESKTOP_FEED_PUBLIC_KEY` | The Ed25519 public key, the base64 of 32 bytes, a `cloud` feed's release records are signed with. Required for a `cloud` feed, refused for a `github:` one | not set | the `site-profile` action (masked, exported as `FEED_PUBLIC_KEY`); `build-gtk.sh`, `build-windows.sh`, `build-app.sh` (`--feed-public-key`); `gtk/build.rs` (`DESKTOP_FEED_PUBLIC_KEY`, `build_profile::feed_public_key()`); the Mac app's `Resources/feed-public-key.txt` (`AppInfo.feedPublicKey`) |
| `DESKTOP_FEED_PUBLISHER_TOKEN` | The token the publisher bridge signs in to the profile's site with to list and publish a `cloud` feed. Required for a `cloud` feed, refused for a `github:` one; only whether it is set reaches the `site-profile` action, and its value reaches only the cloud feed steps' environment, never a command line or a log | not set | the `site-profile` action (`feed-publisher-token-set`); the version job's List the cloud feed step and the publish and promote jobs' cloud publish steps (`scripts/desktop-feed-publish.py`, from the environment variable of the same name) |

A prepare-only Desktop Release run of a `github:` feed needs
`RELEASES_REPOSITORY` to name an existing public repository, because the
version job lists its releases with the workflow's own `GITHUB_TOKEN` to
allocate the version. It does not need the releases App pair or
`RELEASES_TOKEN`: those are needed by Desktop Promote and by a publishing
Desktop Release run. A `cloud` feed's version job lists the feed instead.

### Crate overrides

An internal build can choose the core crates it is built with. Desktop
Release takes `agent-relay-node-rev` (a full 40-character commit or a tag in
`AgentWorkforce/relay`) and `relaycast-version` (an exact crates.io
version); an empty input keeps the pin. The Resolve version job refuses
either input unless the run is `prepare-only`, names a `profile`, and that
profile's `desktop-<id>` environment sets the variable
`DESKTOP_CRATE_OVERRIDES_ALLOWED` to that profile's id (for `q7m2k9`, the
value `q7m2k9`). The value has to name the profile because a variable the
environment does not set falls back to a repository or organization one: a
value set there names no single profile, so it unlocks none. A published
release or the default profile never carries an override. A 40-character
`agent-relay-node-rev` must also be on a branch or tag of
`AgentWorkforce/relay` (the job checks with a blob-less clone and
`git for-each-ref --contains`): a commit on an unmerged branch there is
fine, while a commit that only exists in a fork, which GitHub would
otherwise serve under the relay URL, is refused. Push it to a branch in
`AgentWorkforce/relay` first.

`scripts/apply-crate-overrides.sh` rewrites the pin in both `Cargo.toml`
files, runs `cargo update` on both lockfiles, checks that they agree and
writes the resolved crates as JSON; with no input or with the pinned values
it changes nothing. It fails before any build when the rev, tag or version
does not resolve, and leaves the files as they were. The version job runs it
once and hands the four Cargo files to every build job as the
`crate-overrides` artifact. When the agent-relay-node rev changed, the macOS
`build` job runs `scripts/generate-contract.sh` before it builds the app,
so the Swift contract matches the core it bundles; a contract that no longer
compiles against that core is a real build failure, not a workflow fault.
The pre-release tests run on the pinned source, not on the override: with
an override active they build the pinned Linux core themselves instead of
reusing the release-core job's overridden one. The
candidate manifest records `"crates": {"agent-relay-node": {"rev": …},
"relaycast": {"version": …}}`, and About > Diagnostics on macOS and GTK shows
the same two crates, read from the lockfile at build time (`crates.json` in
the Mac bundle, `gtk/build.rs` for GTK; a development reload shows them as
Unavailable). A local `build-app.sh` that cannot record them warns and
leaves `crates.json` out, so About shows them as Unavailable; a signed
release build stops instead. `bash scripts/test-crate-overrides.sh` tests the script.

`relayscrubber-version` (an exact crates.io version of the session-body
scrub engine) is gated the same way. Until both `desktop-core/Cargo.toml`
and `gtk/Cargo.toml` depend on `relayscrubber`, the script refuses the
input with a plain message instead of ignoring it; once they do, it is
required exactly in both manifests, both lockfiles move to it, and the
record adds `"relayscrubber": {"version": …}`. The engine itself sits behind
`ScrubEngine::default_engine()` in `desktop-core/src/history/scrub.rs`; it
is pinned at exactly 0.5.0-alpha.8 (ruleset 10), and its engine name
carries that version. relayhistory and terse are out of scope: Desktop does not depend on them as
crates.

### Publishing to a cloud feed

A profile whose `RELEASE_FEED` is `cloud` publishes to its own site's feed
through `scripts/desktop-feed-publish.py`, which follows the proposed Cloud
contract (relay-control docs/desktop-feed.md). Its paths, fields
and error codes may still change, so nothing here is a stable API. The
bridge is never reached from a `github:` feed, and a `github:` profile
publishes as before.

- The version job lists the feed, drafts included (`list`), and
  `release-notes.py --allocate --feed cloud` allocates the version against
  that listing. Cloud versions write the month without a leading zero
  (`2026.3.1`).
- The publish job, and Desktop Promote for a prepared candidate, verify the
  candidate (its manifest records `"feed": "cloud"`). They take its
  fixed cloud set (see below) from
  `release-candidate.py cloud-assets` (never the unversioned aliases or the
  `.sha256` files), and `publish` declares, uploads and publishes them.
  `install.sh` is declared as Linux `x64` and `install.ps1` as Windows `x64`,
  both of kind `install-script`: the proposed contract's `arch` is `arm64`
  or `x64` only, and each script installs either, so its arch is ignored.
  A stable is published as latest; a beta is not. Each run prints the signed
  release's version, `published_at` and asset ids.
- If the reply to a publish is lost, a rerun's create answers 409
  `version_exists`. The bridge then lists the feed: if the version is
  published there, on its channel and by a publish, the rerun is done and
  sends nothing more. The publisher token reads versions and states only,
  so the published assets cannot be compared. Anything else fails.
- `--dry-run` prints the plan and sends nothing.

#### The signer's manifest limit

The feed's signer caps the RFC 8785 canonical bytes of the manifest it signs:
an AWS KMS Ed25519 key signs at most 4096 raw bytes. The listing reports the
cap as `limits.max_canonical_bytes`. The contract reads null as "the signer
declares none", but `publish` does not treat null or absent as uncapped: it
assumes the KMS default of 4096, and says so in its pre-flight line.

Every cloud profile publishes the same fixed set of 16 assets, from
`release-candidate.py cloud-assets`: the floored `macOS15` `.dmg` for arm64
and x64, the Linux `.tar.gz` and `.deb` and the Windows `-Setup.exe` and
`.zip` for arm64 and x64, the four Linux and Windows probe bundles
(`Linux-<arch>-probe.tar.gz`, `Windows-<arch>-probe.zip`, which
`linux/install.sh` and `windows/install.ps1` select for headless install and
update), `install.sh` and `install.ps1`. The macOS probe bundles and the
unfloored image are not published (floor-aware updaters ignore the
unfloored image when floored ones exist). A candidate that does not yield
exactly those 16 is refused, and the command logs every selected and
omitted name. A `github:` release keeps its full asset set.

The release notes are signed inside the manifest, so `desktop-feed-publish.py`
cuts them on whole lines to up to 300 canonical bytes (`NOTES_MAX_BYTES`),
less when the asset list leaves less room under 4096: the budget is the
limit less the manifest measured with empty notes, never more than 300. It
keeps the leading lines and logs how many bytes it kept and whether the
fixed cap or the budget cut them. Cut notes end with a blank line and
`Full notes: <site>/cloud/dashboard/downloads#v<version>`, the room for which
is reserved inside the budget; notes that fit whole get no link. The feed
contract has no notes URL, and the downloads page is the only release page it
names. The budget depends on the profile,
version, channel, assets and limit, never on the notes, so a rerun builds
the same notes and an identical create resumes its draft.

Before it creates a draft, `publish` measures the manifest with Cloud's
fields at their longest (a 10-character `id`, `a_` and 8 base62; a
20-character `published_at`; one 100-character download host, since a
profile has one storage host or none and an S3 virtual-host name fits in
100) and prints the size and the limit. For a 10-character profile, with
ten-digit sizes, the 16 assets with empty notes measure 3854 bytes at
2026.11.1-beta.1 and 3884 at 2026.12.99-beta.99, which leaves the notes 242
and 212 bytes; a 5-character profile at 2026.11.1-beta.1 leaves the full
300. When even empty notes do not fit it exits 2 having created and uploaded
nothing; a malformed limit exits 1. If the feed still answers 422
`manifest_too_large`, `publish` exits 2 quoting the feed's `canonical_bytes`
and `max_canonical_bytes`; it is never retried and never read as a replay.
At create nothing was created. At publish the draft stays with every asset
uploaded, and the message names it: a rerun with the same notes and packages
resumes it once the signer takes the manifest; other notes or assets need a
new version, since the publisher cannot delete a draft. If the listed limit
changes between attempts, the notes are cut differently and the rerun fails
with exit 1, saying so: rerun at the earlier limit or use a new version.

The bridge fails closed. It refuses, with exit status 2 and before any
request, a missing token, profile or site, a site that is not an `https`
origin, a feed that is not `cloud`, an unknown asset or an empty or
duplicate one. It also refuses a profile id that is not 1 to 32 lowercase
letters and digits, and a version that does not match its channel. Every
request carries `X-Agent-Relay-Profile` and `Authorization: Bearer`, with
the token read only from `DESKTOP_FEED_PUBLISHER_TOKEN`, redacted in
everything it prints, and never sent through a redirect. A failed request
or an unexpected receipt or envelope exits 1 and leaves the feed's draft
unpublished. A rerun resumes the same draft and uploads only what it is
missing. GitHub creates no release, and the releases App and
`RELEASES_TOKEN` are not used.

To move a profile's installed apps from a GitHub releases repository to the
cloud feed, Desktop Promote takes a `bridge-environment` input,
`desktop-<id>-bridge`. That environment holds only `RELEASES_REPOSITORY` and
the `DESKTOP_BRIDGE_APP_ID` + `DESKTOP_BRIDGE_APP_PRIVATE_KEY` pair, a
GitHub App installed on that repository with Contents read and write. The
names are the bridge's own, so a repository-level `RELEASES_TOKEN` or
releases App pair is never used in their place. Only the promote workflow's
bridge job reads it, and that job never builds. It refuses the default
product's releases repository. Once the cloud publish has succeeded, the
job verifies the same candidate the same way and publishes it to that
repository, so the older apps update to a build that reads the cloud feed.
It first checks that the version sorts after every release in that
repository as the updaters compare versions (numerically, so `2026.3.1` is
older than `2026.03.2`), and fails if it does not.

Other environment **secrets**: an environment-level secret of a name the
repository already has overrides it for that profile.

| Secret | Meaning | Default profile | Consumed by |
|---|---|---|---|
| `POSTHOG_API_KEY` | The default profile's telemetry key, built into the core; required for the default profile and used by no other | the repository secret | every build job of the default profile (`AGENT_RELAY_POSTHOG_API_KEY` for the core's build); `site_choices.rs` keys telemetry on a key being built in |
| `DESKTOP_POSTHOG_API_KEY` | No longer read. A profile build is given no PostHog key: it reports only to its own site's analytics gateway with the signed-in session, never to the public host, and the core's build refuses `AGENT_RELAY_POSTHOG_HOST` or `AGENT_RELAY_POSTHOG_API_KEY` for a site build. The environment secret may stay set; it is unused | not set | nothing |
| `DESKTOP_POLICY_DEFAULTS` | Optional. The organization's upload settings, built into the core: a JSON object with any of four keys, `sharing_mode` (`all`, `new` or `selected`), `upload_control` (`operator` or `user`), `oops_window_seconds` (0 to 86400) and `user_retroactive_delete` (`true` or `false`); anything else (another key, a wrong type, a value out of range, no object) fails the build, naming only the secret and the key. Applied from first launch and while offline until the site's `access/me` policy has been read, and always replaced whole by the fetched policy: a key the fetched policy leaves out is the person's. A profile without it keeps uploads withheld until the policy is read. The site-profile action checks, masks and exports it as compact JSON; `desktop-core/build.rs` checks it again and refuses it on a build for the default site | not read: the default profile never takes defaults | the site-profile action; `desktop-core/build.rs` (core and probe builds) |
| `DESKTOP_HISTORY_UPLOAD_STORAGE_ORIGINS` | Optional. A JSON array of the `https` origins of the storage hosts direct uploads may PUT to even when they resolve to private addresses (a store on the organization's private DNS), such as `["https://store.example:9000"]`. Site builds only: `desktop-core/build.rs` refuses a non-empty list on a build for the default site. Each entry is an origin on a DNS name: no IP literal, user info, path, query or fragment; anything else fails the build, naming only the secret. A PUT is let through only when its URL matches an entry exactly by scheme, host (without case) and port, and only for the storage PUT itself: History, Cloud and Control requests to the same host keep the private-address refusal, and redirects are never followed. The site-profile action checks, masks and exports it as `AGENT_RELAY_HISTORY_UPLOAD_STORAGE_ORIGINS` (a sorted JSON array); `desktop-core/build.rs` checks it again | not read: the default profile never takes storage origins | the site-profile action; `desktop-core/build.rs` (core and probe builds) |
| `RELEASES_APP_ID` | The ID of a GitHub App with the repository permissions Contents read/write and Metadata read, installed on the organisation with access limited to `RELEASES_REPOSITORY`. The jobs mint a short-lived installation token for that repository alone; preferred over `RELEASES_TOKEN` | the repository secret (default profile only) | the default profile's publish and promote jobs (`actions/create-github-app-token`) |
| `RELEASES_APP_PRIVATE_KEY` | A private key (`.pem`) of that App; set with `RELEASES_APP_ID` or not at all | the repository secret (default profile only) | the default profile's publish and promote jobs |
| `DESKTOP_RELEASES_APP_ID` | A profile's own releases App ID, with the same permissions, installed on that profile's `RELEASES_REPOSITORY`. The name differs from the repository's on purpose: an environment secret shadows a repository secret of the same name but cannot hide one, so a profile can never run the default profile's App | not set | the profile's publish and promote jobs |
| `DESKTOP_RELEASES_APP_PRIVATE_KEY` | A private key (`.pem`) of that App; set with `DESKTOP_RELEASES_APP_ID` or not at all | not set | the profile's publish and promote jobs |
| `DESKTOP_RELEASES_USE_ORG_PUSHER` | Optional. `true` (compared case-insensitively, as GitHub expressions do) opts a profile into the organisation's pusher App (the organisation secrets `GH_APP_PUSHER_ID` and `GH_APP_PUSHER_PRIVATE_KEY`) when its own `DESKTOP_RELEASES_APP_ID` and `DESKTOP_RELEASES_APP_PRIVATE_KEY` are unset; the token is minted the same way, Contents read/write on the profile's releases repository alone, and the App must be installed on that repository. Any other value, or unset, is no opt-in. A profile's own pair always takes precedence | not read: the default profile never uses the pusher App | the profile's promote job (Desktop Promote) |
| `RELEASES_TOKEN` | The fallback, per scope, when the selected App pair is unset: a token with Contents read/write on `RELEASES_REPOSITORY`. A profile's environment secret of this name shadows the repository's | the repository secret | the publish and promote jobs |
| `DESKTOP_BRANDING_URL` | Optional. An `https` URL of a `tar.gz` holding, at its root, `icon.svg` (the app's mark, square), optionally `icon.png` (1024×1024) and `colours.json`, and, for a co-branded profile (`DESKTOP_COBRAND_NAME`), one of `cobrand.svg` or `cobrand.png`, the customer's mark. With a co-brand mark `icon.svg` is optional; without one it is required. A co-brand mark is refused for the default profile, and without `DESKTOP_COBRAND_NAME` | none: the repository's mark and colours apply | the site-profile action fetches and unpacks it into `branding/` (exported as `BRANDING_ICON` and `BRANDING_COBRAND_MARK`); the build steps pass `--icon branding/icon.svg` to `build-gtk.sh`, `build-linux.sh`, `build-windows.sh` and `build-app.sh`. The Mac app records `Resources/mark.txt` and draws its app icon where it would draw Agent Relay's mark. The macOS build and disk image steps pass `--cobrand-mark` and `--cobrand-name` to `build-app.sh` and `package-dmg.sh`; Linux and Windows do not draw the co-brand yet. `colours.json` is unpacked and consumed by nothing yet on Linux and Windows |
| `DESKTOP_BRANDING_TOKEN` | Optional. An override, sent as `Authorization: Bearer` when fetching the branding | none: for a URL in `AgentWorkforce/desktop-branding` (`https://raw.githubusercontent.com/AgentWorkforce/desktop-branding/<ref>/<id>.tar.gz` or the contents API, `https://api.github.com/repos/AgentWorkforce/desktop-branding/contents/<id>.tar.gz`), the site-profile action mints a one-hour installation token, Contents: read on that repository only, from the organization's `GH_APP_PUSHER_ID` and `GH_APP_PUSHER_PRIVATE_KEY` (the App must be installed on `desktop-branding`); any other URL is fetched without a token | the site-profile action |

What a profile does not change: the core's constants that every install
shares, which a profile build keeps on Windows and Linux, since there is one
Agent Relay app per machine (the install root `%LOCALAPPDATA%\Programs\AgentRelay`,
the Start menu shortcut `Agent Relay.lnk`, the `Agent Relay (<account>)`
logon task, the AppUserModelId, the installer's `AppId`, the tree names
`AgentRelay-Windows-<arch>` and `AgentRelay-linux-<arch>` inside the
archives, the `agent-relay.desktop` entry file and the package's file paths,
the headless prefix `~/.local/lib/agent-relay` and the `agent-relay.service`
unit). The command-line copy of the core and launch at login are on
for any release profile, that is any build whose site is not a development
(loopback `http`) one; hosted-MCP recognition of a profile's hosts is a later
change.

**Install identity.** There is one Agent Relay app per machine (decided
2026-11-06). On macOS a profile's build is its own app, with its
own bundle identifier, but a profile's
build launched where Agent Relay, or another profile's build, is installed
replaces it silently, and Agent Relay replaces a profile's build the same way
(`ProductTakeover` in `macos/Sources/RelayDesktop/ProductTakeover.swift`, run
from `AppDelegate` at every launch, synchronously, before the model starts).
A product is recognised only by its marker: `build-app.sh` writes
`AgentRelayProduct` (the bundle id) into a release build's `Info.plist` and
its core's LaunchAgent, and `LaunchAgentService` copies it into the fallback
LaunchAgents it writes; Desktop CI's macOS package lane checks that a
release build carries it in both and a `--dev-site-url` or `--coexist` build
in neither. A
marker is trusted only in a bundle whose code signature is valid, from Agent
Relay's team (`AppUpdateInstaller.signingRequirement`) and under that same
identifier, which seals the `Info.plist` carrying it (`System.verifyProduct`,
Security framework); a LaunchAgent's marker counts only for a product one of
whose bundles passed that check. A
marker that is not a bundle id (`^[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$`) is
ignored, so it is never used as a path, label or defaults domain. A bundle,
running app or LaunchAgent without a trusted marker is never touched, so a
development build (Agent Relay Dev) neither replaces nor is replaced. The
replacement only works between builds from the release that introduced the
marker onward: an Agent Relay installed before it carries no marker and is
not found, so update Agent Relay first, then open the profile's build.

A profile can opt out: with `DESKTOP_COEXIST` set to `true` (an internal or
dev profile; see Site profiles) its build is made with `build-app.sh
--coexist`, which writes no marker into its `Info.plist` or its core's
LaunchAgent. Such a build sits beside Agent Relay and every other product with
its own socket `core.<bundle id>.sock`, data directory `<bundle id>` and
LaunchAgent `<bundle id>.core`; it is never taken over and never takes over
(`ProductTakeover.launch` does nothing without the build's own marker). A
product that replaced a marked build of that bundle id earlier and still
remembers it leaves the id alone while an unmarked build of it is installed or
running: its core is not booted out and its settings are not merged.
`--coexist` is refused for `com.agentrelay.desktop`, and the default profile
refuses `DESKTOP_COEXIST`. GTK (Linux and Windows) has no product takeover, so
the setting changes only the Mac app.

The takeover holds an exclusive lock on
`~/Library/Application Support/AgentRelay/takeover.lock` while it runs; a
product that finds it held waits for it (up to 30 seconds), then quits before
its core registers if that takeover replaced it (its bundle went to the
Trash; one whose bundle stayed survived it, even if its core was booted out,
and does not quit); if it was not replaced it takes over once it has the
lock, and skips the takeover for that launch if the lock is still held. It finds the
other product's LaunchAgents (`~/Library/LaunchAgents` and
`AgentRelay/SessionServices`), its running app
and its bundles (the Applications folders, the running app's location and
every copy LaunchServices knows, never one in `.Trash` or `.Trashes`), then,
in this order: boots out its core's labels only (never this product's core),
moves its bundles to the Trash, quits its app, deletes its LaunchAgent plists.
An app whose bundle could not be moved to the Trash (an admin's install in
`/Applications` opened by a standard user), or that started after this one,
is not quit, so two products that both stay installed cannot quit each other
at every login. With its core stopped the other app is force-quit at once: a
graceful quit could only show its "keep agents connected" confirmation about
a core that is gone. If its core could not be stopped it is asked to quit and
forced after five seconds. An app made to quit that is still running two
seconds later survives, like one left running. The launch only waits for launchd (up to six
seconds) when a core was booted out in that run; a remembered product whose
core is not loaded costs no wait. Its login item cannot be removed by another
app; it stays listed until its bundle leaves the Trash, and a replaced
product's core that comes back is booted out again at the next launch (the
replaced ids are kept in this product's defaults). Once, and only when this
product has no data yet (a directory holding only `.DS_Store` counts as
none), the most recently modified data directory of the replaced products is
moved to this product's (the core migrates it on open, as after an update),
only when it is a directory in Application Support itself, not a link to
elsewhere. While a product that has a data directory survives (a bundle of it
could not be moved to the Trash, its app was left running, or it did not
exit when made to quit) or its core is still loaded, nothing is moved, its
defaults stay and no marker is written, so a later launch decides. Its
defaults are merged in, its core's `startsAtLogin` renamed and its
build-specific keys left behind; a `.carried-over-from` file in the data
directory records the decision. Its first line names the product the data
came from (or says `kept`); each following line is `pending <bundle id>` for
a product whose settings are still to be merged, oldest first. Right after
the move it reads `<id>\npending <id>\n`; once the defaults are merged only
the first line remains, so a launch interrupted between the two merges the
defaults at the next launch without moving anything again. The product is
added to the remembered replaced ids before its data moves, and a pending
line is honoured only for a remembered id (any other is ignored and dropped),
so the file cannot make a launch import and delete an unrelated defaults
domain. If the moved directory brings the marker of that product's own
interrupted carry-over, its pending ids that the product recorded as
replaced stay pending ahead of its own and are remembered too (product C
taking over B, which had A pending, writes `B\npending A\npending B\n`);
the settings then follow at the next launch. Every launch first boots out
the core of each pending product, as for any replaced one, and merges their
defaults newest first, keeping one pending while it survives or its core is
still loaded and completing the others. A merge only adds keys this product
does not have yet, so this product's own values win, and among pending
products the newest wins a key both carry (it replaced the older ones). No keychain item is keyed on the bundle id.
Only a copy installed where it can keep its core (no
`LaunchAgentService.installationProblem`: not a disk image, a temporary
folder or the Trash) replaces anything.
On Windows and Linux a profile's build keeps Agent Relay's
install identity, so installing it over Agent Relay replaces Agent Relay in
place, as an ordinary upgrade would, silently and without asking; so does
Agent Relay's over a profile's, and one profile's over another's.
Coexistence is not a goal there. A profile's Windows Setup upgrades over
whichever product is installed (same `AppId`, install root, Start menu
shortcut, Run value and logon task; `windows/installer/AgentRelay.iss`).
What names the product follows the new one: Setup rewrites the uninstall
entry's `DisplayName`, and its `--install-launcher` run rewrites the
notification identity's `DisplayName`, the link handler's name and the
Start menu shortcut's description (`windows_product_records` in
`gtk/src/build_profile/windows_records.rs`; the app does it again whenever
it registers the launcher or shows a notification). Setup also removes what
the replaced product left that names it: every link handler under
`HKCU\Software\Classes` that runs a program under the install root and is
not this build's scheme, and the replaced Setup's desktop shortcut. A zip
or in-app-updater install unpacked over another product's gets the same
records on its first start, but the other product's link handler and
desktop shortcut stay (the app knows only its own scheme and name), still
opening that product's version folder until a Setup install removes them.
On Linux every profile's `.deb` is the `agent-relay` package with no `Conflicts` or `Replaces`, so
`dpkg -i` or apt upgrades whichever product is installed in place; only the
file name (by the profile's id) and the control file's `Homepage` and
`Description` name the profile, and versions compare across profiles as
within one (`build-linux.sh`). Every release publishes the headless
`install.sh` and `install.ps1`: a profile's copies are `linux/install.sh`
and `windows/install.ps1` with seven built-in values set to the profile's
(`PRODUCT_NAME`, `RELEASES_REPOSITORY`, `ASSET_PREFIX`, `RELEASE_FEED`,
`SITE_URL`, `PROFILE_ID` and `FEED_PUBLIC_KEY`; `$ProductName` and so on in
PowerShell), by Stage the release assets in `desktop-release.yml`. A
`github:` profile needs its releases repository and a `cloud` profile its
site, feed key and an id of 1 to 32 lowercase letters or digits; staging
fails otherwise, never printing a value. `install.sh` installs into the
same `~/.local/lib/agent-relay` prefix and `agent-relay.service` unit,
replacing whichever product's headless core is there.

A `cloud` profile's installers read the site's signed release feed instead
of a releases repository, through Cloud's proposed routes (not a stable
contract); see the Headless section of
[docs/enterprise-distribution.md](enterprise-distribution.md#headless).
The updaters and the installers read it; the rest of that design is not
implemented yet.

## Validation

Run `swift test --package-path macos` and
`xvfb-run -a cargo test --manifest-path gtk/Cargo.toml --features gtk --test linux_app_gtk`,
and on Windows the same `cargo test` in an MSYS2 CLANG64 or CLANGARM64 shell
(see [windows/README.md](../windows/README.md)). The GTK tests need GTK 4 and a
display (Xvfb on Linux; Windows has its desktop session). The bridge tests use local
Unix sockets and never require a real account. Inspect both wide and
minimum-size native windows when modifying these regions.

The bottom bar shows the full indexed session count for this machine, independent
of pagination, search filters, and hidden rows. It shows **Counting sessions…**
until that total is available. **Live Agents on Relay: N** has a green dot when
N is greater than zero and a grey dot otherwise. Updates remain available
in Settings and the account menu; request failures remain in Diagnostics.

### Reading a local session

Click any session row to open its local conversation, whether or not it is uploaded.
Opening the reader uses `sessions.transcript` without enabling uploads or relay registration.
The reader matches Cloud: chronological messages in a centered 760-point column,
compact user bubbles, assistant Markdown and tables (tables beyond the conversation’s rendering limit stay readable as text), and a compact **Activity** disclosure for tool work.
System prompts and image attachments are collapsed by default. Internal thinking and harness idle nudges stay hidden;
repeated capture IDs appear once while genuine repeated messages remain separate.
Messages load automatically through the full local transcript and open at the
latest reply. New replies follow while you are at the bottom; scrolling back
keeps your place and shows **Latest message** on its own row under the
conversation, so it never covers message text. There is no conversation sort button.

The editable session title replaces search in the shared header; there is no separate
transcript toolbar. Back to sessions sits between the wordmark and session title
in the shared header. **Rename title** sits immediately beside the session title;
**Details** stays with the account controls on the right and opens the sidebar, closed by default.
Matching full-width Open and Refresh buttons live in that sidebar
(a spawned session Agent Relay still runs may have no
Open; see below). Receive messages updates optimistically while saving. Failures
open a dialog and remain visible beside the control; a successful registration
survives a failed refresh so turning messages off still releases its name.
The Open action keeps its provider artwork when the shared core returns a
session-opening plan or a later refresh redraws the action. GTK shows **Open session**
until it has an actual plan for that row, then uses that plan's action wording.
This presentation is cleared when selecting a session; every Open still asks the
core to revalidate. Claude Code uses its CLI artwork, distinct from the Claude app.
The label remains centered across the full control width; it does not shrink to
a plain text button.
The sessions-table Messages button changes only messaging; it does not select
its row or reveal the bulk selection controls on any desktop platform.
The Messaging section groups Receive messages
with the agent name field and Rename button. Naming appears only when messaging
is enabled and a name is available, without scrolling at the minimum window size.
Copy name copies the session's name, even while a new name is being edited.
Helper text lives in tooltips; status and actionable errors remain visible; the
name field's tooltip explains renaming. Upload session includes its
local/uploading/uploaded status. Renaming changes the name other agents see on
Relaycast, leaving the session title unchanged. A session has one name: a rename
replaces it, so only the new name is shown and replies come from it, while the
earlier name only forwards messages sent to it, so nothing sent while renaming
is lost: the name the session was registered with until it leaves the relay, a
name a later rename replaced for 24 hours.
Turning Upload session off uses the
existing cloud-deletion confirmation. Back to sessions keeps the table’s search,
grouping, collapsed sections, and scroll position.

Group headers are buttons with chevrons and session counts. Click to collapse or
expand; collapsed state is independent for each grouping and survives list refreshes
and searches. Rows, headers, and controls have hover and keyboard focus feedback.
Actionable rows, buttons, menu triggers, and switches show a pointing-hand cursor.
Disabled controls use the arrow cursor and suppress action hover feedback; cursor
state follows sensitivity changes and is cleared when a hovered control disappears.

### Cloud styling

The desktop uses the effective Cloud workspace palette from
`cloud/packages/web/app/dashboard/_components/workspace-chrome.module.css`:
light canvas `#f9fafb`, dark canvas `#14191f`, primary `#2d6a9c` / `#74b8e2`,
and row hover `#e7eff7` / `#223e58`. These override the older airframe and
marketing palettes. macOS centralizes the tokens in `CloudTheme.swift`; GTK
uses the matching `LIGHT` / `DARK` palettes in `ui/theme.rs`. The macOS menu-bar panel
also uses these tokens for its canvas, text, hover, and status colors. Its session
rows and header/footer controls highlight immediately, without a fade, and pointer
motion within a control does not rewrite its hover state. The Linux
StatusNotifierItem menu is rendered by the desktop shell and follows the shell’s
theme; its protocol does not provide app-controlled menu colors.

Both appearances share Cloud’s 52px session rows, 28px compact filters,
6px group corners, subtle borders, bordered count badges, and muted zebra
stripes. Settings and local conversations use the same surfaces; code blocks
use Cloud’s dark code palette. Light, Dark, and Match system remain available.

Relay-owned model features use authenticated Cloud endpoints. The opt-in local
`credential-proxy serve` command and daemon proxy have been removed; independently
launched agent CLIs keep their existing authentication and environment.

The logged-out screen offers **Continue with Google** and **Continue with Microsoft**,
which open the chosen provider's account chooser instead of reusing a saved app or CLI session.
macOS, Linux and Windows use this explicit browser flow and PKCE handoff
without device-code confirmation. Local setup
clients can opt into it with `/setup/sign-in` and `{"flow":"browser"}`; older
and remote/headless setup clients retain device approval by default.

Conductor and Paseo sessions keep their provider badge and host label on macOS.
Live Codex sessions and top-level Claude Code chats hosted by Paseo use the
Paseo badge. Nested structured Claude runs remain automation and are not
auto-activated; host identity comes from the running process tree.

Conversation rows share the profile-picture cache and pending download, retrying a
transient failure together so newly displayed rows reuse a photo already loaded.
On macOS, transcript geometry callbacks only observe position; following new
messages and Latest message perform scrolling outside geometry updates to avoid
a layout feedback loop. GTK already avoids redundant adjustment writes.

Local conversations skip Grok’s `unified_log_usage` accounting records, preserving
message and tool events, usage totals and pagination through usage-only pages.

Local transcripts hide Codex external-context XML and keep embedded image
copies from RelayHistory behind an **Attached image** disclosure, closed by default. The reader writes each copy into its private
image cache and sends only that path over the core bridge. Reading never changes
upload selection. PNG, JPEG, GIF and WebP copies up to 1 MiB are supported; other
image blocks show an explicit unavailable notice. Capture requires the pending
RelayHistory embedded-image release; the published 0.40.0 capture engine omits
these bytes. Cloud additionally needs the History image-preservation change and
transcript renderer deployed before shared copies appear there.

Session titles include a compact harness · model · reasoning subtitle on both
platforms, with the full value in a tooltip. A session on the relay leads that
line with its agent name (`@name`, its newest name once renamed, in the foreground
color); hovering it explains which names reach the session. Models come from the latest observed
Codex turn context or Claude assistant record in a bounded transcript tail,
falling back to RelayHistory's catalog model names. Reasoning shows the explicit
Codex turn setting when available. Missing metadata is labelled unknown, including
sessions without local transcripts. These reads share the existing one-second
metadata budget and never hydrate or upload a transcript.

Upload status sits beside the switch in the single Upload column. Local-only icons are muted; uploaded
checkmarks use subdued neutral color. Hover labels read “Local only”, “Uploading…”,
or “Uploaded” (other states retain their specific labels). Uploading animates an
upward accent fill, with a steady arrow for reduced motion. The sweep represents
activity, not a percentage: the core does not report per-session byte/record
completion. Accessibility keeps the status available without hovering.

Every visible session column header is clickable. The Upload header sorts by whether uploading is enabled.
Click the active header again to reverse direction; an arrow marks the current
sort. Updated defaults to newest first; text/status columns start A–Z and toggle
columns start with enabled sessions. Missing values stay last. Sorting applies
to loaded sessions and within groups, and persists while filtering, viewing a
transcript, refreshing, or loading additional history.

### Reopening a local session

Desktop destinations use a provider logo, visible action text and open arrow in
the full-width Details-sidebar action; the tooltip and accessibility label name
the action (for example, **Open in Claude**). This includes the normal
`sessions.open_plan` path, not only the separate handover fallback. Terminal
resume keeps its terminal label and symbol on macOS. GTK keeps the selected
session's provider badge and the actual plan's action wording when the core's
answer or a transcript refresh redraws Open. Until an ordinary row has returned
a plan, GTK uses the generic **Open session** label; a known spawned plan can
name its destination immediately.
The layout does not add an opening capability where the core reports none.

**View on web**, below the open action, opens an uploaded session's Cloud page
(`/cloud/dashboard/sessions/<session-id>`) in the browser. It is hidden until
the session finishes uploading.

A Claude Code session something else holds is not offered a handoff it
cannot have. Claude Desktop resumes a session in a process of its own, so
Claude Code refuses `claude --desktop --resume` while anything holds the
session ("Session … is open in another terminal. Run /desktop there
instead."); only the holder hands it over. The core reports what holds each
session it discovers (`holder`), read from the field that session's own
registry entry records — the one Claude Code's handoff reads — and the open
action follows it:

- A session a person is typing in keeps **Open in Terminal**, which resumes
  beside them, with the note "This session is open in a terminal. Run
  /desktop there to continue it in Claude."
- A session a headless process holds (`claude -p`, a structured pipe, an SDK
  run) has no open action: Claude Code refuses a Terminal resume for one too.
  Its note is "A headless Claude Code process is running this session. Run
  claude agents to find it, or stop it, to continue it here."

A chat hosted in another app (Conductor, Paseo, Claude Desktop) runs a
structured pipe and is still a session a person types in; its entry records
that, so it keeps its open action. The holder is read only from the session
as discovery lists it now, so one that has ended holds nothing.

**Open in Claude** returns once nothing holds the session. A holder that
takes the session between that check and the launch is reported with the same
note, in place of the CLI's own line naming the session id. Linux shows only
the headless note: its answer for a session a person is typing in would be
`/desktop`, which Claude Code offers on macOS and x64 Windows only.

A session Agent Relay spawned is never resumed beside the process that runs it
([native-spawn.md](native-spawn.md#joining-and-taking-over-a-spawned-agent)).
Its transcript is followed live with a note ("Live: new output appears as the
agent writes it.") and a message box (**Message @name**, **Send**) under it: one
rounded chat-bubble shell, as in Cloud, that grows with the text and keeps
**Send** at its far right (Return sends; Shift+Return starts a new line on
Linux and Windows); an
agent's details on the agents page link to it with **Transcript: Join**. A
spawned Claude has **Take over in Claude** or **Take over in Terminal** in place
of the open action: it asks **Take over @name?** (mid-turn, offering **Take Over
After This Turn**, then **Interrupt Now** while it waits), stops the headless
agent and resumes the session where the opening preference says. A spawned
Codex thread has **Open in Codex** only, with a note that Agent Relay's Codex
app server runs it. Any other spawned agent (Grok, OpenCode, Muse, Devin) has no
open action while Agent Relay runs it; it is joined only, with the note "Agent
Relay runs this session. Follow it and message it here; it is not resumed in
Terminal while Agent Relay runs it." Linux takes over in Terminal: Claude
Desktop handoff is macOS-only.

Every spawned session Agent Relay still owns also has **End Agent** after the
transcript actions. It asks **End @name?**, then stops or archives the exact
harness session, confirms it ended, and removes its relay identity and
integrations. Any work in progress stops; local session history stays. While
that control applies, **Receive messages** is disabled with "Use End Agent to
stop this spawned agent", so switching messaging off cannot leave an untracked
headless process. A taken-over session has no end control: its process belongs
to the person and taking it off the relay leaves it running.

Take-over confirmations remain bound to the original agent when switching
transcripts. Take-over failures stay with their session until another attempt,
so returning to a transcript restores its error without showing it on others.

**Settings → Opening sessions** stores a per-computer **Automatic** or
**Terminal** preference for Claude Code and Codex. Automatic opens the desktop
app when installed and otherwise resumes in Terminal. macOS checks Launch
Services (including apps outside `/Applications`) when the viewer opens, when
the app becomes active, and again at click time. The provider page's **Local
sessions › Open with** row shows installation status; the collapsed **Opening
requirements** disclosure holds **Check installed apps** and Claude
download/setup links. The transcript
header's **Opening settings** button links directly to this page.

Codex opens `codex://threads/<session-id>` when a handler is available. On macOS,
Claude Code opens through `claude --desktop --resume <session-id>` in the
original directory, with a hidden pseudo-terminal: Claude refuses this handoff
when standard output is redirected to a pipe. The launcher captures failures,
limits execution time, and keeps **Open in Terminal** available. Claude requires
CLI 2.1.285+ and subscription login; a session still open elsewhere must first
be closed there. A launch failure offers Terminal and Opening settings.

Terminal resume runs `claude --resume <session-id>`, `codex resume <session-id>`,
or `grok --resume <session-id>` in the session's original directory. The adjacent
**Copy resume command** includes that directory. Missing directories never fall
back to the app's own working directory. Unsupported sources and Claude cloud
sessions do not offer a local resume action. Opening sends no prompt and changes
no upload selection. Historical local Claude Code records do not reliably identify
terminal versus desktop origin, so routing uses the preference and installed apps,
not the `claude-terminal` delivery-kind name or an inferred origin.

macOS terminal resume opens a private, self-removing command file in Terminal.
Linux uses `xdg-terminal-exec`, the system terminal alternative, or a supported
installed terminal. Linux keeps Claude and Grok on Terminal because Anthropic
currently documents the Claude desktop handoff only for macOS and x64 Windows.
Linux supports the same saved preferences, with Codex handler detection and
terminal fallback. Launcher errors remain visible; CLI errors appear in Terminal.

### Session search panel

Open **Search or filter sessions…** or press **⌘F** on macOS / **Ctrl+F** on
Linux and Windows, or press **/** while outside a text field. The search panel groups suggestions under Harnesses, Status, Projects,
and Upload. Choosing a suggestion completes the current query token and keeps
earlier words and filters; typed qualifiers and menu choices use the same query.
Press Return to view results and Escape to close. Use ↑/↓ to navigate suggestions; Tab on macOS or Return on a focused Linux
option chooses a suggestion. Search covers loaded sessions; scrolling loads older history.

Search suggestions support multiple selections. Click or press Right Arrow to
toggle an option; selecting it again removes that value. For example,
`harness:codex harness:claude is:active` shows active sessions from either harness.
Repeated harness, project, status and upload qualifiers match any selected value;
different filter types intersect, and exclusions always apply. Existing search
words stay in the query when selecting a filter.


### Workspace sign-in and upload ownership

The avatar menu shows **Switch workspace…** only when Cloud confirms multiple
workspace memberships. Sign-in opens a workspace selector in the
browser only when the account has multiple workspaces; a single workspace uses
the default directly. Desktop credentials remain bound to that choice even when
the browser switches workspaces.
Uploads and messages use that one workspace and its key; Settings › Messages
shows it as **Workspace from your Agent Relay account**.

Signing out stops the collector and removes its credentials, while retaining the
workspace's sharing choices, pending batches, and acknowledged delivery progress.
Signing back into that same account and workspace restores its existing job;
acknowledged history is not replayed. A new workspace starts with selected-only
sharing. Session discovery lists recent history first without uploading it. When
automatic uploads are enabled, initial full indexing prioritizes the last 24
hours, then the rest of the week, then older sessions by latest activity.
Registrations retained from another relay workspace cannot automatically
select sessions for the new upload destination. Explicitly sharing a session in
another workspace remains a separate choice.

Selected search filters toggle off individually without removing other query
terms. The search bar's × tooltip reads "clear search"; the search panel's ×
closes the panel while preserving the query. Escape also closes the panel.

The session search bar fills the toolbar beside the Table display icon and uses 14px horizontal
and 9px vertical padding. Filter qualifiers appear as chips with a remove ×,
including exclusions and date constraints. Removing a chip keeps other filters
and text, without opening the search panel. Chips scroll horizontally as needed.

Local transcript messages show the signed-in person’s name and avatar.
Consecutive tool calls and results collapse under **Activity**; expanding
the group reveals individually expandable calls and results. Groups continue
across loaded pages until a non-tool entry.

Workspace controls live in the avatar menu. The header uses one centered
30-point account avatar and a connection dot. Only an account with more than one
confirmed workspace shows **Switch workspace…** in the menu. Cloud's `whoami`
response supplies the distinct membership count and current organization avatar;
the core refreshes that metadata every five minutes without renewing credentials.
Older Cloud responses safely omit switching until the count is known.
**Switch workspace…** retains the confirmed sign-out/sign-in flow; it does not
move uploaded history or silently change accounts. Older sign-ins resolve the display name
from Cloud independently of Relaycast; until available, the label is “Workspace”
rather than an internal ID. A name is accepted only for the attached account and workspace.

Click anywhere in the session search bar, including padding, blank space, and
chip labels, to open search. Only the × controls remove filters or clear the
query. Chip labels do not remove filters, and scrolling chips does not open search.

Search editing is optimistic: the query, chips and checkmarks update immediately.
Session results are coalesced after an 80ms pause; obsolete requests are canceled
and only the latest query is applied. macOS filtering, sorting and grouping run off the UI
thread against a snapshot; cached groups keep query edits cheap. GTK defers table rendering to a later main-loop turn.
The previous result list remains visible while the next result set is prepared.

In search suggestions, ↑/↓ move the highlight. Right Arrow selects the highlighted
filter and keeps it highlighted; Left Arrow removes that filter and keeps the
highlight. Repeated Right or Left is idempotent. Text editing still uses arrow
keys normally until suggestion navigation starts. Clicking still toggles a filter.

Opposite search choices replace each other: Active versus Inactive, Not uploading
versus In progress, and Indexing versus other upload states. Compatible
choices (such as Codex + Claude or Uploaded + In progress) remain multi-select.
Choosing a positive value also clears its exact negative qualifier. Manually
combining opposite positive qualifiers shows a validation error.

On the relay is an independent condition and intersects Active/Inactive, rather
than broadening the activity filter.

After navigating suggestions with ↑/↓, Return completes and selects the highlighted
option, then submits and closes search. Its matching trailing fragment is replaced
by the full qualifier (for example, `clo` → `project:cloud`); earlier words and
filters remain. Return without suggestion navigation submits the typed text.

Right Arrow completes the highlighted suggestion in place of its matching typed
fragment, preserving the highlight and keeping search open for more filters.

Keyboard filter selection tracks the highlighted filter across rapid additions
and removals, even when completing text expands the suggestion list.

Selected search filters show a white checkmark inside a filled blue circle.

Turning Upload off asks **Delete cloud copy?** before changing the switch. Confirming
removes that machine's uploaded session and its delegated subagents, keeps local
history, and stops future uploads; a session that joined the relay by itself
leaves it too. Cancel leaves Upload on. A server consent fence
blocks late batches until Upload is explicitly enabled again; that action starts a
fresh backfill. Confirmations are scoped to the current account/workspace. Deletion
errors appear on the affected row, and other rows remain interactive.

The native deletion contract is `uploader.sessions.delete` (the older exclude method
still only stops delivery). This requires History's session-selection endpoint and
migration 0037; deploy the History change before releasing these desktop controls.
No client-side hiding substitutes for successful cloud deletion.


### Development session reset

Session reset is a probe CLI operation, not a desktop Settings control.
Use `agent-relay-probe sessions reset --local` with the install's site, account,
workspace, and `--confirmation 'RESET LOCAL SESSIONS'` to clear local session
state and rediscover existing conversations while retaining sign-in and cloud
history. See the root README's development reset instructions.

The Linux sign-in and app mark use the canonical `assets/agent-relay-mark.svg`
with the same light-blue front and dark-blue back as macOS.

Search expands over the search bar, centered on its anchor, and dismisses
when clicking outside. Escape and the close button also dismiss it.

### Session column preferences

Open the **Table display** columns icon to configure the local session table.
The panel starts with a full-width native **Group by** dropdown with arrow-key
navigation, Enter to select, and Escape to dismiss. Its list matches the field
width and has no popover arrow or repeated heading. This is followed by column
visibility and column order. Drag a
handle to move the full row, or use the up/down buttons. Dragging shows the
destination with a gap on macOS and an insertion line on GTK. Rows have a
consistent 40px height and 6px spacing. Canceling a drag preserves the order. Changes apply immediately
and persist on this machine. **Done** uses the compact brand-blue primary button;
**Reset to default** restores Session, Status, Project,
Updated, Messages, and Upload. Harness, Directory, Session ID, Model, Reasoning, and Created
are optional. At least one column stays visible. Headers and rows
scroll horizontally together when the chosen columns need more space. Drag a header’s
trailing divider to resize its column; body cells have no resize handles. All
headers support sorting and share a 36-point row height so dividers align. Widths
preview live and save on release, survive hiding and reordering, and reset with
**Reset to default**. Double-click a divider to reset just that column. Escape
cancels a drag; keyboard/accessibility adjustments are available on the divider.
Sessions can be hidden from their row menu or the selection toolbar. Hiding takes
an active session off the relay, then excludes it from future uploads while
preserving its existing cloud copy. Hidden sessions are omitted by default;
**Show hidden** in table settings reveals them with muted rows and disabled
relay/upload controls, including in the transcript sidebar. Visibility and the
checkbox persist on this machine. Unhide restores visibility without restarting
sharing. A failed sharing shutdown returns the session to the table with an error.

The Session column fills spare table width, including after reordering (the last visible
column fills it when Session is hidden). The default window is 1280 × 800, with
420-point Session, 100-point Status, 160-point Project, 120-point Updated,
126-point Messages, and 100-point Upload columns. Reset restores these widths
and the default window size (except in full screen). Messages and Upload are centered;
wider layouts scroll horizontally. Harness and Model sort alphabetically, and
Reasoning sorts by effort from none through ultra; missing metadata stays last.
Hovering a sortable header shows the direction its next click will use. Moving
away restores the current sort arrow (or hides it for an unsorted column). Hiding
Upload or Messages does not change either setting on a session.

Created and Updated show relative times (including “Just now”) and reveal the exact
UTC date and time, including seconds, on hover. Every column supports sorting;
Created defaults to newest first, Status uses the displayed activity state, and
Directory and Session ID sort alphabetically. Created uses the first recorded
session activity; missing metadata reads “—”. Harness includes its logo, and Upload stacks its status
icon above its label. Messages uses labeled controls: Off, Connecting…,
Receiving, Offline, and Turning off…, each with a state icon.

Hovering anywhere on a session row highlights the entire row. Clicking its text,
harness logo, or gaps opens the session; the checkbox and surrounding leading
padding select it instead. Message controls keep their own click behavior.

Session rows have a left selection checkbox and harness logo, then the title and known model · harness · reasoning details. The harness logo sits on its provider's primary colour: Muse is a white mark on a blue circle, Grok a white mark on a black circle, and the Claude app a white mark on an orange rounded square; Claude Code in the terminal shows the orange Claude Code crab, and Codex and Grok Bot keep their own app tiles. A Claude session is the Claude app's when its transcript records the `claude-desktop` entrypoint (the history row's `originator`), or when it is a Claude app cloud session. Status is separate from the read-only Upload column. Selected rows are highlighted. Selecting sessions reveals the top bulk action bar with Messaging and Upload switches, followed by Hide with an eye-slash icon. Mixed on/off selections display as off; switching on enables the selected sessions, and switching off disables them. Turning uploads off retains the cloud-copy deletion confirmation.

Search expands from and collapses into its toolbar anchor with a 200 ms eased
transition. Reduced-motion preferences skip the animated close.

The compact header brand has no hover highlight or pointing-hand cursor. The search border is muted while closed. Account avatars are centered with padding, including single-workspace accounts. Available updates appear as an icon-only download button beside the upload status, without an update dot on the avatar.

The local session count appears beside the bottom-left status. When problems are present, click Needs attention to open Diagnostics with full, selectable and copyable error details. Distinct session and connection problems are collected together.


A discovered session awaiting an upload-catalog entry is labeled **Indexing**
in the list, sidebar, and upload filter. Missing catalog data alone does not
prove uploads are unavailable. Upload selection waits for the catalog entry;
known upload errors keep their specific explanation. Opening a session
excluded by Reset all sessions explains the reset and directs the person to
the source app, without recreating Relay’s cleared reader cache.

Each session group header has a checkbox that selects or deselects only the matching rows in that group, including when the group is collapsed. Other groups keep their selections. A partially selected group displays unchecked; clicking it selects the whole group.

### Request diagnostics

Settings → Diagnostics fills the available content width and shows recent Relaycast and Cloud requests from the background service, newest first, with request and response details, timestamps, duration, and a Failures only filter. Click anywhere on a request header to expand or collapse its details. The bottom-left dot and status text show overall app status: green when uploads are up to date with no problems, orange when a request fails or another problem needs attention. Clicking a request failure opens and highlights the exact entry in Diagnostics; it stays available when older requests roll out of the log. Viewed request failures do not notify again. Other current problems appear in Diagnostics with selectable, copyable details. Copy details briefly changes to Copied when the problem text is copied.

If the background service exits unexpectedly, the app shows **Reconnecting** while its service manager starts the replacement. Read-only requests interrupted by the exit wait up to five seconds and run again against the replacement. Changes are never sent again automatically, because the old service may have completed them before the connection closed; the app asks the person to check the result before trying again. A change the app could not send at all, because no connection to the service opened, is different: its message says the app is reconnecting or starting, and it clears once the service answers; Retry uploads and Compact now, which are safe to repeat, are then sent once on their own. A service that stays unavailable shows a recovery action. Once the new snapshot arrives, a transient read error no longer keeps the status orange.

A plan cap Relaycast reaches is explained where it bites. A session refused
from the relay because the plan's agent cap is full keeps the core's sentence
on its row (**This workspace is using 15 of 15 agents on the Intro plan.
Upgrade the plan to add more.**); the core's typed `plan_limit_exceeded` error
carries the cap, the counters and the plan, so no request-log scan is needed.
A device cap reached while enrolling this computer's spawn node is the node's
status line, with the same sentence.

An upgrade notice appears at the bottom right, at most once every 15 minutes per workspace, with **Review upgrade** (opens Cloud Billing) and dismiss. It reads **Agent Relay plan limit reached** when Relaycast reports the workspace's plan quota is used up. It reads **Workspace at capacity**, naming the plan, when a Free or Pro workspace had three or more writes answered with `workspace_busy` in the last five minutes. The plan comes from Relaycast's workspace (read during verification); busy answers on lanes no plan raises (connection tokens, heartbeats, node calls, delivery acknowledgements) do not count, and Relaycast's own `upgrade_available`, which the request log records from each busy answer, decides when present. A single busy write is ordinary contention and the app simply retries. Requests made for a Relay Connect room (the request log tags them with `connect`) are that room's workspace, not the team's, and never raise either notice.

The log lives only in memory for the current service run (up to 250 entries; large responses and bridge snapshots are bounded). Credentials and message content are redacted. Relaycast SDK rows contain decoded results or final error details, including HTTP status, request ID, and retry count where available. Successful HTTP statuses and individual SDK retry responses are not exposed by the SDK. Cloud rows include the actual HTTP status. Calls made with a Relay Connect room's credential are labeled Relay Connect instead of Agent Relay, because they use that room's workspace and its limits, not the team's. The log covers the core's Relaycast calls and Cloud HTTP transport, not WebSocket frames or separate upload transports.

Session lists use the daemon's shared snapshot: reopening the app reads cached
rows immediately while live-agent discovery and upload enrichment run in separate
background tasks. A cold daemon seeds rows from the collector's persisted catalog;
global upload status and history requests run independently in both desktop apps.

User-facing messages, help text, and diagnostic service labels use **Agent Relay**.
Relaycast remains an internal provider identifier.

The menu-bar panel and the Linux and Windows tray are an at-a-glance summary of this computer's
sessions, laid out as a plain menu. **Messages** counts sessions **Receiving
messages** (`is:active is:on-relay`) and **Local only** (`is:active
is:off-relay`); **Uploads** counts **Uploaded** (`upload:uploaded`) and **Not
uploaded** (`-upload:uploaded`). Each count is that search run over the rows
the session list shows, hidden sessions excluded unless shown, and choosing it
opens the session list with that search, so the list shows exactly the sessions
counted. **All sessions** opens the session list with no search; Settings… and
Quit Agent Relay follow.

The menu shows no banners. On the Mac each section title carries a status dot:
green when it is working, grey while paused or connecting, and the warning
orange when something stops it. Its wording shows on hover, and an orange dot
opens Settings → Diagnostics (the failed request, when there is one), like the
window's **Needs attention**. While something needs attention, the menu-bar
icon is drawn in that orange. Receiving messages and Uploaded are badges in the
brand blue while nonzero and nothing needs attention, the rest muted. The Linux
tray has no hover: its icon turns the same orange, **⚠ Needs attention** under
Messages opens Diagnostics, and the upload status is written beside Uploads.

After browser authentication, accounts with multiple workspaces choose a workspace in the welcome screen. No relay connection or upload setup starts until a workspace is selected. A single workspace continues automatically; Cancel returns to sign-in.

### Saved session views

A single bookmark button sits beside the search box and opens
**Saved views**. The bookmark, table settings, and account controls use compact
6-point gaps while retaining their existing hit areas. Its bookmark is filled whenever the current search and table
configuration matches a saved view; changing those settings returns it to an
outline. Choose **Save current view** in the dropdown, give it a unique name,
and optionally choose **Make default**. Select a saved name to restore it, use
the star to change the default, or the × to delete the view. Deleting a view leaves
the current table unchanged;
deleting its default removes the startup override. Views are local to this machine.

A view preserves the search text, grouping, sort direction and column, column
order/visibility/widths, and **Show hidden**. Date filters preserve their explicit
dates; saving a date range does not turn it into a rolling window. The default
loads at app startup. All controls have accessible names and keyboard focus;
Enter saves from the name field and Escape dismisses the popover. Delete remains
available without hover, with stronger emphasis on hover or keyboard focus.

Primary blue actions use the canonical Agent Relay button, including Save current
view, Save, Done, sign-in, settings, and upgrade. Both themes use brand blue
`#2d6a9c`, white text, a `#234969` border, and 10/10/10/3 corners (the tight corner
is bottom-left). Hover darkens to `#26557e` to keep white text accessible.
There is no shadow or glow. Keyboard focus and disabled states remain visible.
Use `AgentRelayPrimaryButtonStyle` on macOS and the `primary` class on GTK;
workspace accent colors are separate and must not define button fills.

### Usage diagnostics

macOS, Linux and Windows share explicit PostHog usage events through the relay core.
Agent registration/removal, message sending and initial transcript reads record
attempts and outcomes in shared handlers. The UI records page/settings-pane
navigation, opening the message composer, browser opening, and debounced
session searches. Search queries, names, recipients, session IDs and
message contents are never sent. Session selection counts and sharing modes
accompany their operations; deletion has its own operation. Observed upload
completion counts exclude startup history and repeated polls. Update events
report installation handoff; Linux `.deb` downloads require manual installation.
Background transcript following is excluded from open counts, even without a
cursor. Awaited update diagnostics wait at most three seconds per call; GTK
keeps the window responsive during that wait.
Settings → Share diagnostics controls all of these events. See README's
Desktop analytics and error tracking section for the complete schema and limits.

Spawned Grok agents can reply through the relay-send socket without a separate
MCP setup. The core verifies the caller's exact recorded leader, session claim,
process ancestry and private marker before identifying the sender.

## iPhone and iPad

The mobile client has **Agents** and **Messages** tabs; Drive has no separate
tab. The phone header pins the Agent Relay logo to the leading edge, and iPad
keeps its logo in the left rail. Agent conversations follow the current relay
roster and disappear when their peer leaves or its presence expires. People's
conversations remain available offline. Message history stays on Relaycast.
Presence reconciles every 30 seconds, on foregrounding, and on tab changes.
Machine menus group equivalent hostname spellings and match every variant
when filtering. These changes are mobile-only; the desktop layout above is
unchanged.

Agent rows show **Stalled · N messages waiting · oldest M min** when the core reports an unread Claude inbox or a silent Codex turn blocking relay delivery. The wording and warning color match on macOS and Linux; the mark clears when progress resumes. See [native-spawn.md](native-spawn.md) for Codex steering, spawn-only interruption, and timing settings.

### Relay agent names


New relay registrations use `harness-repo-task`, with up to two meaningful
words from the session title (for example `claude-cloud-plan-support`). Names
are chosen locally; filler words are skipped. A missing task title uses
`session`. Duplicate automatic names get `-2`, `-3`, and so on. Existing
ordinary title edits leave the relay name unchanged. **Rename title** changes
native session metadata; **Rename agent** under Details → Messaging changes
the @mention name.

The shared core's `session_naming` worker refreshes both fields after ten new
human text prompts in a live personal registration, using the existing Cloud
LLM session brief. The agent name is the normal `harness-repo-task` slug of that
generated title. It starts counting when local prompt evidence first becomes readable, checks
once a minute while native prompt scans are limited to once every five minutes,
and persists the interval in private `session-naming.json` in the app data
directory. Background scans only hydrate local sessions; remote-only sessions
use evidence already cached by a person opening the reader and never download
transcripts or request provider credentials. Manual edits restart the interval; a pending generation
cannot overwrite a later manual edit. Tools, system context, images attached
to a prompt and duplicate captures do not count as additional prompts. No
provider turn is injected and sharing choices are unchanged. A session must
already have readable shared Cloud evidence; missing, pending, stale or offline
briefs keep the existing names and retry with a five-minute backoff. Native
title support is required; spawned agents retain their assigned names. Only
registrations owned by the signed-in person in the attached workspace qualify.
The original @name keeps forwarding until the registration leaves; replaced
aliases forward for one day. This behavior is shared by macOS, Linux, Windows
and headless mode.

### Codex CLI discovery

Codex terminal message delivery resolves npm CLI launchers to their bundled native
executables, so the desktop or background service does not require Node on its
PATH. Nested and hoisted platform packages and legacy vendor layouts are supported.
On macOS, discovery also recognizes the CLI inside the newer
`Resources/codex-cli/CodexCLI.app` bundle layout.

### Account health

Account settings on macOS, Linux and Windows include **Check health**, which
asks the shared core to verify Cloud and Relaycast account/workspace access
using the app’s signed-in credentials. The core checks automatically and refreshes
renewable sign-ins. The header and menu/tray show **Connected** only after
verification; checking, network failures and sign-in requirements have distinct
states. **Sign in again** appears when browser authentication is required. A
failed network check does not log out the account or claim authentication expired.

Account health does not verify that every running session has loaded its MCP
configuration. Agent Relay retains its existing hosted MCP endpoint and toolset;
clients connect to that HTTP endpoint directly. Other harness integrations keep
their existing transport.

#### Hosted MCP shares the app login

Selected Codex and Claude Code clients connect directly to the existing hosted
MCP endpoint. Their native HTTP header-helper mechanism invokes
`agent-relay-probe mcp-headers --site <origin> --workspace <id> --account <id>`
when authentication needs refreshing. This credential-only helper asks the private
desktop core for headers pinned to that site, workspace and account, and returns
only the Authorization header as JSON to the client. The fetch is bounded to
30 seconds and never starts an interactive login. The core renews the app’s kept login;
configuration files contain the helper command and account pins, without a stored
bearer token. Switching accounts or workspaces refuses stale configuration. A
kept app login is required: an environment-only `CLOUD_API_ACCESS_TOKEN` cannot
substitute. Remove a
differing `CLOUD_API_ACCESS_TOKEN` override so the desktop and hosted client use
the same credential. No local MCP server or replacement toolset is installed.
Grok, OpenCode, Devin, Muse, Pi, OpenClaw and Hermes have native global installers.
The app creates a separate opaque credential for each provider, restricted to
this hosted MCP endpoint and bound to the retained desktop login, account and
workspace. Credentials persist in owner-only storage before issuance, making
interrupted installation safe to retry. The app renews their 15-minute server
leases every five minutes without rotating the bearer, so clients that cache
HTTP headers do not need another login or reload for routine authentication.
Provider configs never contain the desktop access token or refresh token.
Disconnect revokes the provider grant; app sign-out revokes all grants. If the
server cannot be reached, grants expire after their bounded lease. Failed setup
retries back off. A server must advertise the provider-credential protocol before
issuance; older app logins without desktop consent get the existing Agent Relay
sign-in action, rather than a separate provider login. Deselecting a provider
removes its global entry and revokes its grant.

Grok reads its private app-managed bearer file on each request and supplies its
native session-ID header template; Pi registers with the native
MCP API from its global extension and updates context on session switches.
OpenCode V2 supplies native per-call session metadata. The hosted server resolves
these identifiers only against the app’s privacy-filtered session registrations.
Older OpenCode versions and providers without verified native session metadata
can browse shared sessions, but actor-dependent messaging remains blocked.

Claude Code also gets a PreToolUse hook beside its `agent-relay` MCP entry in
each shared login's `settings.json`. Install, repair and uninstall add and
remove the hook together with that entry and leave other hooks alone. Before
each hosted Agent Relay tool call, the hook adds a short proof, signed by this
computer, that the call comes from that registered chat, including after
`/resume`. A running chat picks the hook up on its next tool call. The core
signs proofs only once Cloud advertises `claude_session_attestation`; until
then Claude messaging stays unavailable.
First installation into an already-running client still depends on that client’s
configuration reload support; configuration verification is not a claim that all
running chats have loaded the tools.


### Choose and manage providers

**Choose providers…** on the Settings › Providers overview reopens the onboarding overlay
with the saved choices. Only Continue applies changes; Not now closes a reopened
chooser without changing existing connections.

Cursor is selectable in onboarding and uses the same install, repair and
uninstall actions as other local providers. Its scoped, revocable desktop
credential is written to `~/.cursor/mcp.json`, preserving unrelated MCP servers.
On macOS and Linux, explicitly selecting Cursor also installs Agent Relay's
messaging hooks in `~/.cursor/hooks.json`, including before the first Cursor
chat. Repair reconnects both; uninstall removes only Agent Relay's entries.
A later explicit hooks Off is preserved by maintenance until repair is requested.
Windows supports Cursor MCP configuration and reports local Cursor session
messaging hooks unavailable. Reload Cursor or start a new chat after setup,
and approve the MCP connection if Cursor asks.

Initial onboarding shows Amp and Capy only with the core's explicit local-install
witness, not saved choices, Cloud connections or retained data. Capy's supported
macOS application locations qualify; `~/.capy` alone does not. Amp has no supported
local-install witness, so stays out of initial onboarding. Linux and Windows
report no local Capy application witness. Existing choices/connections, history
and the explicit **Add provider** catalog remain unchanged.
Grok Bot and Dot retain their Cloud setup choices without an installed app.
In Settings, these rows use setup actions rather than local MCP checkboxes; Amp's is
**Open Cloud connections**.
Capy opens `capy://app/new?prompt=…` when a registered app is available, falling
back to its prefilled web thread. Grok Bot opens
`grokbot://app/v1/prompt?text=…` with the Agent Relay Grok Bot setup skill in
its existing main Bot's input; Grok Bot preserves an existing draft. If the app
or a Bot is unavailable, the copied setup prompt can be pasted after creating
one. Neither action sends the prompt. Dot always links directly to the
Agent Relay plugin at
`https://chatgpt.com/plugins/plugin_asdk_app_6ad1e2038840819189295d49199c2915`,
even when no Dot install is detected. Their provider pages keep the same
setup actions; app detection does not claim a verified cloud connection.

Grok Bot connection status uses the signed-in person’s connector rows from the current workspace. Only a Cloud-verified owned agent with active message delivery shows Connected; app installation alone does not. Connection details offers Check again, and unavailable checks preserve saved setup. Dot also checks the person’s signed-in ChatGPT plugin connector; this verifies the plugin connection, not whether a particular chat has subscribed to incoming messages.

After sign-in, **Choose your providers** lets the person select the integrations to
install. Detected supported providers and installed Conductor/Paseo hosts are
offered. Selecting Conductor also selects detected Codex and Claude Code providers;
selecting Paseo also selects detected Codex, Claude Code, Grok and Muse providers.
An installed host can be enabled again from its page after being deselected. The host keeps using those providers’ settings, with
no separate host MCP installation or claim of live connection health. Conductor
discovery is macOS-only. Checkboxes, icons and names share a centered heading
row. Unavailable integrations explain their limitation. **Not now** saves an
empty selection. No integration is installed by background maintenance before
this choice is saved.

**Settings → Providers** lists the providers and their current configuration and shared
authentication status. **Check health** refreshes the checks. **Install**,
**Repair**, **Reinstall** and **Uninstall** act on the selected provider's recognized
Agent Relay entries, preserving unrelated MCP servers. Recognized older Relay
entries and duplicate aliases are replaced by the canonical `agent-relay` hosted
connection. Native Copilot uses the separator-free `agentrelay` name, while
legacy VS Code uses `relayhistory`. Copilot migrates only marked HTTP entries
or recognized legacy commands; unmarked same-endpoint entries and foreign aliases
are preserved. Grok, OpenCode, Devin, Muse, Pi, OpenClaw and Hermes also receive
global hosted MCP configuration authenticated by the app’s Agent Relay sign-in.
Their native configuration formats and environment overrides are respected;
Pi uses its native global extension registration API.
Uninstall persists an opt-out across restarts and sign-ins.

An installed configuration is reported separately from verification of tools in
a running chat. Provider maintenance does not restart chats or stop the background
service. Current failures remain visible so an unsuccessful repair cannot look
like a healthy connection.

Providers has its own Settings sidebar section, separate from Preferences.
Add provider is a full-width sidebar button with hover, focus, selection and a pointing-hand cursor. Providers
detected on this machine or explicitly enabled appear in alphabetical order, and so
do Capy, Grok Bot, Dot and Amp once Agent Relay Cloud reports their connection.
GitHub Copilot is the exception to detection: it is added explicitly (from **Add
provider** or onboarding) and never appears only because VS Code was found. **Add
provider** opens a separate panel for enabling the remaining providers; enabling one
saves the choice and opens its settings. Manually added providers without local MCP
support can be removed with **More options → Remove from sidebar**. Enabling a provider
does not mark its connection verified. Each
compact row shows the provider’s icon, name and connection indicator; selecting it
opens its health and install, repair, reinstall or uninstall controls. The “on”
count and green indicators require both verified configuration and verified account
access. Muted rows remain available for setup or explanations; they do not imply
that a running chat has loaded its tools.

Provider maintenance now reloads reachable existing Codex runtimes automatically
and checks each loaded chat’s hosted tools. Unverified configuration stays neutral;
failed or partial runtime verification needs attention. Green Codex status also
requires a verified runtime catalog. Per-chat sender identity is checked separately. See [hosted MCP runtime and native chat
identity](hosted-mcp-runtime.md) for the Cloud deployment dependency
and the private-runtime, Claude and spawned-worker boundaries.

Opening preferences now live on the Codex, Claude Code and Grok provider pages.
Claude’s page also contains its session folders, delivery, approvals and activation
hook controls. General contains the one global upload policy and Upload details,
explicitly applying to all providers. The separate Opening sessions and Session
sharing sidebar entries are removed; existing stored preferences are preserved.

### Operator upload policy

A workspace's operator can enforce upload settings. Cloud answers them in the
`policy` field of `GET /workspaces/{ws}/access/me`, as
`{"version":1,"revision":N,"settings":{"sharing_mode":"all"}}`; the only
setting today is `sharing_mode`. The core
(`desktop-core/src/history/operator_policy.rs`, applied from
`desktop-core/src/account/operator_policy.rs`) reads it at sign-in, when the
workspace is refreshed, and every 15 minutes while signed in, and keeps the
last answer per workspace in `policy.json` in the app data directory, which
signing out keeps.

- An enforced `sharing_mode` replaces the person's choice (`all` uploads past
  sessions too and clears exclusions, as choosing **Upload all past and future
  sessions** does). `uploader.sharing.set` and excluding a session while
  `all` is enforced are refused with "This setting is set by your
  organization." Resuming while uploads are withheld is refused with "Uploads
  wait until your organization's upload policy can be checked." The person's
  own choice is kept and given back when the operator lifts the setting; a
  sign-in under an enforced mode remembers the mode the person picked there.
- `uploader.status` carries `enforced_settings` (`{"sharing_mode":"all"}`,
  every present key as a string) and `uploads_withheld` (`true`, absent
  otherwise). While uploads are withheld, macOS
  (`GlobalSessionSharingSettings`) and GTK (`settings_page/sharing.rs`) lock
  every upload policy row and add "Uploads wait until your organization's
  upload policy can be checked." under them.
- Only keys present in the policy's `settings` are enforced; a present key
  whose value cannot be read fails the whole read (the cached policy stays).
  A control a present key governs is removed, never disabled, on macOS and
  GTK, and "Managed by your organization." (`operator_policy::MANAGED`, the
  Mac's `OrganizationPolicy.managed`, GTK's `MANAGED_BY_ORGANIZATION`) shows
  where it was:
  - `sharing_mode` (`all`, `new`, `selected`): the upload policy rows give way
    to one read-only row with the enforced mode's title and explanation. The
    terminal sign-in wizard says "Upload policy is <mode>. Managed by your
    organization." and asks nothing.
  - `upload_control: operator`: uploads run. `uploader.pause` (pause and
    resume) is refused with ENFORCED, a pause the person made earlier is
    resumed when the policy is applied, and the apps drop pause and resume.
    `user` changes nothing.
  - `user_retroactive_delete: false`: `uploader.sessions.delete` is refused
    with ENFORCED, and the apps remove every control that deletes a cloud
    copy (the session's Upload switch while it uploads, the bulk Upload
    switch when it could only delete, and Delete cloud copies). `true`
    changes nothing.
  - `oops_window_seconds` (0..=86400) is honoured only with
    `upload_control: operator` and a value above 0; only then is it reported
    in `enforced_settings`. Otherwise sessions upload as soon as they are
    captured, as before. A value outside the range fails the read.
    - A session is *finished* once it has been quiet for the whole window:
      `now >= last_activity_ms + window`, with `last_activity_ms` from the
      capture's `sessions` table (or, when that is unknown, the time its first
      record was held). New activity moves `last_activity_ms`, so resuming a
      session restarts its window.
    - The hold is on the delivery side (`history/delivery/hold.rs`). The
      collector's `deliver_captured_on` reads the policy through
      `operator_policy::upload_hold`; preparing a batch keeps a copy of each
      record of a session still in its window in `delivery_held_records`
      (history.db) and moves the job's cursor on. Every pass first releases
      the copies whose sessions (and relationship children) are finished, in
      revision order, then deletes each one, so nothing is sent twice. No
      server round trip decides the release. If the organization lifts the
      window, the next pass releases every held copy. A policy that cannot be
      read while signed in holds every upload and reports `uploads_withheld`.
    - While held, the session lists as `waiting` with
      `upload_hold_until_ms` (when the window ends). macOS and GTK show
      "Uploads in N min" in the row and the detail, and a **Don't upload**
      button in the detail. It excludes the session through the exclusion
      store, locally, and drops its held copies. Once the window has ended,
      excluding under the operator's upload control is refused with "This
      session's upload window has ended. Your organization uploads finished
      sessions automatically." A finished session whose copies have not
      been sent yet lists as `queued`.
    - The held copies, the exclusions and `last_activity_ms` are all on disk,
      so a hold survives a restart and time offline. A window that ended while
      the app was closed uploads on the first pass after it starts.
- A revision older than the cached one is ignored. A successful read without a
  `policy` field (or a Cloud that does not serve the route: 404, 405 or 501)
  means the person chooses. A failed read keeps whatever is cached, so an
  enforced value never relaxes offline. Only a sign-in with nothing cached and
  a failed read sets the install up with uploads withheld (paused, nothing
  deleted) until a read succeeds. Pausing uploads yourself while they are
  withheld clears the withheld mark and keeps them paused. Installs from before
  this read stay in the person's mode until their first read.
- `RELAY_DESKTOP_OPERATOR_POLICY` stands in for Cloud in test and debug
  builds only: a policy object, `null`, or a path to a file holding one.
  Release builds ignore it.

Cloud does not enforce the policy on uploads yet; that is a Cloud follow-up.

Providers also includes Conductor, Paseo, Capy and Replicas. Claude cloud-session
sharing is part of the Claude Code page, with no separate Claude cloud provider.
Capy appears in the sidebar when its connector directory exists, or its app is
installed in /Applications or ~/Applications on macOS. Replicas uses its official
symbol from https://replicas.dev/brand as bundled template artwork.
Codex runtime verification failures show reload/new-chat instructions and a
Check connection action that retries runtime verification immediately; connection
actions show Working… while their request is in progress.
Provider discovery follows custom GROK_HOME paths; macOS also searches the
bundled Devin command in /Applications and ~/Applications. Disconnecting one
provider keeps host sidebar choices, without restoring it when another host is enabled.
Each provider page includes that harness’s full-access switch under Spawned
agents while this computer accepts spawns. The general Spawned agents section
keeps the shared spawn controls and cap; saved permission values and defaults
are unchanged.
Host pages link to the underlying agents’ settings; Capy offers its existing
cloud setup flow. These entries do not install a second local MCP or inherit
a green connection indicator from the app login. Host/cloud connection health
remains unverified. Automatic discovery of Conductor and packaged Paseo host
sessions is currently available on macOS; their provider pages remain visible
on Linux and Windows with that limitation explained.

Amp is a Cloud connection: its token lives on the workspace in Agent Relay
Cloud, never on this computer. The core reads the workspace's agents from
Agent Relay Cloud; while Cloud reports one of your own bound Amp threads as
live, Amp is configured: it shows in the sidebar, its status reads **Connected
in Agent Relay Cloud**, and with a ready account it is “on” (green) like Capy,
Grok Bot and Dot. Otherwise its status reads **Connects through Agent Relay
Cloud**, even if one of your threads is still bound but idle; a teammate's
thread proves nothing. This proves a live bound thread, not that a token is
stored. Amp has no Check action: its one action is **Open Cloud connections**,
on its page and in onboarding, which opens
`<site>/cloud/dashboard/settings#connections` on the signed-in site. Amp shows
its official logo, bundled with the app. The core's
`amp_connections_url` and macOS `AmpConnections.url` apply the same site rules
(https, or http for a loopback host; an origin optionally followed by `/cloud`)
and return nothing for any other site.

GitHub Copilot (VS Code) detects VS Code (the macOS app in /Applications or
~/Applications; /usr/share/code or the Snap package on Linux; the per-user and Program Files installs on Windows; or `code` /
`code.cmd` on an absolute PATH entry) and the newest GitHub Copilot Chat
extension under `~/.vscode/extensions`, chosen by its version. A bundled
`extensions/copilot` is also recognized when its manifest identifies GitHub's
`copilot-chat`. Its page lists what Copilot needs and the detection result.
Setup uses a revocable provider grant from the current Agent Relay account.
On macOS the native harness's `~/.copilot/mcp-config.json` (`COPILOT_HOME`
replaces `~/.copilot`) selects the app-owned stdio MCP bridge; Core keeps the
grant and signs native calls at dispatch after the editor's normal approval.
The legacy configuration retains an http server under `servers` in VS Code's user
`mcp.json` (`~/Library/Application Support/Code/User` on macOS,
`$XDG_CONFIG_HOME/Code/User` on Linux, `%APPDATA%\Code\User` on Windows;
`VSCODE_PORTABLE` selects its `user-data/User`). On macOS the same choice
installs the owned native session/tool/stop hooks plus a separate no-op
pre-MCP observer, preserving the public four-hook schema and user hooks.
Linux and Windows retain HTTP in both configurations. The native Copilot server
uses `agentrelay`; legacy VS Code uses `relayhistory`. VS Code keys root MCP
enablement by name and can forward its global configuration into the native
harness, so separate names prevent a legacy HTTP import from replacing native
dispatch. Only `agentrelay` is accepted by native tool attestation. Repair
migrates recognized owned legacy entries, preserves foreign servers, and refuses
foreign `relayhistory` entries or an unowned legacy `agentrelay` shadow. Failed setup rolls back attempted writes before
reporting the error; uninstall cleans each owned file even if a sibling fails.
Verified messaging requires
Cloud's Copilot attestation capability for the exact signed-in account and
identity-sync generation; Linux and Windows report messaging unavailable.
History uses RelayHistory's native `copilot` reader and the existing automatic
upload and sharing policy. The older Local harness has no history or wake
adapter. The native runtime probes and the still-pending packaged end-to-end
acceptance are described in
[cursor-copilot-existing-session-injection.md](cursor-copilot-existing-session-injection.md).
`AGENT_RELAY_PROVIDER_DETECTION_ROOT` (absolute) moves the fixed macOS and
Linux install locations under another root, as
`AGENT_RELAY_PROVIDER_DETECTION_HOME` moves the home directory searched.

Provider setup uses the existing Agent Relay sign-in. “Setup not available yet”
means Agent Relay has not implemented the provider’s automatic setup; it does
not mean the provider lacks MCP support. Global installation, account
authentication and running-session verification are separate capabilities.

Provider details use one balanced column. The heading row puts the provider's
icon, name and status dot on the left and its one next action on the right,
named for what it connects or fixes (**Connect Codex**, **Fix Claude Code
connection**, **Sign in to Agent Relay**, **Check connection**), then More
options with Reinstall, Disconnect or Remove from sidebar. A one-line summary
appears only when it adds something, such as saved but unverified setup or an
unsupported provider. The top bar shows Providers without a subtitle. Settings
follow as aligned rows: **Local sessions › Open with**, and for Claude Code a
**Sharing** group whose **Claude Code folders** row summarizes how many folders
are shared and expands to the folder switches with their upload explanation,
beside **Claude Code cloud sessions** and its consent detail. **Connection
details** (diagnostics and the manual Check again action for local
integrations) and **Opening requirements** (CLI requirements, **Check installed
apps** and setup links) close the page collapsed; disclosures stay open or
closed across background refresh. **Add provider** lists providers not
installed here, each with a secondary **Add** button. Hosted/cloud providers
use their existing setup routes without a misleading account health button;
Capy's **Set up in Capy** takes the action slot. Passive status refresh leaves
actions and menus available, keeps newer action results, and does not show a
manual-check spinner.

Development cores with a `.dev` app-data directory use isolated provider
profiles under `mcp/clients` and stage credential helpers under `mcp/bin`.
Codex uses `~/.agent-relay-dev/<app-data-hash>/codex` for both `CODEX_HOME`
and `CODEX_SQLITE_HOME`; the short private path lets the native control socket
work on macOS and keeps the same isolated profile layout on Linux and Windows.
The helper explicitly targets that development core socket. Normal provider
credentials and settings are not copied; use the development profile when
starting test chats. Grok, OpenCode, Devin, Muse, Pi, OpenClaw and Hermes
also write only inside their development profiles. See the root README for the macOS test commands.

Session-opening setup opens the affected provider’s settings page directly. A failed
disconnect remains **Needs attention** after deselection; Connection details keeps
the failure visible so cleanup can be retried from More options.

Local capture uses the shared core's [incremental capture scheduler](capture-scheduling.md)
on macOS, Linux and Windows: exact dirty paths stay responsive while a fair,
paced historical iterator and shallow selected-only inventory make progress.
The core's `src/probe/history/roots.rs` refreshes retained capture roots from validated
folder choices. Its `src/probe/live/maintenance.rs` cooperatively interrupts
full audits for new captures or changed consent, preserving SDK checkpoints.
Cold timing reports distinguish elapsed slice time from measured SQLite lock
sleep; retries and partial filesystem watches retain paced recovery.
Session search uses the core's indexed local catalog and revision-pinned cursor
pages on macOS, Linux and Windows. **Next page** replaces the current working
set (at most 200 candidates and 256 KiB); an empty filtered page may still offer
**Next page**, since upload-state proof is evaluated on bounded candidates.
**First page** returns to current history. Cursors retain a bounded recent version window; after enough mutations or
database replacement, refresh starts over.
Unchanged-response tokens avoid transferring the same page on idle refreshes.

Reasoning effort remains visible for each page, but global reasoning sorting is
disabled: local history does not persist one session-level reasoning value.
The RPC rejects that sort explicitly rather than returning an incorrect order.

If upload proof cannot finish within one bounded attempt, **Retry page** resumes
before the unresolved row and skips the already-verified prefix. Filter menus
load catalog-wide facets through independent pages of at most 200 values/64 KiB.

Upload proof retries retain a private, candidate-page cache (at most four entries,
16 MiB each). The SDK also bounds retained member proofs. A family whose proof
cannot fit or finish remains explicitly checking with **Retry page** available;
it is never silently skipped by an uploaded-only filter. Catalog initialization
reads metadata once, then repairs changed identities; project-name expiry repairs
at most 32 names per request after the initial seed.

Opening or revealing the desktop requests a fresh local-session discovery pass.
The core returns cached rows immediately and publishes providers independently.
**Relay connected** identifies the relay connection; check each message’s delivery
status to confirm its local handoff. It does not assert that a provider ran a turn.

Native session titles are published as `session_title` roster metadata, refreshed after
renames and when existing registrations reconnect. A published objective keeps priority.
OpenClaw and Hermes gateway registrations identify an agent rather than one conversation;
their `session_scope: "agent"` prevents a misleading History summary link. Their relay
addresses and message delivery are unchanged.

What each agent is doing is derived, never reported by the agent: whether a turn
is running (Claude Code's registry status, Codex's rollout turns, a spawned
agent's harness), the repository and branch of its working directory (read from
git's files, cached 30 seconds), and a one-line *doing* made of the session's
title and the in-progress item of its own to-do list (Claude Code's task list or
TodoWrite, Codex's `update_plan`), at most 120 characters with invite links and
workspace keys left out. Only sessions shared with the workspace (included in
uploads, as every session on the relay is unless someone stopped uploading it)
say it. It is
published as `activity` roster metadata only when it changes, at most once every
two minutes for an agent and six times a minute for the computer, through the
same retried, paused-on-limit path as titles. `roster.list` and `roster.get`
carry it for live agents; this computer's agents carry the core's current value
without waiting for Relaycast.

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

**Native input from the spawning agent** (`spawn_native_input`, on by default)
lets the verified requester send DMs as native Codex or Claude user turns. Other
agents remain peers and tool approvals remain with the human owner. The switch
is shared across macOS, Linux and Windows; see [native spawn](native-spawn.md) for details.

**Approvals from the spawning agent** (`spawn_requester_approvals`), shown
below it while native input from the spawning agent is on, also lets that
requester answer its sessions' permission and approval prompts by 1:1 direct
message; the first answer, the owner's or the requester's, counts, and each
is logged with who answered. On in Agent Relay Dev, off in the release build
until the person turns it on; agents can only turn it off. See [approvals
from the spawning agent](native-spawn.md#approvals-from-the-spawning-agent).

## Terminal onboarding

Linux and Windows bare-core installations can complete account and provider
setup with `agent-relay-probe setup` (`.exe` on Windows), without GTK or
a display. The private binary owns this wizard; the public core continues to
expose only the shared bridge contract. The wizard calls existing account,
workspace, provider, sharing and health methods over the same per-user socket
or named pipe the apps use.

Device approval happens in a browser on any computer. Interactive terminal
prompts guide the remaining choices; `--json --non-interactive` emits
newline-delimited progress events and a final result. Pending consent reports
`action_required` with a public approval URL and device code (exit status 2).
`--wait` waits for approval or resumes an existing pending approval. An initial
interactive account authorization is still required for hosted MCP provider
setup. Omitted preference flags preserve existing choices; new connections
start with `selected` uploads.

The Linux install script installs the GTK-free private helper, runs it under
`agent-relay.service`, and prints its full-path wizard command. It never starts
terminal onboarding from the installer's piped standard input. `--start-service`
starts an existing Linux user unit when requested; the installer remains
responsible for service installation and updates. The Windows bare-core
installer owns its existing **Agent Relay (<account>)** logon task. Both use
the same setup command, while retaining the lower-level agent-driven setup
routes. See [Linux terminal setup](../linux/README.md#finish-setup-over-ssh) and
[Windows installation](../windows/README.md#install).

## Crates

| Crate | Owns |
|---|---|
| `agent-relay-node` (relay's `crates/relay-node`) | The public core, messaging only: discovery, delivery, spawning, the bridge. It works from a workspace key and runs with no account. It is not in this repository: `desktop-core` and `gtk` pin it by git to a relay commit. |
| `desktop-core/` | The private layer: `Account`, Cloud sign-in (browser sign-in in `browser_login.rs`; device sign-in comes from the core), auth health, provider credentials, `mcp_settings`, `mcp-headers`, Claude attestation, the terminal setup wizard, and session history (`src/history`: capture, the upload queue and RelayHistory transport, the session catalog and view models, retention, the uploader, session naming, and the Devin Cloud and Claude Code on the web sessions the catalogs list), and session locations (`src/session_locations`: `sessions.location`). Its `agent-relay-desktop-probe` binary ships as `agent-relay-probe` and runs both the core's commands and the collector's. It also holds the build-version helpers `gtk/build.rs` shares and the test fakes (`tests/support/`) both crates' integration tests use. |
| `gtk/` | The Linux and Windows app. It takes the session view models from `desktop-core`. |

`AccountPlugin`, `SessionPolicy`, `MethodExtension`, `SettingsExtension`,
`PluginBuilder` and `session_sources::SessionSources` are the seam: unsupported
internals, not a public API. Clients use the contract; the macOS app's copy holds
the pinned core's registry and the methods `desktop-core/src/contract.rs` adds
(`scripts/generate-contract.sh`, through the `agent-relay-desktop-contract`
binary). Both crates
share `target` through `.cargo/config.toml`, and `scripts/check-lock-parity.py`
keeps their lockfiles, the core pin included, together. Approvals from the spawning agent, and the
workspace plan, are off in a core with no account.

Codex’s managed MCP entry sets per-tool approval defaults for Relay inbox reads,
direct messages, conversation reads, status, joining and leaving. Explicit
server or tool approval choices and tool allow/deny lists survive repair and
account changes; the global Codex approval policy is unchanged. A bare Codex
CLI needs a restart to load config changes. A PostToolUse hook gives an
unregistered interactive chat the absolute local helper command to register
only itself after the person chooses to join and supplies a name. Merely loading
MCP tools never registers or uploads the chat.

The **Ask Claude chats to join** row appears only with **Only sessions I choose**.
It is hidden for both automatic upload policies, without changing its saved value.

On macOS, Claude cloud discovery retries a previous Keychain refusal before
the normal five-minute listing interval once a quiet credential read succeeds.
Denied quiet checks have a five-second cooldown and never show another prompt.
Development reloads keep the Sessions collector hardlinked to the signed core
helper, so Keychain approval applies to the same executable in both roles.

A named Claude `join_relay` call passes through the local attestation hook.
The hook never registers or renames a chat: it provides a command for that
exact local chat to run through normal command approval, disclosing that
joining shares the conversation. Once registered, the verified native join
uses the desktop identity. Any failed join is denied with recovery guidance;
it never silently substitutes a hosted identity.

Confirmed Cloud deletion also removes manual Relay registrations. Persisted
exclusions block automatic re-inclusion after restart, and only the confirmed
Cloud-deletion journal permits integration cleanup. Unknown or corrupt status
never grants cleanup permission. Mac and GTK deletion confirmations state
that the session leaves Relay.

Saved native-provider connections are checked using their own scoped Agent Relay credentials. The core confirms the MCP handshake and required search/messaging tools, caches proof against account/workspace and credential fingerprints, and bypasses the cache on Check connection. This verifies the configured connection; existing chats can still need a reload. Capy also checks the current workspace’s saved connection through its project-list API when no active thread is verified. Permission or network failures stay unverified.

The coding provider keeps the durable `muse` identifier but is displayed as **Muse Code**. **Muse** (`muse-assistant`) is the personal assistant at muse.ai and appears separately, with an Open Muse handoff. On macOS the installed assistant opens directly; Linux and Windows use its website. Automatic history sync and webhook delivery require separately verified Muse connector capabilities; they are not inferred from a saved login.

Muse shows a selectable, monospace setup prompt in onboarding and provider settings. **Copy setup prompt** copies it without opening another app. The separate **Open Muse** action opens the installed macOS app, or the website on Linux and Windows; the person pastes and sends the prompt. **Read setup skill** opens the published skill directly. No Accessibility access is used. The skill recommends scheduled polling and requires a Muse-compatible Relay polling registration before enabling a recurring task.

### Native session locations

`sessions.location` reports current and historical local hosts.
`sessions.open_plan` makes the opening decision in desktop-core for both Swift
and GTK: focus a verified target, open the exact provider link, resume a stopped
session in its last host, choose among locations, or explain an unavailable
capability. Native adapters revalidate process identity before acting and report
OS dispatch separately from verified selection.

Origins are stored in `session-locations.sqlite3`, independently of the disposable
history index. The store preserves the first observation and latest host across
core restarts. Claude desktop identity mapping, structured resume arguments,
platform capabilities, regression commands and native acceptance procedures are
specified in [session-navigation.md](session-navigation.md).
