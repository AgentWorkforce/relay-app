# Agent Relay for Windows

The app opens Sessions on launch and whenever the signed-in window is rebuilt
after an update or sign-in, matching macOS and Linux.

Conversation rows share the profile-picture cache and pending download, retrying a
transient failure together so newly displayed rows reuse a photo already loaded.
On macOS, transcript geometry callbacks only observe position; following new
messages and Latest message perform scrolling outside geometry updates to avoid
a layout feedback loop. GTK already avoids redundant adjustment writes.

Local conversations skip Grok’s `unified_log_usage` accounting records, preserving
message and tool events, usage totals and pagination through usage-only pages.

The local conversation reader matches Cloud, macOS and Linux: chronological
messages in a centered 760-point column, compact user bubbles, assistant Markdown
and tables (tables beyond the conversation’s rendering limit stay readable as text), and a compact **Activity** disclosure for tool work. System prompts and image attachments are collapsed by default. Internal thinking and harness idle nudges stay hidden. Repeated capture IDs appear once; genuine repeated
messages remain separate. Conversations open at the latest reply and follow new
replies while you are at the bottom. Scrolling back keeps your place and shows
**Latest message** on its own row under the conversation. The header's
second line names the harness, the agent's relay name and an **Online**,
**Connecting…** or **Offline** badge, and a spawned agent's message box is one
rounded shell that grows with the text, with **Send** at its far right; Return
sends and Shift+Return starts a new line.
**Details** opens the right sidebar, closed by default,
with Open, Refresh, messaging and upload controls. There is no conversation sort button.
Open and Refresh fill the same sidebar width. Open keeps the selected provider's
artwork beside its centered action text when a core opening plan or transcript
refresh redraws it. This presentation uses the existing Windows opening
capabilities; it does not claim a provider app or exact terminal pane can be
opened when the core reports it unavailable.

The conversation header keeps **Rename title** beside the session title. **Details**
and a single centered account avatar stay on the right, without overlapping badges.

The bottom bar shows the total indexed sessions on this machine, independent of
loaded pages or search filters, and **Live Agents on Relay: N** with a green dot
when N is greater than zero and a grey dot otherwise. Updates remain available
in Settings and the account menu; request failures remain in Diagnostics.

Settings → Relay shows one row, **Continue agents from mobile**, which
covers Claude Code (`mobile_claude_handoff`) and the other native adapters
(`mobile_native_continuation`) and is on unless turned off on supported
computers. On Windows the shared core reports both `*_supported` capabilities
as false, so the row is shown disabled: Claude's native HTTP connection has not
been verified with Windows named pipes, the Pi and OpenCode runtime bindings use
Unix process and socket evidence, and the Codex app connection also requires
Unix transport. There is no subprocess fallback, and unsupported sessions do not
fall back to peer delivery as owner input. On a supported Mac or Linux computer
the same row sends your phone messages into Claude Code, Pi, OpenCode and Codex
app sessions as your own turns, and compatible Codex app windows can send
command and file approval requests for mobile-origin turns to your phone.
macOS and Linux use a native Unix-socket connection authenticated as Claude
itself; tool subprocesses are refused. There, the plugin waits on a doorbell
connection instead of polling; Windows does not run the plugin, so nothing
changes here.

**Native input from teammates** (`teammate_native_input`, on unless turned off)
is shown on Windows too, but it routes through the same Claude plugin, so on
Windows a teammate's message keeps reaching Claude Code through its peer inbox
until that connection is verified. See [the shared flow and
limits](../docs/workspace-layout.md).

As on macOS and Linux, an agent changing settings on the session socket
(`PATCH /settings`) is refused with `person_only_setting` when the change
widens who can reach or act as you: adding a trusted sender, spawn repository
or Claude Code folder, or turning on automatic activation, direct delivery,
spawning or a tool's delivery route. You make those in Settings; **Changed by
agents** says so. Settings for tools that are not available on Windows
(OpenClaw, Hermes, Grok, OpenCode, Devin, Cursor CLI) are not offered at all.

