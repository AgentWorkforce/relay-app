# Agent Relay for Linux

The app opens Sessions on launch and whenever the signed-in window is rebuilt
after an update or sign-in, matching macOS and Windows.

Settings → Relay has one row, **Continue agents from mobile**, on unless
turned off. It covers Claude Code (`mobile_claude_handoff`) and the other native
adapters (`mobile_native_continuation`): after machine ownership is verified for
your signed-in account, your messages from the phone continue a Claude Code,
Pi, OpenCode or Codex app session as your own turns with its permissions
unchanged, and tool approvals come to your phone. Until the claim holds the row
is off and says it is waiting for that. The switch writes both core settings;
older cores omit the row, and the `*_supported` capabilities disable it where a
native connection is unverified.

For Claude Code, use 2.1.310 or later in the 2.1 release series. Once the
computer is claimed, Agent Relay installs its plugin in each shared Claude
login folder on startup and on each discovery pass; a session already open
needs a restart or `/reload-plugins` once, and until then its messages arrive
the old way. Reading a conversation sends nothing. Claude must remain running.
Owner-only single-action approvals expire after five minutes; peers cannot
approve them. Claude logins may link `skills` to another private directory
owned by the same user; Agent Relay resolves that link but refuses writable
shared targets and links inside its managed plugin. Pi sessions with the
updated Relay extension, OpenCode sessions with a compatible Relay connection
and Codex app windows use their own native input paths; compatible Codex app
windows can send command and file approval requests for mobile-origin turns to
your phone, and other prompts stay on the computer. Keep it awake with the
session running; messages an agent already accepted may finish after you turn
the row off.

**Native input from teammates** (`teammate_native_input`, on unless turned off)
is under the same settings. A direct message from a teammate or their agent to
a Claude Code session whose plugin is loaded starts a user turn,
labelled `[Agent Relay native input — from @name …]` and, while relay messages
are hidden, shown as the same short pointer a peer message gets; its answer is not sent to your phone. Channel posts,
@mentions and sessions without the plugin stay on Claude Code's peer inbox,
tool approvals still go to you, and turning it off cancels a teammate's
message not yet started.
See [the shared flow and limits](../docs/workspace-layout.md). An unsupported
native runtime reports unavailable rather than changing session permissions.
Keep the computer awake with the session running. Messages already accepted by
an agent may finish after opt-out. Older cores omit the control.

Core diagnostics are saved in the app data directory's `logs/core.log`
(`~/.local/share/com.agentrelay.desktop/logs/core.log` by default, respecting
`XDG_DATA_HOME` and `--app-data-dir`). Credentials are redacted; the log rotates
at 1 MiB with three backups. The systemd user service also sends these
diagnostics to the journal (`journalctl --user -u agent-relay.service`).
Logging failures do not prevent core operation. Raw child output and request
or message bodies are not captured.

Codex delivery uses the shared core router on all platforms. Windows connects
to Codex's named pipe; Linux and macOS retain their existing socket, app-server
and queue routes.

The same GTK app also runs on Windows; Windows readers, see
[windows/README.md](../windows/README.md) for installing, building and what differs.

Existing Pi sessions can receive Relay messages without a Pi upgrade or
restart. The core installs the extension; run `/reload` once in Pi, then enable
**Receives messages** in the session table. Delivery waits for idle and confirms
the message in Pi’s history. See [Pi delivery](../docs/pi-existing-session-injection.md).

Pi queues preserve arrival order while busy, and channel batch answers reply to
the last post. A failed extension installation remains visible in status details
and Settings → Relay on both platforms; fix the conflict or permissions and
restart Agent Relay to retry, then run `/reload` in Pi.

Existing Oh My Pi (`omp`) sessions receive Relay messages the same way. Oh My
Pi loads extensions only when a session starts, so new sessions connect on
their own and one already open connects after `omp --continue`. See
[Oh My Pi delivery](../docs/omp-existing-session-injection.md).

Spawn recovery and startup diagnostics run in the shared core on both platforms.
Known spawn origins survive legacy store merges; Codex origins omitted by an old
writer can be restored from matching local receipts and Relaycast identity.
Failed launches preserve redacted harness reasons in durable completion receipts,
core logs and hosted status, including through older brokers that accept only
plain error strings. Ended intents retire after acknowledged cleanup. CLI failure
logs include argv, observed exit status and a bounded output tail; Codex's
app-server RPC has no per-launch exit status. OpenCode warns when model startup remains
unverified; Grok reports clearly missing default credentials as unavailable.

Replicas launches use Agent Relay Cloud through the same shared contract on
Linux and macOS: environment listing, durable spawn/status, and release.
Spawning remains an agent operation; no GTK-only button or background service
is added. The [existing operator bridge](../integrations/replicas/README.md)
can remain while the Cloud integration rolls out.

Cursor Cloud agents use that same shared core on Linux, macOS and Windows.
They are identified as `cursor-cloud`, separate from the removed local
`cursor-agent` target. Agent operations can list models and agents, launch,
inject follow-ups, inspect results, archive and restore; Cloud holds the Cursor
credential and routes Relay messages into the provider's native follow-up API.
There is no GTK-only launch form. See [Cursor Cloud agents](../docs/cursor-cloud.md).

Native spawning includes Muse through a detached Relay-owned terminal host.
Its startup task is a CLI argument and later messages use Muse's courier route;
Relay never types into the terminal. Muse's sandbox is on by default. The core
publishes the same harness settings and wording for the GTK and macOS apps.
The shared core records Muse's detached host before launching its child so a
restart during startup can recover or stop the exact owned process group.

Spawned Muse and Devin expose a per-launch MCP tool for Relay messages. It uses
the authenticated local session socket and accepts only the recipient, message
text, and optional reply parent. Shell sandboxing stays enabled by default;
Relay replies do not require granting shell access to the socket.

The native GTK desktop app for Agent Relay. It is a thin client of the relay
core, the `agent-relay-probe serve` daemon bundled with it
([relay core](../docs/relay-core.md)): every screen renders the core's state,
and every action is a bridge call. Sign-in, uploads, delivery into sessions,
messaging, telemetry and launch at login all run in the core.

The app follows the Mac app's local-first layout (see
[workspace layout](../docs/workspace-layout.md)). Startup opens the session list
for this computer: one table for running agents and historical sessions. Sessions
spawned through Agent Relay show their task as the session title and original
request, without the relay delivery header and report instructions. Relayed
messages use the same task text. A single header holds the Agent Relay logo
and canonical wordmark, **Search or filter sessions…**,
the icon-only **Table display** control, and the avatar menu with **Settings…** and **Log out…**.
**Ctrl+,** opens Settings. The search panel offers **Receives messages** and
**Does not receive messages** as mutually exclusive filters, displayed as readable
chips. They match confirmed registrations (including offline sessions) or their
absence and preserve other filters, grouping, and sorting. Removing the chip
includes both message states again. Sessions stay current automatically.

- **Filters:** activity, message reception, harness, upload status and updated dates live in one
  popover. Choosing Active adds `is:active` to the visible search; running
  sessions have an Active label in their row. Combine `title: Foo`,
  `project:cloud`, `harness:codex`, `upload:uploaded`, and
  `updated:>=2026-10-01`. Quoted phrases, exclusions and inclusive date ranges
  (`updated:2026-10-01..2026-10-11`) work too. Dates use UTC. Search help lists
  the syntax, and Clear resets search and filters together. All filters apply
  to the same list. The header logo returns from a conversation,
  preserving the query and grouping. As on macOS, Filters stays open while
  combining choices and during background refreshes; Done, Escape, or a click
  outside dismisses it. Group by closes after choosing a grouping. Both apps
  share fixtures for search results and menu edits to the query.