Claude Code delivery behaves as on macOS and Linux: a message Claude Code will
hold for approval reads **Held for approval in Claude Code** in Recent
Messages and under a message you sent, an agent follows its session through
`/clear` (messages sent before the clear are refused), and before writing to a
session's inbox pipe the core checks with `GetNamedPipeServerProcessId` that the
session's own process created it.

The Windows app is the Linux GTK 4 app built for Windows: the same
`agent-relay` program from [`gtk/src`](../gtk/src)
(the app crate's (`gtk/`) `gtk` feature), with the same sessions table, header,
Settings panes, Agents, Team, Messages, tray menu and sign-in, word for word.
Settings › Account › Workspace shows the plan row when Cloud reports a plan
(**Intro plan**, its caps against what the workspace uses, **Manage plan**),
and a session a plan cap kept off the relay names the cap, as on Linux and the
Mac.
Session rows name their harness and working folder as on Linux; Pi's mark is
not shown because Pi sessions are not listed on Windows. Session icons sit on
their provider's primary colour: Muse is a white mark on a blue circle, Grok a
white mark on a black circle, and the Claude app a white mark on an orange
rounded square; Claude Code in the terminal shows the orange Claude Code crab,
and Codex and Grok Bot keep their own app tiles. A Claude session is the Claude
app's when its transcript records the `claude-desktop` entrypoint (the history
row's `originator`), or when it is a Claude app cloud session.
The logged-out screen offers **Continue with Google** and **Continue with Microsoft**.
Each opens its provider's account chooser instead of reusing a saved app or CLI
session. There is no tool-connection checkbox, and sign-in does not wait for tool
settings. Agent Relay changes no other tool's config until sign-in; existing
integration choices remain in effect and provider setup is available afterward,
as on macOS and Linux.

An account with no workspace yet gets **Open dashboard** (Cloud's Workspace
settings, where it creates one) and **Try again**, as on the Mac and Linux.
While connected without a Cloud session the core may use, a notice under the
header reads **Agent Relay needs you to sign in again.** with **Sign in again**,
as on the Mac and Linux.
Provider setup appears in the post-sign-in chooser and Settings › Providers,
without Capy or Devin banners above Sessions. **Set up in Capy** opens a new
thread with the setup skill prompt prefilled. Session messaging controls live
on each provider's page, including Cursor (detected through `%USERPROFILE%\.cursor`).
**Ask Claude chats to join** is on by default once after sign-in; an explicit
saved Off choice stays off.
Windows toast notifications need no
permission prompt.
It is a thin client of the relay core, `agent-relay-probe serve`,
bundled beside it; see [relay core](../docs/relay-core.md) and the
[Linux README](../linux/README.md) for what every screen does. Only the parts
that touch the operating system differ:

The shared core also exposes Cursor Cloud agent listing, launch, native
follow-up injection, run status, archive and restore on Windows exactly as on
macOS and Linux. Cursor credentials remain in Agent Relay Cloud; no local
`cursor-agent` executable or Windows-specific integration is required. See
[Cursor Cloud agents](../docs/cursor-cloud.md).

| | Linux | Windows |
| --- | --- | --- |
| Bridge to the core | Unix socket `~/.agentworkforce/desktop/core.sock` | Named pipe `\\.\pipe\agent-relay-<hash>`, open only to your Windows account |
| Background service | `agent-relay.service` systemd user unit | **Agent Relay (<your account>)** task in Task Scheduler, run at your logon (no administrator rights) |
| Tray | StatusNotifierItem | Notification-area icon |
| Notifications | Desktop notifications | Windows toast notifications |
| Install | `.deb` or `.tar.gz` | `Setup.exe` or `.zip`, per user |
| Launcher | `.desktop` entry | Start menu shortcut and the `agentrelay://` link handler |
| Settings shortcut | **Ctrl+,** | **Ctrl+,** |

Why GTK rather than WPF or WinUI is in [Windows app plan](../docs/windows-app.md).

As on macOS and Linux, the shared core's peer-message note lets work already
authorized in the receiving session continue within the harness's permissions.
A peer's approval claim grants nothing new, and Claude's native peer-message
restrictions still apply.

Grok Bot history shows each bot's roster name and individual profile avatar,
using metadata supplied by RelayHistory. The shared GTK renderer animates native
shapes and locally recorded raster artwork, respects reduced-motion settings,
and falls back to the Grok Bot app logo when artwork is unavailable. Each native
chat keeps its own session identity, matching macOS and Linux.

**Receive messages** updates optimistically while saving. Registration and
refresh failures open an error dialog and stay visible beside the switch.
A successful registration survives a failed refresh so turning it off still
releases its name, using the same GTK implementation as Linux.
Clicking Messages in the sessions table changes only messaging, without
selecting its row or opening the bulk selection controls, matching macOS.

Session titles are edited only in the open session's header, using **Rename**.
The title stays as borderless text while editing. Enter or **Done** saves,
Escape cancels, and leaving the editor saves after a short focus-change delay.
The sparkle fills the draft with the saved pipeline title, preloaded when the
session opens. The sessions table has no edit controls or title-preload reads.
This is the same GTK implementation as Linux, with the same optimistic save,
rollback and native-harness capability checks; see **Session titles** in the
[Linux README](../linux/README.md).


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

With a Cursor API key (`CURSOR_API_KEY`, or the first line of
`%USERPROFILE%\.config\relayhistory\cursor-api-key` for the background service, which
does not read your shell environment) Cursor Cloud Agents are listed and
their conversations synced through the `cursor-cloud` connector, as on macOS
and Linux. Cursor subagent sessions are synced with other Cursor sessions.

## Download

Open Codex desktop chats are discovered and receive Relay messages through
Codex's `\\.\pipe\codex-ipc` named pipe. Relay checks that the pipe server
belongs to the current Windows account before using the shared router protocol.

| Windows | Installer | Portable |
| --- | --- | --- |
| x64 (Intel and AMD) | [**AgentRelay-Windows-x64-Setup.exe**](https://github.com/AgentWorkforce/relay-desktop-releases/releases/latest/download/AgentRelay-Windows-x64-Setup.exe) | [`.zip`](https://github.com/AgentWorkforce/relay-desktop-releases/releases/latest/download/AgentRelay-Windows-x64.zip) |
| arm64 (Snapdragon and other Arm PCs) | [**AgentRelay-Windows-arm64-Setup.exe**](https://github.com/AgentWorkforce/relay-desktop-releases/releases/latest/download/AgentRelay-Windows-arm64-Setup.exe) | [`.zip`](https://github.com/AgentWorkforce/relay-desktop-releases/releases/latest/download/AgentRelay-Windows-arm64.zip) |

Not sure which you have? **Settings › System › About › System type**.
Every release also carries version-pinned copies
(`AgentRelay-<version>-Windows-<arch>-Setup.exe`,
`AgentRelay-<version>-Windows-<arch>.zip`) and a `.sha256` for each, plus
`AgentRelay-<version>-Windows-<arch>-probe.zip`, the bare core
(`agent_relay\helpers\agent-relay-probe.exe` and the DLLs it needs) for
headless use.

## Beta updates

**Settings → Updates → Include beta releases** opts into numbered beta builds,
with the same wording and behavior as macOS and Linux. It is off by default,
even in a manually installed beta, and persists in the app's `desktop.json`.
Beta users receive the newest eligible beta or stable version. Changing the
setting clears the pending update and checks immediately. Turning it off keeps
the installed version until a newer stable release is available.

Beta updates download and verify the architecture-specific Windows ZIP and its
SHA-256 checksum, using the same per-user installation flow as stable updates.
GitHub releases use **Beta** and **Stable** titles with
[beta](https://github.com/AgentWorkforce/relay-desktop-releases/releases?q=prerelease%3Atrue)
and [stable](https://github.com/AgentWorkforce/relay-desktop-releases/releases?q=prerelease%3Afalse)
filter links. Stable download links above continue to select stable releases.

## Install

Agent Relay installs per user, without administrator rights, under
`%LOCALAPPDATA%\Programs\AgentRelay\<version>\`.

**With the installer.** Double-click `AgentRelay-Windows-<arch>-Setup.exe`.
It asks for no administrator rights and no folder: it installs into
`%LOCALAPPDATA%\Programs\AgentRelay\<version>\AgentRelay-Windows-<arch>\`,
adds Agent Relay to the Start menu (and, if you tick it, the desktop),
registers `agentrelay://` links and starts the app. The installer is not
code-signed yet, so SmartScreen shows *Windows protected your PC*: choose
**More info** › **Run anyway** (see [Unsigned builds](#unsigned-builds)).
Running a newer Setup over an installed Agent Relay upgrades it in place: it
closes the open window, installs the new version beside the old one and moves
Launch at login and the logon task to it, as an in-app update does; if the
logon task cannot be moved, Setup says so; turn **Settings › General › Open
at login** off and on to repair its login registration. For a scripted install, run
it with `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`; it then does not start the
app.

**Portable, from the zip.** Unpack the zip under the same folder and start
the app once; it adds itself to the Start menu. In PowerShell:

```powershell
$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x64' }
$release = Invoke-RestMethod https://api.github.com/repos/AgentWorkforce/relay-desktop-releases/releases/latest
$version = $release.tag_name.TrimStart('v')
$base = "https://github.com/AgentWorkforce/relay-desktop-releases/releases/download/v$version"
$zip = "$env:TEMP\AgentRelay-$version-Windows-$arch.zip"
Invoke-WebRequest "$base/AgentRelay-$version-Windows-$arch.zip" -OutFile $zip
$expected = (Invoke-RestMethod "$base/AgentRelay-$version-Windows-$arch.zip.sha256").Split(' ')[0]
if ((Get-FileHash $zip -Algorithm SHA256).Hash -ne $expected) { throw 'Checksum mismatch' }
$dest = "$env:LOCALAPPDATA\Programs\AgentRelay\$version"
Expand-Archive $zip $dest -Force
& "$dest\AgentRelay-Windows-$arch\bin\agent-relay.exe"
```

Either way, the install is this folder:

```
%LOCALAPPDATA%\Programs\AgentRelay\<version>\AgentRelay-Windows-<arch>\
  bin\agent-relay.exe          the app
  bin\agent-relay-probe.exe    the relay core
  bin\*.dll, gdbus.exe, …      the GTK 4 runtime from MSYS2
  lib\gdk-pixbuf-2.0\          image loaders, with SVG
  share\glib-2.0\schemas\      GTK's settings schemas
  share\gtk-4.0\               GTK's data
  share\icons\                 Adwaita symbolic icons, the Agent Relay icon and agent-relay.ico
  NOTICES.txt                  the bundled libraries and their licenses
  LICENSE.txt                  the Functional Source License (FSL-1.1-ALv2) the app is released under
  agent-icons-LICENSE.txt      the license of the bundled agent icons
```

On Linux the app is installed as `agent-relay-desktop`, because the npm Agent
Relay CLI is `agent-relay` there. Windows keeps `agent-relay.exe`: the install
folder is never added to `PATH`, the npm CLI is `agent-relay.cmd`, and launch
at login and the logon task only ever run the install's own `.exe`.

On first start a per-user install writes the Start menu shortcut
(`%APPDATA%\Microsoft\Windows\Start Menu\Programs\Agent Relay.lnk`), the
`agentrelay://` link handler under `HKEY_CURRENT_USER\Software\Classes`, and
the **Agent Relay (<your account>)** logon task in Task Scheduler, turned off (one task per Windows account). Windows files the
app's notifications under the AppUserModelId `AgentWorkforce.AgentRelay`
(registered under `HKEY_CURRENT_USER\Software\Classes\AppUserModelId`). The
app's own preferences are in `%APPDATA%\agent-relay\desktop.json`. `agent-relay.exe
--install-launcher` writes the shortcut without opening the window. A copy
unpacked anywhere else runs, but is treated as a build: it does not update
itself or add a shortcut.

Agent Relay runs once per Windows account. Starting `agent-relay.exe` again
(the Start menu, a terminal, an `agentrelay://` link) hands its arguments to
the running app and exits within a second: the window comes back from the
tray with the link opened, and `--background` leaves it in the tray, with no
second tray icon. As on Linux this is GApplication's D-Bus uniqueness; the
bus is the bundled `bin\gdbus.exe`, which GLib starts
(`gdbus.exe _win32_run_session_bus`) the first time the app registers and
which keeps running while you are signed in to Windows.

**Settings › General › Open at login** controls both the tray app's Run
entry and the **Agent Relay (<your account>)** logon task. On starts the tray
app and `agent-relay.exe --headless` for uploads when you sign in; off schedules
neither, leaving current uploads running. The headless service preserves the
tray app's Run entry. Upgrading preserves either existing startup opt-in,
including a task enabled by an older build that removed the tray entry, and
reconciles both registrations once. A failed migration shows the enabled choice
and a repair notice. Manually opening the app still starts or connects to its
core. **Keep agents connected after quitting** controls the current session:
off stops the core when you quit, without changing the next-login setting.

**Quitting.** With **Settings › General › Keep agents connected after quitting**
on, quitting leaves your agents connected and uploads running. With it off,
quitting while signed in asks **Quit and take your agents offline?**:

- **Keep Agents Connected** (the default) turns the setting on and quits,
  leaving agents connected and uploads running.
- **Quit Anyway** quits and stops the core; agents and uploads stay offline
  until you open Agent Relay again.
- **Cancel** keeps Agent Relay open.

Installing an update and signing out of Windows do not show this confirmation.
Turning the setting off while the logon task runs first asks to turn the task
off too, so it cannot keep agents connected after the app quits.

**Headless, from a script.** A PC or server without the app runs the bare
core as that same logon task: [`install.ps1`](install.ps1) downloads
`AgentRelay-<version>-Windows-<arch>-probe.zip`, verifies its published
SHA-256, unpacks it under `%LOCALAPPDATA%\Programs\AgentRelay\core\versions\<version>`,
points `current` at it, registers **Agent Relay (<your account>)** to run
`agent-relay-probe.exe serve --headless` at logon and starts it, then
waits for the core to answer and rolls back if it does not. It is the
counterpart of `linux/install.sh`: run it again to update (`-Version X.Y.Z`
for one release, `-Rollback` to go back, `-DryRun` to only say what would
change), and it needs no administrator rights. When the app is installed and
owns the task, the script leaves it to the app. Finish sign-in and provider
setup with the same terminal wizard as Linux. Coding agents may still use the
lower-level [agent-driven setup](../docs/agent-driven-setup.md#scriptable-windows-installation)
routes over the named pipe.

```powershell
Invoke-WebRequest https://raw.githubusercontent.com/AgentWorkforce/relay-app/main/windows/install.ps1 -OutFile install.ps1
powershell -ExecutionPolicy Bypass -File install.ps1
```

**Terminal setup without the app.** The bare-core installer starts the existing
logon task. From PowerShell, run:

```powershell
& "$env:LOCALAPPDATA\Programs\AgentRelay\core\current\agent-relay-probe.exe" setup
```

The wizard uses the core's named pipe automatically and needs no GTK window.
Approve its device code at the displayed verification URL in a browser on any
computer, then finish workspace, provider and upload-policy choices in the
terminal. Existing choices are preserved when their flags are omitted.

```powershell
& "$env:LOCALAPPDATA\Programs\AgentRelay\core\current\agent-relay-probe.exe" setup --non-interactive --json --providers detected --sharing selected --wait
```

`--json --non-interactive` emits newline-delimited progress events and a final
result. Initial approval is `action_required`, with its public URL and code,
and exits with status 2 unless `--wait` waits for the person's approval. A
later `--wait` run resumes the pending approval. Use
`--providers codex,claude`, `--workspace UUID`, `--sharing selected|new|all`,
or `--socket PATH` to configure those choices explicitly. The Agent Relay site
is the one the core was built for (`AGENT_RELAY_SITE_URL` when it was built);
nothing at run time changes it.
The Linux-only `--start-service` option is not needed for the Windows logon
task. A service token does not replace the interactive account consent needed
for provider MCP setup. See the [Linux wizard option reference](../linux/README.md#finish-setup-over-ssh)
and `setup --help`.

**Updates.** A per-user install checks the public releases once a day.
Installing an update downloads the release's zip, checks its published
SHA-256, unpacks it beside the running version, points the logon task at it
(restarting the task if it runs) and relaunches. The previous version stays on
disk until the next update. After the installer, an update also keeps the
**Installed apps** entry and a desktop shortcut on the new version.

**Uninstall.** If you used the installer, uninstall **Agent Relay** from
**Settings › Apps › Installed apps**. Its uninstaller
(`%LOCALAPPDATA%\Programs\AgentRelay\unins000.exe`) ends and deletes the
**Agent Relay (<your account>)** logon task, quits Agent Relay, and removes
every installed version (all of `%LOCALAPPDATA%\Programs\AgentRelay`,
including versions in-app updates added), the Start menu and desktop
shortcuts, the `agentrelay://` handler, the notification identity and the
**Launch at login** entry. It keeps your sign-in and data.

For a zip install: quit Agent Relay (tray icon › **Quit Agent Relay**), turn off
the task or run `schtasks /Delete /TN "Agent Relay (%USERNAME%)" /F`, then delete
`%LOCALAPPDATA%\Programs\AgentRelay`, the Start menu shortcut and
`HKEY_CURRENT_USER\Software\Classes\agentrelay` and
`HKEY_CURRENT_USER\Software\Classes\AppUserModelId\AgentWorkforce.AgentRelay`.
Your sign-in and the core's data are in `%USERPROFILE%\.agentworkforce`, and
the app's preferences in `%APPDATA%\agent-relay`.

### Unsigned builds

Releases, the installer included, are not code-signed yet. Microsoft Defender
SmartScreen may show *Windows protected your PC* when you open the installer
or on the first start: choose **More info** › **Run anyway**. **Smart App Control** blocks unsigned programs with no way to allow
one; it must be off (**Windows Security › App & browser control › Smart App
Control**) to run Agent Relay, or anything you build yourself, until releases
are signed. Signing the installer, executables and DLLs is planned (see
[the plan's status](../docs/windows-app.md#status)).

## What works on Windows

Core diagnostics are saved in the app data directory's `logs/core.log`
(`%USERPROFILE%\.local\share\com.agentrelay.desktop\logs\core.log` by default,
respecting `XDG_DATA_HOME` and `--app-data-dir`). This also works for the logon
task, which has no console. Credentials are redacted; the log rotates at 1 MiB
with three backups. Logging failures do not prevent core operation. Raw child
output and request or message bodies are not captured.

Everything the app shows works as on Linux, with these sessions: **Claude
Code** and **Codex** run natively on Windows and list, upload, receive
messages and spawn in full. **Codex mid-turn delivery** adds Agent Relay's
Codex hook to `%USERPROFILE%\.codex\hooks.json`, run through `cmd.exe` with
the probe's path quoted, and talks to the core over the session pipe. The
core also writes `%USERPROFILE%\.codex\rules\agent-relay.rules` and
**Ask Claude chats to join** adds `permissions.allow` rules, as on macOS and
Linux, but Codex's rules cannot match a quoted program and every Windows
path is quoted, so Windows Codex sessions still ask before Agent Relay's
read and reply commands; Claude Code's rules match the quoted path. Windows gets no space-free link to the probe: a symlink
there needs Developer Mode or administrator rights.
Claude Code on the web sessions are listed under **Agents** and receive
messages through `claude --print --cloud` as on Linux and macOS
([integrations/claude-code-web](../integrations/claude-code-web/README.md)).
Their Settings row shows whether the source is working, checking, blocked or
needs attention, its last successful pass, and the plain cause and fix; failed
passes and helper failures also reach the credential-redacted `core.log`.
Sessions spawned through Agent Relay show their task as the session title and
original request, without the relay delivery header and report instructions.
Relayed messages use the same task text.

A spawned session Agent Relay still owns has **End Agent** in its transcript
sidebar, matching macOS and Linux. After **End @name?** is confirmed, the
shared core stops or archives the exact harness session, confirms it ended,
and removes its relay identity and integrations. Work in progress stops; local
session history stays. **Receive messages** is disabled while this action
applies, and a taken-over session is never ended by Agent Relay.

Harnesses that are Unix-only tools are unavailable on Windows, and their
Settings rows say so: **Muse**, **Devin**, **Cursor sessions**, **Grok CLI**,
**Hermes**, **OpenClaw**, **OpenCode** existing-session injection, **Pi** and
**Oh My Pi**. Cursor CLI delivery proves a session by the conversation store
its process holds open, which the Windows process table cannot read yet, and
the Cursor editor's agent is proved through the same Unix-only route. The core
reports them unavailable rather than failing to start them.

**Trust people in your workspace** and the read-only **Workspace member** list
share the macOS and Linux GTK implementation. These Trusted senders controls
apply to OpenClaw and Hermes, so they are absent from the Windows core's settings
while those harnesses are unavailable. On supported computers, workspace people
are matched by their person identity IDs; their agents still need manual entries
under **Other names and agents**. See [delivery and trust](../docs/openclaw-hermes-delivery.md).

Every control that changes the core shows the change the moment it is made
and goes back, with the reason beside it, if the core refuses, as on Linux
and the Mac; launch at login (the Run key) works the same way. See
*Controls answer at once* in `linux/README.md`.

Uploads recover the same way as on Linux and the Mac. Uploads that keep
failing on a server error or a dropped connection read **Waiting to retry**
while they wait out a backoff of at most five minutes, and the upload stage
offers **Retry uploads** too; a rate limit only waits. A Retry or Compact that
could not reach the background service is sent on its own once the service
answers again, and its message clears without a click.

Agent rows show **Stalled · N messages waiting · oldest M min** when the core
reports an unread Claude inbox or a silent Codex turn blocking relay delivery,
with the same wording and warning color as on Linux and the Mac; the mark
clears when progress resumes. The Claude inbox watch covers channel posts and
messages to a renamed agent's old name, and recovers after the core restarts,
as on macOS and Linux. Codex stall detection and recovery are the shared
core's, unchanged on Windows: guarded steering only while relay messages are
shown, and an interrupt only for a still-owned spawned turn. See
[native-spawn.md](../docs/native-spawn.md) for steering, spawn-only
interruption, and timing settings.

## Building from source

The app and the core build with [MSYS2](https://www.msys2.org)'s clang
toolchain, which provides Rust, GTK 4 and its runtime without administrator
rights: **CLANG64** on x64, **CLANGARM64** on Arm. Install MSYS2, then in its
CLANG64 or CLANGARM64 shell:

```sh
# x64 (CLANG64); on Arm, replace clang-x86_64 with clang-aarch64
pacman -S --needed zip unzip \
  mingw-w64-clang-x86_64-{toolchain,rust,gtk4,pkgconf,librsvg,ntldd}
```

Check out with LF line endings, so the shell scripts run:

```sh
git clone -c core.autocrlf=false https://github.com/AgentWorkforce/relay-app.git
```

Build and run the app and its core:

```sh
cargo build --manifest-path gtk/Cargo.toml --features gtk --bin agent-relay
cargo build --manifest-path desktop-core/Cargo.toml --bin agent-relay-desktop-probe
RELAY_DESKTOP_PROBE="$(cygpath -m "$PWD")/target/debug/agent-relay-desktop-probe.exe" target/debug/agent-relay.exe
```

The app looks for its core beside itself (`agent-relay-probe.exe`), or where
`RELAY_DESKTOP_PROBE` points. A debug build runs from the MSYS2 shell, which
puts the GTK DLLs on `PATH`; outside it, use the packaged tree.

Build the portable tree and the zips (the same as CI and releases):

```sh
scripts/build-windows.sh --version 0.0.0 --arch x64 --out dist/windows
```

Without `--app` and `--probe` it builds both in release mode with the Agent
Relay icon and version embedded (`AGENT_RELAY_WINDOWS_ICON`, read by
`desktop-core/build.rs`), then writes `dist/windows/AgentRelay-Windows-<arch>/`,
`AgentRelay-<version>-Windows-<arch>.zip` and
`AgentRelay-<version>-Windows-<arch>-probe.zip`, each with a `.sha256`, and,
when [Inno Setup 6](https://jrsoftware.org/isinfo.php) is installed
(`winget install JRSoftware.InnoSetup`; the script finds `ISCC.exe` through
`$ISCC`, `PATH` or Inno Setup's install folder),
`AgentRelay-<version>-Windows-<arch>-Setup.exe` and its `.sha256`, compiled
from [`windows/installer/AgentRelay.iss`](installer/AgentRelay.iss).
`--installer` fails the build without Inno Setup (CI and releases pass it);
`--no-installer` skips it. It copies every DLL the programs and image loaders need from the MSYS2 prefix
(found with `ntldd -R`), compiles GTK's schemas, trims Adwaita to its
symbolic icons, renders `agent-relay.ico` from the SVG mark with
`rsvg-convert` (no binary is checked in; without `rsvg-convert` the tree has
no `.ico` and says so), and writes `NOTICES.txt` from pacman's package and
license data.

A locally built program is unsigned: Smart App Control must be off to run
it, or to run cargo's build scripts at all.

## Tests

In the CLANG64 or CLANGARM64 shell:

```sh
cargo build --manifest-path gtk/Cargo.toml --locked --features gtk --bin agent-relay --bin agent-relay-fake-core
cargo test --manifest-path gtk/Cargo.toml --locked --features gtk --lib
cargo test --manifest-path desktop-core/Cargo.toml --locked --lib
cargo test --manifest-path gtk/Cargo.toml --locked --features gtk --test linux_app_gtk
cargo test --manifest-path gtk/Cargo.toml --locked --features gtk --test linux_app_hosting
```

The GTK harness needs a desktop session (no virtual display, unlike `xvfb-run`
on Linux). CI runs these on `windows-latest` (x64) and `windows-11-arm`
(arm64), then builds the zips and the installer with
`scripts/build-windows.sh`, and installs and uninstalls with the installer
silently to check where it puts the files and what it removes.

Session search covers the local history catalog through bounded core cursor
pages, as on macOS and Linux. **Next page** replaces the visible page and
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

### Session history

Settings › General › **Session history** chooses how long this computer keeps
local session history: 7, 30 or 90 days, or Forever, 30 unless changed, as on
macOS and Linux (`history_retention_days`; see
[workspace layout](../docs/workspace-layout.md)). The choice is kept in the app
data folder and in the signed-in workspace's history store, and applies to this
computer only; copies already uploaded to your workspace are not affected. A
session is never removed before it uploads: for 7 days, and for any limit while
you're signed out or uploads are paused, Settings notes that unuploaded sessions
are kept until delivery succeeds.

### Usage diagnostics

Windows uses the same optional PostHog usage events as macOS and Linux through
the shared core and GTK interface. Settings → Share diagnostics controls these
events, including agent registration, messaging, navigation and searches. Search
queries, message contents, names and session IDs are never sent. See the root
README’s Desktop analytics and error tracking section for the schema and limits.
Background transcript following never counts as a new open, even without a
cursor. Awaited update diagnostics have a three-second deadline and wait
asynchronously so an unresponsive core cannot freeze the window before restarting.

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

Capy, Grok Bot and Dot are also offered in onboarding without requiring an
installed app. These rows have setup buttons rather than local MCP checkboxes.
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
manual-check spinner. The layout and wording match the Mac and Linux.

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

On a supported Mac or Linux computer, mobile continuation is on unless turned
off, but takes effect only after **Accept spawns from Agent Relay** is on and
the computer is claimed by your signed-in account: it uses that existing
ownership proof, the plugin is not installed before it, and turning the row on
explicitly before then is refused with that explanation. A peer message cannot
establish the claim.

On supported platforms, after your first mobile message, replies stay synced to your phone, including
replies to later messages you type in that same desktop conversation. They stay
bound to the verified owner and current control generation. Before mobile
engagement, desktop replies are not forwarded. Permission prompts for turns you
start on the desktop continue to use Claude's native desktop prompt.

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

The core records local process and host identities through `sessions.location`.
The shared `sessions.open_plan` decision opens an exact provider link, shows a
verified native window, resumes a stopped session in its recorded terminal, or
explains an unavailable capability. Windows Terminal profiles and captured
provider storage paths are retained when resuming.

Native window activation checks HWND ownership and process creation time. A
Windows Terminal session GUID alone does not prove a tab or pane, so Relay does
not claim exact pane selection from it. Multiple unidentifiable windows and WSL
runtime restoration remain explicit limitations. See
[session navigation](../docs/session-navigation.md) for capability and acceptance
details.