- **Sessions:** local history joined to local relay discovery, with a **Receives messages** status orb
  and an **Upload** toggle. Cloud-style striped rows group by
  None, Harness, Project, Status, or Week, with tinted headers and counts.
  Labels prefer RelayHistory's harness-provided catalog title, falling back to
  prompt text when no title is available, matching macOS.
  Each row's second line names its agent once registered, then its model,
  harness, reasoning effort and working folder. Pi sessions and agents draw
  Pi's block mark wherever harness marks appear.
  Project names come from local Git repository metadata when available, so
  linked worktrees and nested directories group under the repository name.
  Relay broker controller roots show no project instead of the machine-node
  folder; other non-repository directories retain their folder-name fallback.
  Grok Bot sessions show their roster names and individual profile avatars,
  using metadata supplied by RelayHistory. Native shapes/colors use the same
  idle animation as macOS, with animation disabled by reduced-motion settings.
  Missing artwork falls back to the Grok Bot app icon. Each native chat keeps
  its own row and UUID for identity.
  Search and harness/upload-status filters narrow the loaded list; older history
  loads invisibly as you approach the end of the list. Discovered sessions awaiting
  upload indexing remain visible, labeled Indexing. Other machines and teammates are viewed in Cloud.
  As on macOS, a running Codex, Claude Code, Grok, OpenCode, Muse or Devin
  session on the relay is one row with its transcript, joined by the core's
  upload key, so its transcript shows its real messaging state; putting one on
  the relay also uploads it. Registered Grok CLI sessions use Grok's title.
  Claude app, OpenClaw, Hermes, Pi and Oh My Pi relay rows are listed on their own.
  As on macOS, Grok sessions explicitly marked as subagents stay in local
  history but do not appear as separate rows, even without a recorded parent;
  refreshing the catalog also corrects previously cached sessions.
  Previously selected Grok subagents are removed from independent upload
  selection before delivery; children with recorded parents share with them.
  As on macOS, Grok Bot (the desktop app, a `grok-bot` source separate from the
  Grok CLI) lists one session per bot from the app's local transcript tail
  (about the last 200 entries or 7 days), and a Codex thread whose rollout file
  was pruned is recovered from Codex's SQLite history; such a thread moves with
  its catalog row, like a session in a shared database.
  OpenClaw (an `openclaw` source, RelayHistory 0.49.0) lists each OpenClaw
  agent's sessions, its main chat, channel chats, Agent Relay conversations,
  cron and subagent chats, read from `~/.openclaw` (or `OPENCLAW_STATE_DIR`) on
  every OpenClaw from 2026.3 (its agent database from 2026.8.1, its JSONL
  session store before), one row per conversation: every heartbeat or cron run
  of a key is part of that row, and follows the session-sharing mode as on the
  Mac. Antigravity (an `antigravity` source) and Oh My Pi (an `omp` source)
  are listed too, from `~/.gemini/antigravity*/brain` and `~/.omp/agent/sessions`,
  as on the Mac.
- **Session titles:** open a session and choose the quiet **Rename** button
  beside its transcript-header title, matching macOS and Windows. The title
  edits in place with the same typography and no input border or background.
  The table has no rename controls and does not preload title suggestions.
  Enter or **Done** saves; Escape or **Cancel editing** cancels. Moving focus
  outside the entire editor saves after a short debounce, allowing movement
  between its text and buttons without submitting. Unchanged titles do not
  write, and typing does not send requests. **Use generated title** fills the
  draft and refocuses it without saving. Opening the session preloads its saved
  pipeline title, with bounded reads and a cache scoped to harness and session;
  neither preloading nor the sparkle button starts generation. The sparkle
  appears while editing once a saved title is available. Native support comes from
  `sessions.title.capability`, and saving calls `sessions.title.set` to update
  the original harness. Unsupported sessions explain the limitation in the
  Rename tooltip. Saves appear immediately and roll back with an error on
  failure, preserving the rejected draft for retry. Reconnecting retires
  interrupted title requests; changing accounts clears cached suggestions and
  edits. Leaving the session cancels delayed saves. A confirmed optimistic title
  expires after at most one minute and refreshes from the source, so later
  harness changes remain visible. Editing never sends an agent prompt or
  changes the Relay messaging name.
- Provider setup appears in the post-sign-in chooser and Settings › Providers,
  without Capy or Devin banners above Sessions. **Set up in Capy** opens a new
  Capy thread prefilled with the Agent Relay setup skill prompt and copies it
  as a fallback. Capy uses this action in the chooser instead of a disabled checkbox.
- Grok, OpenCode, Devin, Cursor and Codex session messaging controls live on
  their respective provider pages, along with OpenClaw and Hermes gateway
  settings. Cursor appears in the sidebar when `~/.cursor` is present.
- Starting Capy threads is an agent operation, so the account and Agents/Team
  overflow menus omit **Start Capy thread…**, as on macOS. The relay core keeps
  its Capy project, spawn and binding-status methods available to agents. The
  workspace Capy key stays in Cloud.
- Detailed upload stages and journal compaction live under **Settings → Session
  sharing → Upload details**. The bottom bar shows the total indexed sessions on this machine.
  When a problem that is not retried on its own stops uploads, session rows read
  **Upload stopped** and the upload stage offers **Retry uploads**.
  Uploads that keep failing on a server error or a dropped connection read
  **Waiting to retry** while they wait out a backoff of at most five minutes,
  and the upload stage offers **Retry uploads** too; a rate limit only waits.
  A Retry or Compact that could not reach the background service is sent on
  its own once the service answers again, and its message clears.
  **Live Agents on Relay: N** opens Sessions with `is:active is:on-relay`, replacing
  previous search filters and returning from any open transcript.
- **Settings:** General (appearance, open at login, keep agents connected after quitting, privacy, how long session history is kept, and service recovery when needed), Updates (software updates),
  Diagnostics (log files and expandable, copyable diagnostic reports), Messages (connect the account workspace or
  paste a key, Claude Code direct delivery, Grok leader mode, the OpenCode plugin, OpenClaw agents (any OpenClaw from 2026.3.13; one older
  than 2026.8.1 cannot limit relay sessions, which the row says; with **Agent Relay tools** under it while it is on:
  **Connect** opens the hosted Agent Relay MCP's consent page in the browser, as on the Mac) and Hermes
  ([openclaw-hermes-delivery.md](../docs/openclaw-hermes-delivery.md)) with their trusted senders (the
  people in the workspace, listed read-only under **Trust people in your workspace**, plus names added by hand),
  the Devin hooks, the Cursor hooks (**Cursor sessions**, as on the Mac),
  self-registration, spawned agents, incoming messages to Codex threads), Session sharing and Account.
  Account's **Workspace** section adds the plan row the Mac shows when Cloud
  reports one (**Intro plan**, its caps against what the workspace uses, and
  **Manage plan**); a session a plan cap kept off the relay names the cap, in
  the Mac's words.
  **Spawned agents** lets Agent Relay start agents here, headless, as on the Mac
  ([native-spawn.md](../docs/native-spawn.md)), with the same **Allow teammates to spawn
  here** switch (and the claim under it), an optional **Max spawned agents**
  number field. Full-access switches live on each provider’s settings page for
  the harnesses the core describes and the node offers. Leave the cap empty for **No limit** (the default), or set
  1–32. The old untouched default of 12 becomes unlimited; explicitly saved caps
  remain. Clearing the field sends `spawn_max_agents: 0`, Relaycast’s documented
  unlimited sentinel, matching macOS. Save with **Save** or Enter; leaving the field or refreshing Settings does not apply an unfinished edit. Failed saves show the error and restore the confirmed cap.
  A spawned agent's first message names the agent that asked for it and how to
  report back over the session socket; a spawned Claude or Codex that sends
  nothing has its first turn's final answer sent to the requester
  ([native-spawn.md](../docs/native-spawn.md#reporting-back-to-the-requester)).
  A spawned Codex or Claude takes the signed-in machine owner's messages as
  plain user turns; other agents, its requester included, stay peer-framed.
  Its approvals (Codex's app-server requests, Claude's permission prompts)
  reach only the owner as Relay DMs; replies use a short reference and expire
  to denial. A spawned Claude that has ended is never resumed: a message to it
  is not delivered, and its sender is told the session has ended.
  Devin uses a persistent private ACP host, remains messageable while idle, and defaults
  to accept-edits. A launch-specific MCP server permits only Agent Relay sends
  through the authenticated local session socket; other approval requests are refused. It requires `devin auth login`
  when `devin auth status` reports signed out. Each runs in a systemd scope of its own, so restarting or updating
  `agent-relay.service` leaves it running. Codex is offered only when its
  app-server daemon can run here: a Codex installed only through npm or a
  version manager such as mise cannot start one, so install Codex with its own
  installer to spawn it (see native-spawn.md). Follow-up messages resume a
  native Codex spawn's unarchived thread if Codex unloaded it while idle;
  ordinary stopped sessions still wait for their host.
  OpenCode is offered for OpenCode 1,
  whose server the spawn drives; its server runs in a scope of its own too.
  Grok is offered from version
  1.0.46; an older one is refused with a reason that says to run `grok update`.
  Spawned OpenCode replies use the session socket: the spawn loads Agent
  Relay's OpenCode plugin into that private server itself, whether or not it
  is installed globally, and never writes to `~/.config/opencode`; the core
  verifies its process and sealed binding, and never identifies a managed
  caller from a session tag alone.
- Agents lists running sessions only, with the Mac's harness marks (Capy
  threads on the relay show the Capy mark, run in the cloud and group under
  Capy); a
  registered session whose process ended keeps its name and queued messages
  off the list until it runs again, and Grok sessions are listed from Grok's
  own live-session registry and bound to Grok's leader when added; with
  automatic activation on, one whose leader runs is added by itself, is
  rebound when Grok restarts its leader, and leaves after the grace period
  when it cannot be rebound. Claude Code
  direct delivery and Grok leader mode are on by default wherever those tools
  are installed and the person has not set them, from sign-in on. Existing
  choices remain in effect; provider setup can be managed after sign-in.
  The welcome screen has no tool-connection checkbox, on Linux, macOS or Windows.
  Live Devin CLI sessions are listed too: each `devin` process proves its
  session by the `session_locks/<id>.lock` it holds open, a session can
  register itself from inside its own process tree, and a relayed message is
  written as a `session/prompt` frame into a terminal session's `acp` stdin
  through `/proc` — immediate, even while the session sits idle — or, when no
  such pipe is reachable (a Devin Desktop session's `acp` speaks over a
  socketpair), staged into `~/.local/share/devin/relay/inbox.d/` for the next
  hook boundary. The *Devin sessions*
  row (`devin_direct_delivery`, offered in Sessions where Devin has run, never
  turned on by itself) adds or
  removes Agent Relay's lifecycle hooks in `~/.config/devin/config.json`;
  the hooks drain staged messages into the conversation at each boundary —
  a session started before they were on fires none and must be restarted.
  When the Devin CLI is signed in, the inventory also polls the org's Devin
  cloud sessions (at most every five minutes, through the `devin-cloud`
  connector) so they appear alongside the local ones. They are listed in
  the sessions table too, hosted by *Devin Cloud*: one auto-activates like
  a local session while the sign-in resolves and the session was active in
  the last 24 hours; an older one is still listed and can be added by hand,
  but does not join the relay by itself, and an automatic one leaves it.
  A relayed message to a cloud session — any Devin session not running
  locally — is posted into it through the connector, which needs the
  `ManageOrgSessions` scope; the send's result is reported once and never
  repeated, since the Devin API cannot deduplicate a reposted message.
  With a Cursor API key (`CURSOR_API_KEY`, or the first line of
  `~/.config/relayhistory/cursor-api-key` for the background service, which
  does not read your shell environment) Cursor Cloud Agents are listed and
  their conversations synced through the `cursor-cloud` connector on the same
  schedule. Their `bc-` ids keep them apart from local Cursor sessions.
  Live Cursor CLI (`cursor-agent`) sessions are listed too, labelled
  *Cursor CLI*: each `cursor-agent` proves its conversation by the
  `~/.cursor/chats/<workspace>/<id>/store.db` it holds open, a session can
  register itself from inside its own process tree, and a relayed message is
  staged in `~/.agentworkforce/desktop/cursor/inbox.d/`. Conversations of
  the Cursor editor's agent are listed and delivered to the same way, hosted
  by *Cursor*, once one of their tool calls ran Agent Relay's hook under the
  window's extension host. The *Cursor
  sessions* row (`cursor_direct_delivery`, never turned on by itself) adds or
  removes Agent Relay's hooks in `~/.cursor/hooks.json`, which hand the
  message over at the session's next tool call or when its turn ends. A
  Cursor editor chat that has finished its turn gets it as soon as it
  arrives, for up to an hour; an idle Cursor CLI session gets it during the
  next turn its person starts. The waiting editor hook has been run live on
  macOS only
  ([cursor-copilot-existing-session-injection.md](../docs/cursor-copilot-existing-session-injection.md)).
  Copilot CLI is not delivered to yet.
  Claude Code cloud sessions on claude.ai are listed and uploaded the same
  way (the `claude-web` connector, at most every five minutes) once
  **Claude Code cloud sessions** is turned on under *Claude Code folders*,
  with the login in `~/.claude/.credentials.json`, never refreshed or
  changed; an expired one is not used. They go with the `~/.claude` folder's
  Shared switch. On the Mac the login can also come from Claude Code's
  Keychain item, so its row also explains macOS's prompt; Linux has none.
  While the setting is on, the row shows whether the source is working,
  checking, blocked or needs attention, its last successful pass, and the
  plain cause and fix. Failed passes and helper failures are also retained in
  the credential-redacted `core.log`.
  Those active in the last seven days are also listed under **Agents** as
  *Claude Code on the web*; one turned on there receives messages through
  `claude --print --cloud` and answers through the hosted MCP
  ([integrations/claude-code-web](../integrations/claude-code-web/README.md)).
  Claude Code sessions on a second Claude login (`~/.claude-<name>`,
  `CLAUDE_CONFIG_DIR`) are listed too, and direct delivery applies to every
  login, as on the Mac. *Claude Code folders* in Settings › Relay lists every
  Claude Code config folder whose sessions are found and uploaded: the
  default, sibling logins, `AGENT_RELAY_CLAUDE_CONFIG_DIRS`, folders added with
  *Add Folder…*, and any `CLAUDE_CONFIG_DIR` a running Claude Code session
  uses (such as `~/.seclaude`). Turning a folder's switch off keeps its
  sessions off the relay and out of uploads. Wherever a Claude login directory
  exists, *Ask Claude chats to join* (`claude_session_hook`, on by default
  once after sign-in, preserving a saved Off choice)
  adds or removes the same SessionStart hook in each of those logins'
  `settings.json`. In an unregistered session the hook has Claude ask you
  whether to put it on the relay and under what name, and POST /register only
  if you say yes. With the hook come `permissions.allow` rules in the same
  `settings.json` (recorded in `claude-permission-rules.json`) that let the
  session run Agent Relay's own read and reply commands without asking, and,
  when the probe's path needs quoting, a link at
  `~/.agentworkforce/desktop/bin/agent-relay-probe` they name. A session already on the relay gets a short preamble with its
  relay name, how to reach peers and the messaging norm. The hook never
  registers it.
- Relay & messages includes the Mac's **Claude Code message approvals**, collapsed
  by default. Expand it to choose direct delivery or **Ask first** per session,
  manage the optional allowed-members restriction, and use held-message actions **Approve once**, **Always allow**
  and **Dismiss**. The core saves these choices before using Claude Code's
  inbox, independently of the login-wide `crossSessionInbound` setting.
- As on the Mac, when Claude Code itself will hold a message for approval (a
  session that skips permission prompts with `crossSessionInbound` not
  `accept`, or `crossSessionInbound` set to `hold`), the composer's note under a message you sent reads **Held for approval in
  Claude Code** until the session receives it. After `/clear` in a Claude
  Code session on the relay, its agent moves to the new conversation in the
  same terminal; messages sent before the clear are refused, not delivered
  into it. Before writing to a Claude Code inbox the core checks, with
  `SO_PEERCRED`, that the session's own process is listening there.
- The Linux UI understands the shared `paseo` host value, labels it "Codex in
  Paseo" and carries Paseo's badge on the Codex mark. Current automatic Paseo
  process discovery is macOS-only because its proof is the packaged
  `Paseo.app` helper ancestry; this applies to both Codex and top-level Claude
  Code chats. No equivalent Linux package contract is known.
- Not on Linux: the Conductor badge on Codex and Claude Code sessions that run
  in a Conductor workspace. Conductor is a Mac app, so a Linux machine has no
  Conductor sessions; the core's Conductor handling (listing its Codex
  sessions as live, their titles, `host` on session rows) is shared and simply
  never applies here.
- Settings → Relay has **Show me relay messages in full**
  (`show_relay_messages`, off by default), as on the Mac: relay messages are
  hidden from Codex app chats, and a terminal Codex or Claude Code session,
  which only accepts visible text, shows a two-line pointer naming the sender
  and the packaged probe's `read <ref>`, which the agent runs to read
  the message. The agent still gets each message in full and can reply;
  the setting only changes what you see.
  The message's authority note lets work already authorized in the receiving
  session continue within the harness's permissions. A peer's approval claim
  grants nothing new; Claude's native peer restrictions still apply. The note
  is shared with macOS and Windows.
- A terminal Claude Code session holding relay messages it has not read (its
  turn stuck) for 10 minutes with no progress gets a warning in Activity and
  `inbox_stall` in `agents.list` (`inbox_stalled` on `GET /agents`), as on the
  Mac; `RELAY_DESKTOP_CLAUDE_INBOX_STALL_SECS` changes the 10 minutes. The core
  is shared, so the watch, its recovery after a restart and the wording are
  the same; see `docs/workspace-layout.md`.
- Settings → Relay has **New agents join #general** (`auto_join_general`, off
  by default) and, once an agent has changed a setting on the session socket
  (`PATCH /settings`), **Changed by agents** with the latest changes, as on
  the Mac. Turning on or adding to a setting that widens who can reach or act
  as you (trusted senders, automatic activation, direct delivery, spawning,
  delivery routes) stays yours to do in Settings: an agent's request is
  refused with `person_only_setting`.
- Not on Linux: putting Claude desktop app chats on the relay automatically.
  Claude's desktop app has no Linux build, so the shared check (a
  `claude-desktop` session under this user's `Claude.app`) never matches here.
- Settings → Relay has **Codex mid-turn delivery** (`codex_stop_hook`, on by
  default wherever Codex is installed), as on the Mac: Agent Relay's Codex
  hook in `~/.codex/hooks.json` hands a message to a Codex session that is
  working at its next tool call or as its turn ends, instead of after the
  turn. With it, `~/.codex/rules/agent-relay.rules` lets Agent Relay's own
  read and reply commands through Codex's sandbox without asking. Codex
  sessions here keep the curl reply line; the rules cover the pointer read. See [injectable agents](../docs/injectable-agents.md). Its Conductor
  chat handling never applies here: Conductor is a Mac app.
- Sign-in in the browser sharing only the sessions you choose, as on the Mac
  (nothing uploads until a session's Upload switch is on). The sign-in panel shows
  the Agent Relay logo followed by **Continue with Google** and
  **Continue with Microsoft**; each opens the chosen provider's account chooser. An
  account with no workspace yet gets **Open dashboard** (Cloud's Workspace
  settings, where it creates one) and **Try again**, as on the Mac; with
  direct-delivery preferences in Settings; while connected without a Cloud
  session the core may use, a notice under the header reads **Agent Relay
  needs you to sign in again.** with **Sign in again**, as on the Mac; Messages with
  several conversations, each saying whether its recipient runs (**Queue
  Anyway** for one that does not), what Relaycast holds for the last message
  sent and whether a reply is expected; a tray menu counting sessions receiving messages and local only, and
  uploaded and not, each opening the session list with that search, with
  All sessions, Settings and Quit controls; `agentrelay://connect` and `agentrelay://message?to=agent%40machine`
  links.

Codex app-server and desktop-window messages stay pending while the current
turn runs. After it finishes, delivery starts a separate turn; peer messages
and incoming cloud messages use the same idle check on macOS and Linux.
Waiting does not consume a send attempt. Live terminal sessions continue to
use Codex's own queue, which waits for the current turn to finish. Conductor's
existing Stop-hook delivery remains a separate host-specific route.

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

## Controls answer at once

Every control that changes the core (the upload policy, pause and resume,
retry and compact, Settings switches, launch at login, renaming an agent or
putting a session on the relay under a name, sending a message, Codex thread
sharing and held-message approvals, Claude Code delivery choices) shows the
change the moment it is made, as on the Mac: the intent is kept beside the core's last answer and drawn over it until the core
confirms it. A refused change goes back to what the core reports and its
reason shows beside the control; clicking again while a change is still
saving makes the last click win. `docs/workspace-layout.md` describes the
pattern and `gtk/src/models/pending.rs` holds the shared state.

## How it runs

The app is a Rust binary: the app crate's (`gtk/`) `agent-relay` built with its
`gtk` feature (GTK 4.6 or later, no libadwaita, a StatusNotifierItem tray),
installed as `agent-relay-desktop` so it never shadows the npm `agent-relay`
CLI.

- **With a display:** `agent-relay-desktop` opens the app. It connects to the core on
  `~/.agentworkforce/desktop/core.sock` (`AGENT_RELAY_CORE_SOCKET` names
  another). When no core answers, it starts the `agent-relay.service` systemd
  user unit if the person enabled it; the unit keeps agents connected and
  uploads running after the app quits and starts it at login, as the Mac's
  LaunchAgent does, and Settings › General offers to turn it on if needed. An
  enabled unit owns the socket: when systemd cannot confirm its start, the app
  waits for it and never starts a second core beside it. Otherwise the app
  starts its core as its child. **Keep agents connected after quitting** (on
  by default, as on the Mac) leaves that core running when the app quits, and
  the next launch connects to it; one left running from before an update is
  stopped and replaced. Turned off, quitting stops the hosted core with
  `daemon.shutdown`, or stops the running unit, without disabling its next-login
  registration. **Settings › General › Open at login** controls both the tray
  app and the systemd unit: on schedules both, off schedules neither, leaving
  current uploads alone. The headless service preserves the GUI autostart entry.
  Upgrading preserves either existing startup opt-in and reconciles the two
  entries once; a failed migration shows the enabled choice and a repair notice.
  On desktops without systemd or an installed unit, the GUI startup entry still
  works and the app starts its hosted upload core. A scheduler read failure is
  reported rather than silently assuming the service is absent.
  With keeping agents connected off, **Quit Agent Relay** asks **Quit and take
  your agents offline?** and offers **Keep Agents Connected**, which turns the
  setting back on and quits, beside **Quit Anyway** and **Cancel**.
  `--background` starts in the tray.
- **Headless (SSH, tmux, servers):** the systemd user unit runs the core
  directly, with no GTK:

  ```sh
  systemctl --user enable --now agent-relay.service
  /usr/lib/agent-relay/agent-relay-probe call status.get --json
  ```

  `agent-relay-desktop --headless` runs the same core in the foreground. Headless
  startup applies the agent-led defaults: Claude Code direct delivery,
  automatic activation of the sessions the upload policy shares, and uploads of
  only the sessions you choose (sharing mode `selected`), so nothing uploads and
  nothing joins the relay by itself until a session is chosen. An install
  that already has a sharing mode keeps it.

However it runs, the core starts its session collector (`agent-relay-probe
run`), which reads local sessions, lists them and uploads the ones being
shared, at nice 19, so it yields to everything else; the core itself, and the
agents it starts, keep normal priority. Unlike the Mac's *Agent Relay Helper*
and *Agent Relay Helper (Sessions)*, both keep the name `agent-relay-probe` in
process lists: it is the installed file the service unit, the updater and
agents' commands name.

The app runs the first core of: `RELAY_DESKTOP_PROBE` (when non-blank),
`agent-relay-probe` beside its executable (a cargo build), and
`usr/lib/agent-relay/agent-relay-probe` in its install tree.

The window is `gtk/src/ui`; everything it runs on is the rest of
`gtk/src`:

| Module | What it is |
|---|---|
| `ui/app.rs` | The application: lifecycle, core events, sign-in, links, the tray's actions, dialogs |
| `ui/sign_in_actions.rs` | What the sign-in screen offers when an attempt needs the person to act: Continue anyway, or Open dashboard and Try again |
| `ui/state.rs` | What the window shows, as data: the screen, navigation, the core's answers |
| `ui/shell.rs` | The global header, settings sidebar, page heading and status bar |
| `ui/relay.rs` | The listings every surface shares, and switching sessions on the relay |
| `ui/settings_page.rs` | Every Settings pane, the background service and software updates |
| `ui/pages.rs` | The Sessions and Messages pages' roots in the page stack |
| `ui/rows.rs` | Pointer handling and context menus shared by list rows |
| `ui/sessions_page.rs` | Local session table, Cloud-style grouping and filters, relay/upload controls; upload diagnostics used by Settings |
| `ui/transcript.rs` | A local session's transcript and its sidebar |
| `ui/spawn_join.rs` | A spawned agent's live transcript and message box, and opening its session |
| `ui/spawn_end.rs` | Ending a spawned agent after confirmation and confirmed harness shutdown |
| `ui/spawn_take_over.rs` | Taking a spawned Claude over: stopping it headless and resuming it in Terminal |
| `ui/core_settings.rs` | Reading and changing the core's settings for Settings and the setup cards |
| `ui/messages.rs` | Messages (open conversations, the recipient's state, Queue Anyway, what Relaycast holds for the last message, whether a reply is expected) |
| `ui/widgets.rs`, `ui/widgets/popover.rs` | Shared rows, banners, settings rows, avatars, marks, dots and popover lifecycle |
| `ui/theme.rs` | The light and dark palettes and following the desktop's color scheme |
| `ui/tray.rs` | The tray menu, served as a StatusNotifierItem |
| `ui/desktop.rs` | Opening links (`xdg-open`), notifications, dialogs and quitting |
| `bridge.rs` | The bridge client: NDJSON JSON-RPC, the handshake, events, pooled calls, reconnecting |
| `host.rs` | Starting the core as the app's child when no other answers |
| `service.rs` | The `agent-relay.service` systemd user unit |
| `desktop-core/src/install.rs` (`agent_relay_app::install` re-exports it) | The install tree and which core the app runs |
| `updates.rs` | Checking for, verifying and installing releases; the app launcher entry |
| `prefs.rs` | The app's own preferences (`~/.config/agent-relay/desktop.json`) |
| `models/` (session rows, filters and upload states are in the core's `src/session_model/`, re-exported here) | Wording, states and filters, ported from the Mac's `RelayDesktopCore` |
| `fake_core.rs` | An in-process fake of the core, for tests and `agent-relay-fake-core` |

GTK owns the main thread and every widget; the bridge and the tray run on a
tokio runtime, and each answer is awaited on the GLib main context, so the
window's state is touched only on the main thread. The artwork in
`gtk/resources/linux` and `assets/agent-relay-wordmark.svg` is embedded in
the binary.

Updates: a per-user install under `~/.local/lib/agent-relay/<version>` updates
itself (download, SHA-256 check against the release, unpack beside the running
version, repoint the systemd unit, restart). A `.deb` install downloads the
verified package for the system package manager.

**Settings → Updates → Include beta releases** opts into numbered beta builds,
with the same wording and behavior as macOS. It is off by default, even in a
manually installed beta, and persists in `desktop.json`. Beta users receive the
newest version across beta and stable releases; stable users use GitHub's
`/releases/latest`. Changing the setting clears the pending update and checks
immediately. Turning it off keeps the installed version until a newer stable
release is available. GitHub releases use `Beta vYYYY.MM.N-beta.NUMBER` and
`Stable vYYYY.MM.N` titles, prerelease flags, and links to filter each track.
Debian package metadata uses `YYYY.MM.N~beta.NUMBER` so the final stable package
sorts after its betas; download filenames retain the `-beta.NUMBER` version.

App launcher: a per-user install lists itself in the desktop's app launcher
with `~/.local/share/applications/agent-relay.desktop` (under
`$XDG_DATA_HOME` when set) and its icon. The entry runs the install's
`usr/bin/agent-relay-desktop` by its full path. An update writes it for the
new version, the app refreshes it whenever a per-user install starts, and
`agent-relay-desktop --install-launcher` writes it without opening the window,
for a headless or scripted install. Started from the `.deb`, the app removes
the per-user entry a former per-user install left, which would hide the
package's own. An entry not written by the app is left alone. Launch at login
follows the same way: every start of an installed app points an app-written
`~/.config/autostart/agent-relay.desktop` at the running app, changing only
its command, so an entry turned off in the desktop's startup settings stays off.

The shared probe automatically recovers a rotated Cloud workspace key when
Relaycast rejects it: refresh the app's retained Cloud session, resolve the same
workspace and origin, and verify the replacement with one safe read before saving
it. Sign-out or a changed workspace cancels recovery. Failed writes are not replayed.
macOS uses this same core behavior.

## Headless install or update

On a server, or any machine without the app, one command installs the
GTK-free core for the current user and runs it as `agent-relay.service`; run
it again to update:

```sh
curl -fsSL https://github.com/AgentWorkforce/relay-desktop-releases/releases/latest/download/install.sh | bash
bash linux/install.sh --version 2026.10.34   # a given release
bash linux/install.sh --rollback             # back to the previous one
bash linux/install.sh --dry-run              # say what would change
```

It finds what is installed (its own `~/.local/lib/agent-relay/current`, the
app updater's per-user layout, or the `.deb`, which it leaves to apt), downloads
`AgentRelay-<version>-Linux-<arch>-probe.tar.gz`, checks it against the
release's `.sha256` and that the binary reports that version, installs it
(with the archive's `LICENSE`) under `versions/<version>`, and switches
`current` to it. It then restarts the service and waits for the core to
answer with the new version, switching back (and dropping the failed version)
if it does not, or if systemd refuses to start it. Moving over from the app
updater's layout keeps that core as a version, so `--rollback` can go back to
it. Three versions are kept. Settings, sign-in and agents live in
`$XDG_DATA_HOME/com.agentrelay.desktop` and are never touched; spawned agents
keep running through the restart. It never runs anything as root and does
not enable lingering: on a server it prints the `loginctl enable-linger`
command to run once. Releases carry a checksum, not a signature, so the
checksum proves the file is the published one, not who published it.
`scripts/test-linux-install.sh` runs it against a fake release server.

Windows has the same command as `windows/install.ps1`: it installs the bare
core under `%LOCALAPPDATA%\Programs\AgentRelay\core` and runs it as the
**Agent Relay (<account>)** logon task in place of the systemd user service,
with `-Version`, `-Rollback` and `-DryRun`; see
[windows/README.md](../windows/README.md#install). Both bare-core installs can
finish setup with the terminal wizard below. The lower-level
[agent-driven setup](../docs/agent-driven-setup.md#scriptable-windows-installation)
routes remain available to coding agents.

### Finish setup over SSH

After installing, run the terminal wizard as the same user who runs your agents:

```sh
"$HOME/.local/lib/agent-relay/current/agent-relay-probe" setup
```

The running core also maintains `~/.local/bin/agent-relay-probe`. Once that
directory is on `PATH`, the shorter command is `agent-relay-probe setup`.
For a `.deb` install, use `/usr/lib/agent-relay/agent-relay-probe setup`.
The full installed path works while the managed command-line copy is still being
prepared. The `agent-relay` command remains the separate npm CLI.

The wizard signs in with a device code, selects the workspace and providers,
sets the upload policy, and checks the connection. Open its verification URL
and approve the displayed code in a browser on your laptop, phone, or any other
computer. The Linux server needs no display, GTK, local browser, or SSH port
forwarding. The installer prints the next command instead of starting the
wizard from `curl | bash`, where standard input belongs to the installer.

An existing sign-in and preferences are kept when their options are omitted.
A new connection starts with **Only sessions I choose** (`selected`). To choose
providers and upload policy explicitly:

```sh
agent-relay-probe setup --providers codex,claude --sharing selected
```

For automation, request JSON and suppress terminal questions:

```sh
agent-relay-probe setup --non-interactive --json --providers detected --sharing selected
```

JSON output is newline-delimited JSON: progress events followed by a final
result. When first sign-in needs approval, it reports `action_required` with
the public verification URL and device code and exits with status 2. Add
`--wait` to wait while a person approves from another browser, or rerun with
`--wait` to resume the pending approval without opening another login. Authentication still requires that approval;
passing a service token is not a replacement for the account consent needed
by provider MCP setup. Run `agent-relay-probe setup --help` for the
complete options.

| Option | Effect |
| --- | --- |
| `--workspace UUID` | Select the Agent Relay workspace explicitly. |
| `--providers detected` | Choose detected providers that support setup. |
| `--providers codex,claude` | Set an explicit local provider selection. |
| `--providers none` | Clear the local provider selection explicitly. |
| `--sharing selected\|new\|all` | Set the upload policy explicitly; omitted keeps the saved policy. |
| `--json --non-interactive` | Produce machine-readable progress without terminal questions. |
| `--wait` | Wait for approval and connection progress. |
| `--timeout SECONDS` | Limit waiting to 600 seconds by default, at most 3600. |
| `--socket PATH` | Use an existing core socket. The Agent Relay site is the one the core was built for (`AGENT_RELAY_SITE_URL` when it was built); nothing at run time changes it. |
| `--start-service` | Start the existing Linux `agent-relay.service` user unit when needed. |
| `--acknowledge-uninspected-schedules` | Acknowledge legacy scheduled uploads when sign-in requests it. |

The headless installer already starts the service. The wizard's service option
uses an existing unit; installation and updates remain owned by `linux/install.sh`
or the package manager. Keep `agent-relay.service` running. A server
that must remain connected after logout also needs the one-time lingering
command printed by the installer.

## Run from source

The app needs GTK 4 and `pkg-config`. Cargo builds the core beside it, which
it runs when no other core answers:

```sh
sudo apt install libgtk-4-dev pkg-config librsvg2-common
cargo build --manifest-path gtk/Cargo.toml --features gtk --bin agent-relay
cargo build --manifest-path desktop-core/Cargo.toml --bin agent-relay-desktop-probe
RELAY_DESKTOP_PROBE="$PWD/target/debug/agent-relay-desktop-probe" target/debug/agent-relay
```

`agent-relay-fake-core` stands in for the core, for a look at the app with no
account:

```sh
cargo build --manifest-path gtk/Cargo.toml --features gtk --bin agent-relay --bin agent-relay-fake-core
AGENT_RELAY_CORE_SOCKET=/tmp/fake-core.sock RELAY_DESKTOP_PROBE="$PWD/target/debug/agent-relay-fake-core" target/debug/agent-relay
```

`--background` starts it in the tray. On macOS (`brew install gtk4
pkg-config`) it builds and runs for development, without a tray.

## Tests

`gtk/src` (the bridge client, core hosting, the systemd service,
preferences, updates and wording) and the `agent-relay` binary are tested with
the probe, against an in-process fake core and `agent-relay-fake-core`;
`linux_package` builds the package from stand-ins and installs it with the
updater:

```sh
cargo test --manifest-path gtk/Cargo.toml --locked --lib
cargo test --manifest-path gtk/Cargo.toml --locked --test linux_app_hosting --test linux_package
```

The window's tests (`gtk/tests/linux_app_gtk.rs`, the pages' cases in
`gtk/tests/linux_app_gtk/pages.rs`) drive the real widgets against the
in-process fake core. GTK must run on the main thread, so the target has its
own `main` and runs each case in turn; it needs a display (`xvfb-run -a` on a
server):

```sh
cargo test --manifest-path gtk/Cargo.toml --locked --features gtk --lib ui::
xvfb-run -a cargo test --manifest-path gtk/Cargo.toml --locked --features gtk --test linux_app_gtk
```

`linux_app_core_e2e` drives a built core with the app's bridge client, and
runs `agent-relay --headless`, against fake Relaycast and Cloud in an empty
home; it runs when `RELAY_E2E_PROBE` names the core. A core signs in to the
site it was built for and nothing at run time changes that, so the core is
built for the fake Cloud's loopback origin (`AGENT_RELAY_SITE_URL`) and
`RELAY_E2E_SITE_URL` names the same origin, where the fake Cloud then
serves; the test checks the two agree before it hosts anything:

```sh
AGENT_RELAY_SITE_URL=http://127.0.0.1:29147 cargo build --manifest-path desktop-core/Cargo.toml --locked --release --bin agent-relay-desktop-probe
RELAY_E2E_PROBE="$PWD/target/release/agent-relay-desktop-probe" RELAY_E2E_SITE_URL=http://127.0.0.1:29147 \
  cargo test --manifest-path gtk/Cargo.toml --locked --test linux_app_core_e2e
```

No test starts a broker, PTY runtime or agent harness, or reaches a real
Cloud, Relaycast or PostHog.

## Package

`scripts/build-linux.sh --version VERSION --app FILE --probe FILE` assembles a
relocatable filesystem tree from a built app (`scripts/build-gtk.sh`) and core
(`scripts/build-probe.sh`), each with the site and link scheme it is built
for recorded beside it, which must be production's (`https://agentrelay.com`,
`agentrelay://`), and produces a `.tar.gz` and, with `dpkg-deb` (or `ar`), a
Debian package:

| Path | Is |
|---|---|
| `usr/bin/agent-relay-desktop` | The app |
| `usr/lib/agent-relay/agent-relay-probe` | The relay core |
| `usr/lib/systemd/user/agent-relay.service` | The systemd user unit, running the core with `serve --headless` |
| `usr/share/applications/agent-relay.desktop` | The launcher entry, with both production and development link schemes |
| `usr/share/icons/hicolor/scalable/apps/agent-relay.svg` | The icon |
| `usr/share/doc/agent-relay/LICENSE` | The Functional Source License (FSL-1.1-ALv2) the app is released under; see NOTICE for earlier releases |
| `usr/share/doc/agent-relay/agent-icons-LICENSE.txt` | The license of the bundled agent icons |

`agent-relay` is the npm Agent Relay CLI's name, so the `.deb` never
installs `/usr/bin/agent-relay`. The tarball alone also holds
`usr/bin/agent-relay`, a link to the app that the in-app updater of releases
up to v2026.10.57 looks for when it installs a newer version. Run
`usr/bin/agent-relay-desktop` in place, or install the `.deb` to put the app
under `/`. The package's `postinst` (`linux/agent-relay.postinst`) has each running systemd
user manager reload, so `agent-relay.service` restarts the installed core, and
points each account's launch-at-login entry
(`~/.config/autostart/agent-relay.desktop`) that still runs
`/usr/bin/agent-relay --background` at `/usr/bin/agent-relay-desktop`, as that
account, so launch at login keeps working before the app is next opened.
Only the command changes: an entry the desktop turned off stays off. The
package depends on `libgtk-4-1` (4.6 or later), `librsvg2-common` for the
artwork and glibc 2.34 or later. CI and releases build both architectures
natively on Ubuntu 22.04, the oldest GTK the app supports.

Relay-owned model features use authenticated Cloud endpoints. Independently
launched agent CLIs keep their own authentication and environment.

The logged-out screen offers **Continue with Google**, which always opens
Google's account chooser instead of reusing a saved app or CLI session.
macOS, Linux and Windows use this explicit browser flow and PKCE handoff
without device-code confirmation. Local setup
clients can opt into it with `/setup/sign-in` and `{"flow":"browser"}`; older
and remote/headless setup clients retain device approval by default.

Sessions not on the relay have a **new relay name** field in their details.
**Set Name & Retry** registers the session with the chosen name, including after
a workspace name conflict. Relay names are unique across the workspace.

The bottom bar shows the full indexed session count for this machine, independent
of pagination, search filters, and hidden rows. It shows **Counting sessions…**
until that total is available. **Live Agents on Relay: N** has a green dot when
N is greater than zero and a grey dot otherwise. Updates remain available
in Settings and the account menu; request failures remain in Diagnostics.

The welcome screen matches macOS: the Agent Relay logo and wordmark are vertically
centered above the Codex, Claude, Cursor, OpenCode, Grok, Muse, Devin, and Pi logos, with Muse
in blue. During startup, **Rounding up your agents** stays on one line beside compact pulsing bars at the content's right edge.
The subtitle rotates every four seconds (unless reduced motion is enabled),
starting with a randomly chosen message. Sign-in appears as soon as
the service supplies its status; session discovery runs in the background.
After 20 seconds without a ready service, the panel offers **Try again** and
**Copy diagnostics** instead of continuing the animation.

### Reading a local session

Click any session row to open its local conversation, whether or not it is uploaded.
Opening the reader uses `sessions.transcript` without enabling uploads or relay registration.
The reader matches Cloud and macOS: chronological messages in a centered
760-point column, compact user bubbles, assistant Markdown and tables (tables beyond the conversation’s rendering limit stay readable as text), and
a compact **Activity** disclosure for tool work. System prompts and image attachments are collapsed by default. Internal thinking and harness idle nudges stay hidden; repeated capture IDs appear once while genuine repeated
messages remain separate. Messages load automatically through the full local
transcript and open at the latest reply. New replies follow while you are at the
bottom; scrolling back keeps your place and shows **Latest message** on
its own row under the conversation. There is no conversation sort button. The
header's second line names the harness, the agent's relay name while it
receives messages, and an **Online**, **Connecting…** or **Offline** badge. A
spawned agent's message box is one rounded shell that grows with the text, with
**Send** at its far right. Return sends; Shift+Return starts a new line.

The conversation header keeps **Rename title** beside the session title. **Details**
and a single centered account avatar stay on the right, without overlapping badges.

The session title replaces search in the shared header; there is no separate
transcript toolbar. Back to sessions sits between the wordmark and session title
in the shared header. **Details** opens the right sidebar, closed by default.
Matching full-width Open and Refresh buttons live in that sidebar.
Open includes the selected provider's artwork and keeps it after a core opening
plan or transcript refresh redraws the control. Claude Code uses its CLI artwork,
distinct from the Claude app. GTK uses **Open session** until an actual plan for
the selected row is known, then names that plan's action. Selecting a session
clears the last presentation; every Open still requests a fresh core plan.
Its content stays centered and the control continues to fill the same width as Refresh.
Receive messages updates optimistically while saving;
registration and refresh failures open an error dialog and stay visible beside
the switch. A successful registration survives a failed refresh, so turning it
off still releases its name. The Messaging section groups Receive messages
with the agent name field and Rename button. Clicking Messages in the sessions
table changes only messaging, without selecting its row or opening bulk controls.
Naming appears only when messaging
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

The global header is 64px high with a 32px logo and 24px horizontal gutters,
matching macOS. Search and the Table display icon share that same row. Settings alone has a 244px
sidebar; Back to sessions sits in the global header immediately after the wordmark,
in the same position as on a transcript. Search is
hidden in Settings and transcripts. Both platforms retain native window controls.

The desktop uses the effective Cloud workspace palette from
`cloud/packages/web/app/dashboard/_components/workspace-chrome.module.css`:
light canvas `#f9fafb`, dark canvas `#14191f`, primary `#2d6a9c` / `#74b8e2`,
and row hover `#e7eff7` / `#223e58`. These override the older airframe and
marketing palettes. macOS centralizes the tokens in `CloudTheme.swift`; GTK
uses the matching `LIGHT` / `DARK` palettes in `ui/theme.rs`. The macOS menu-bar panel
also uses these tokens for its canvas, text, hover, and status colors. The Linux
StatusNotifierItem menu is rendered by the desktop shell and follows the shell’s
theme; its protocol does not provide app-controlled menu colors.

Both appearances share Cloud’s 52px session rows, 28px compact filters,
6px group corners, subtle borders, bordered count badges, and muted zebra
stripes. Settings and local conversations use the same surfaces; code blocks
use Cloud’s dark code palette. Light, Dark, and Match system remain available.

Conversation rows share the profile-picture cache and pending download, retrying a
transient failure together so newly displayed rows reuse a photo already loaded.
On macOS, transcript geometry callbacks only observe position; following new
messages and Latest message perform scrolling outside geometry updates to avoid
a layout feedback loop. GTK already avoids redundant adjustment writes.

Local conversations skip Grok’s `unified_log_usage` accounting records, preserving
message and tool events, usage totals and pagination through usage-only pages.

Local transcripts hide Codex external-context XML and render embedded image
copies from the private reader cache, matching macOS. Capture requires the
RelayHistory embedded-image release (0.40.0 does not retain image bytes).
PNG, JPEG, GIF and WebP copies up to 1 MiB are supported; reading a local image
does not select its session for upload.

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

The full-width Open action in the Details sidebar includes the selected
provider's logo, action text and open arrow. Its tooltip and accessibility label
name the last verified current-row action, and core-plan replies and refreshes
preserve that artwork. Before the first ordinary plan returns, the label is
**Open session**; known spawned plans name their destination immediately.
Codex can open through a registered `codex://` handler, with terminal resume as a
fallback. Claude Code and Grok keep their supported terminal paths and
**Copy resume command**. Resume targets the
original session ID and working directory. A missing directory hides terminal
resume; unsupported sources and Claude cloud sessions have no local resume
action. The launcher tries `xdg-terminal-exec`, `x-terminal-emulator`,
GNOME Terminal, Konsole, Xfce Terminal, then xterm.

**View on web**, below the open action, opens an uploaded session's Cloud page
in the default browser. It is hidden until the session finishes uploading.

A session Agent Relay spawned is joined, never resumed beside its headless
process, as on the Mac ([native-spawn.md](../docs/native-spawn.md#joining-and-taking-over-a-spawned-agent),
`ui/spawn_join.rs`, `ui/spawn_take_over.rs`): its transcript is followed live
with a message box under it, and the agents page links to it with
**Transcript: Join**. A spawned Claude has **Take over in Terminal** in place of
the open action; it asks first (mid-turn, **Take Over After This Turn**, then
**Interrupt Now** while it waits), has the core stop the headless agent and
resumes `claude --resume <id>` in Terminal in its folder. Claude Desktop does
not take sessions over on Linux. A spawned Codex thread has **Open in Codex**
only: Terminal resume is not offered while Agent Relay's Codex app server
hosts it. Any other spawned agent (Grok, OpenCode, Muse, Devin) has no open
action while Agent Relay runs it; it is joined only, with the note "Agent Relay
runs this session. Follow it and message it here; it is not resumed in Terminal
while Agent Relay runs it."

Every spawned session Agent Relay still owns also has **End Agent** in the
transcript sidebar. It asks **End @name?**, then the shared core stops or
archives the exact harness session, confirms it ended, and removes its relay
identity and integrations. Any work in progress stops; local session history
stays. **Receive messages** is disabled meanwhile with "Use End Agent to stop
this spawned agent", so it cannot leave an untracked headless process. A
taken-over session has no end control and is left running when removed from
the relay. Windows uses this same GTK control and core behavior.

Take-over confirmations remain bound to the original agent when switching
transcripts. Take-over failures stay with their session until another attempt,
so returning to a transcript restores its error without showing it on others.

### Session search panel

Open **Search or filter sessions…** or press **⌘F** on macOS / **Ctrl+F** on
Linux, or press **/** while outside a text field. The search panel groups suggestions under Harnesses, Status, Projects,
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

Claude's **Open in Claude** handoff is macOS-only in Relay Desktop: Anthropic
currently supports `claude --desktop --resume` on macOS and x64 Windows, not
Linux. Linux keeps **Open in Terminal** and **Copy resume command**. A Claude Code
session a headless process holds (`claude -p`, a structured pipe, an SDK run)
has no open action on either platform, because Claude Code refuses to resume a
session something else holds; the note beside it says to run `claude agents`
to find that process, or stop it.

Selected search filters toggle off individually without removing other query
terms. The search bar's × tooltip reads "clear search"; the search panel's ×
closes the panel while preserving the query. Escape also closes the panel.

The session search bar fills the toolbar beside the Table display icon and uses 14px horizontal
and 9px vertical padding. Filter qualifiers appear as chips with a remove ×,
including exclusions and date constraints. Removing a chip keeps other filters
and text, without opening the search panel. Chips scroll horizontally as needed.

**Settings → Opening sessions** provides per-computer **Automatic** and
**Terminal** preferences for Claude Code and Codex and installation status for
the Codex handler; Claude setup guidance and **Check installed apps** sit in the
collapsed **Opening requirements** disclosure. Automatic
uses the Codex app when a handler is installed and otherwise opens Terminal.
Claude and Grok use Terminal on Linux. The viewer links to this page through
**Opening settings**. Historical local sessions do not reliably distinguish
terminal versus desktop origin; these choices do not guess or rewrite origin.

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

Session controls update immediately: Upload changes serialize only for the same
session, so other rows remain usable and rapid clicks settle on the last choice.
Failures roll back that row and show its error; stale listings cannot undo an
accepted change. The Receives messages orb is gray when offline, yellow while
registering/connecting, and green only when the session and push channel are live.
Clicking it toggles message reception; connecting can be cancelled. Tooltips and
accessible names describe the state/action and explain unavailable controls.

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

If the background service exits unexpectedly, the app shows **Reconnecting** while systemd starts the replacement, or while the app restarts its hosted core when the service was not enabled. Read-only requests interrupted by the exit wait up to five seconds and run again against the replacement. Changes are never sent again automatically, because the old service may have completed them before the connection closed; the app asks the person to check the result before trying again. A change the app could not send at all, because no connection to the service opened, is different: its message says the app is reconnecting or starting, and it clears once the service answers; Retry uploads and Compact now, which are safe to repeat, are then sent once on their own. A service that stays unavailable shows a recovery action. Once the new snapshot arrives, a transient read error no longer keeps the status orange.

The log lives only in memory for the current service run (up to 250 entries; large responses and bridge snapshots are bounded). Credentials and message content are redacted. Relaycast SDK rows contain decoded results or final error details, including HTTP status, request ID, and retry count where available. Successful HTTP statuses and individual SDK retry responses are not exposed by the SDK. Cloud rows include the actual HTTP status. Calls made with a Relay Connect room's credential are labeled Relay Connect instead of Agent Relay, because they use that room's workspace and its limits, not the team's. The log covers the core's Relaycast calls and Cloud HTTP transport, not WebSocket frames or separate upload transports.

Session lists use the daemon's shared snapshot: reopening the app reads cached
rows immediately while live-agent discovery and upload enrichment run in separate
background tasks. A cold daemon seeds rows from the collector's persisted catalog;
global upload status and history requests run independently in both desktop apps.

User-facing messages, help text, and diagnostic service labels use **Agent Relay**.
Relaycast remains an internal provider identifier.

After browser authentication, the welcome screen offers **Choose your workspace** for accounts with multiple workspaces, matching macOS. A single workspace continues automatically. Cancel leaves setup unconnected.

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

### Session history

Settings › General › **Session history** chooses how long this computer keeps
local session history: 7, 30 or 90 days, or Forever, 30 unless changed, as on
macOS and Windows (`history_retention_days`; see
[workspace layout](../docs/workspace-layout.md)). The choice is saved in the
signed-in workspace's history store, where the command line and other clients
see it too. The background collector removes older sessions from this
computer in small batches between upload passes, never before they have
uploaded: a session waiting to upload (while signed out, paused or offline) is
kept until it does, and for 7 days, and for any limit while uploads are held,
Settings notes that unuploaded sessions are kept until delivery succeeds.
Copies already uploaded to your workspace, and the provider's own files under
`~/.claude`, `~/.codex` and the like, are not affected.

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

Agent rows show **Stalled · N messages waiting · oldest M min** when the core reports an unread Claude inbox or a silent Codex turn blocking relay delivery. The wording and warning color match on macOS and Linux; the mark clears when progress resumes. See [native-spawn.md](../docs/native-spawn.md) for Codex steering, spawn-only interruption, and timing settings.

### Relay agent names


New relay registrations use `harness-repo-task`, with up to two meaningful
words from the session title (for example `claude-cloud-plan-support`). Names
are chosen locally; filler words are skipped. A missing task title uses
`session`. Duplicate automatic names get `-2`, `-3`, and so on. Existing
registrations retain their names as titles change; explicitly chosen names
are respected. This behavior is shared by macOS, Linux and headless mode.

### Codex CLI discovery

Codex terminal message delivery resolves npm CLI launchers to their bundled native
executables, so the desktop or background service does not require Node on its
PATH. Nested and hoisted platform packages and legacy vendor layouts are supported.

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
connection. Grok, OpenCode, Devin, Muse, Pi, OpenClaw and Hermes also receive
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
detected on this machine or explicitly enabled appear in alphabetical order. **Add
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
identity](../docs/hosted-mcp-runtime.md) for the Cloud deployment dependency
and the private-runtime, Claude and spawned-worker boundaries.

Opening preferences now live on the Codex, Claude Code and Grok provider pages.
Claude’s page also contains its session folders, delivery, approvals and activation
hook controls. General contains the one global upload policy and Upload details,
explicitly applying to all providers. The separate Opening sessions and Session
sharing sidebar entries are removed; existing stored preferences are preserved.

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
manual-check spinner. The layout and wording match the Mac.

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

Session search covers the local history catalog through bounded core cursor
pages, as on macOS and Windows. **Next page** replaces the visible page and
continues a sparse search even when the current page has no matches.
**First page** returns to current history. Scrolling does not load unrelated
history automatically. The core keeps a bounded recent catalog version window for open cursors;
an expired cursor refreshes from the first page.

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

Mobile continuation is on unless turned off, but takes effect only after
**Accept spawns from Agent Relay** is on and the computer is claimed by your
signed-in account: it uses that existing ownership proof, the plugin is not
installed before it, and turning the row on explicitly before then is refused
with that explanation. A peer message cannot establish the claim.

After your first mobile message, replies stay synced to your phone, including
replies to later messages you type in that same desktop conversation. They stay
bound to the verified owner and current control generation. Before mobile
engagement, desktop replies are not forwarded. Permission prompts for turns you
start on the desktop continue to use Claude's native desktop prompt.

Claude opens the private Unix socket through its native HTTP API. The core
requires the exact registered Claude process; tool subprocesses cannot use this
native-input bridge. Windows displays the option but keeps it disabled until
Claude's native named-pipe transport is verified; there is no subprocess fallback.

**Native input from the spawning agent** (`spawn_native_input`, on by default)
lets the verified requester send DMs as native Codex or Claude user turns. Other
agents remain peers and tool approvals remain with the human owner. The switch
is shared across macOS, Linux and Windows; see [native spawn](../docs/native-spawn.md) for details.

**Approvals from the spawning agent** (`spawn_requester_approvals`), shown
below it while native input from the spawning agent is on, also lets that
requester answer its sessions' permission and approval prompts by 1:1 direct
message; the first answer, the owner's or the requester's, counts, and each
is logged with who answered. On in Agent Relay Dev, off in the release build
until the person turns it on; agents can only turn it off. See [approvals
from the spawning agent](../docs/native-spawn.md#approvals-from-the-spawning-agent).

Live personal sessions refresh their native title and agent @name after ten new
human text prompts, using the existing Cloud LLM brief. **Rename title** edits
the session title; **Rename agent** in Details → Messaging edits its relay name.
A manual edit starts another ten-prompt interval. Tools and duplicate captures
do not count. Existing names remain while a fresh brief or native title editing
is unavailable, with retries every five minutes. Prompt counts are checked at most once every five minutes. Background naming uses local or already cached evidence and never downloads remote-only transcripts. This does not enable uploads.
Previous @names still forward; spawned agents keep their assigned names. The
shared core stores counts in private `session-naming.json`, so restarts retain
the interval. See [Relay agent names](../docs/workspace-layout.md#relay-agent-names).

The welcome layout follows agentrelay.com/login: dark ribbon artwork, a dark
form with white provider buttons and the shared eight-agent strip, including
Antigravity’s correct mark. Ribbons and message pulses animate locally and stop
for reduced motion; website footer links are omitted.

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

Saved native-provider connections are checked using their own scoped Agent Relay credentials. The core confirms the MCP handshake and required search/messaging tools, caches proof against account/workspace and credential fingerprints, and bypasses the cache on Check connection. This verifies the configured connection; existing chats can still need a reload. Capy also checks the current workspace’s saved connection through its project-list API when no active thread is verified. Permission or network failures stay unverified.

The coding provider keeps the durable `muse` identifier but is displayed as **Muse Code**. **Muse** (`muse-assistant`) is the personal assistant at muse.ai and appears separately, with an Open Muse handoff. On macOS the installed assistant opens directly; Linux and Windows use its website. Automatic history sync and webhook delivery require separately verified Muse connector capabilities; they are not inferred from a saved login.

Muse shows a selectable, monospace setup prompt in onboarding and provider settings. **Copy setup prompt** copies it without opening another app. The separate **Open Muse** action opens the installed macOS app, or the website on Linux and Windows; the person pastes and sends the prompt. **Read setup skill** opens the published skill directly. No Accessibility access is used. The skill recommends scheduled polling and requires a Muse-compatible Relay polling registration before enabling a recurring task.

### Session locations

Exact app and terminal window navigation is currently available on macOS. The
core reports this capability as unavailable here through `sessions.location`;
the desktop client keeps its existing provider resume actions. It does not claim
to focus an existing terminal pane.
