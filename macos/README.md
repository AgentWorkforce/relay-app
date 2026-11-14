# Agent Relay for macOS (`macos/`)

The native window and menu-bar app: a thin SwiftUI client of the relay core
(`agent-relay-probe serve`, [`docs/relay-core.md`](../docs/relay-core.md)).
The core owns sign-in, the uploader, agents, delivery and every Relaycast and
Cloud call; launchd runs it as a LaunchAgent the app registers. The app renders
the core's snapshot, re-fetches what it invalidates, sends it commands, and
keeps what only a Mac app can do: windows and the menu bar, the clipboard,
opening the browser on a sign-in link, notifications, launch at login, and
self-update.

## Startup

**Settings › General › Open at login** starts both the menu-bar app and its
upload service when you sign in. Off schedules neither: manually opening the
app bootstraps a session-only launchd job from a plist outside
`~/Library/LaunchAgents`. Keeping agents connected after quitting controls the
current session separately and does not schedule a session-only job at login.

## Headless (no app)

A Mac that runs agents but not the app (a Mac mini in a rack, a build Mac)
can run the core alone. One command installs it for the current user and
runs it as the LaunchAgent `com.agentrelay.desktop.core.headless`; run it
again to update. Every release publishes `macos/install.sh` as
`install-macos.sh` (with `install-macos.sh.sha256`), next to the Linux
`install.sh`:

```sh
curl -fsSL https://github.com/AgentWorkforce/relay-desktop-releases/releases/latest/download/install-macos.sh | bash
curl -fsSL https://github.com/AgentWorkforce/relay-desktop-releases/releases/latest/download/install-macos.sh | bash -s -- --uninstall
bash macos/install.sh --version 2026.10.34   # a given release
bash macos/install.sh --update               # update an existing install only
bash macos/install.sh --rollback             # back to the previous one
bash macos/install.sh --status               # what is installed, and does it answer
bash macos/install.sh --uninstall            # stop and remove it, keep the data
bash macos/install.sh --dry-run              # say what would change
```

It downloads `AgentRelay-<version>-macOS-<arch>-probe.tar.gz` (arm64, also
from a shell under Rosetta, or x64), checks it against the release's
`.sha256` and checks the core's Developer ID signature (team `QUJ7SA6X8X`)
before running it, then installs it under
`~/.local/lib/agent-relay/versions/<version>` and switches `current` to it.
The LaunchAgent in `~/Library/LaunchAgents` runs
`current/agent-relay-probe relay serve --headless` in the user's login session
(`gui/<uid>`), restarts it if it fails, and logs to
`~/Library/Logs/Agent Relay`. After an update the installer waits for the
core to answer with the new version, and switches back to the previous one
if it does not. Three versions are kept. Settings, sign-in and agents live in
`~/Library/Application Support/com.agentrelay.desktop` and are never touched,
not even by `--uninstall`. Finish sign-in with the printed
`agent-relay-probe setup` command; the device code can be approved in a
browser on any computer.

- **Login session.** A LaunchAgent in `gui/<uid>` runs only while the user is
  logged in at the Mac; SSH alone has no such session, and the installer
  refuses without one. For an unattended Mac, turn on automatic login
  (System Settings › Users & Groups); FileVault rules it out.
- **The app.** The app runs this same core on the same socket. The installer
  refuses while the app is installed or its core is loaded, and a headless
  core that sees the app's core job exits to hand over. Run
  `install.sh --uninstall` before installing the app.

`scripts/test-macos-install.sh` runs the installer against a fake release
server and a fake `launchctl`, on Linux too.

## Layout

```
macos/
  Package.swift                 swift-tools 5.9, macOS 15, no dependencies
  Info.plist.template           __VERSION__ / __BUILD__ are filled in by CI
  RelayDesktop.entitlements     hardened runtime, no sandbox
  Sources/RelayDesktopCore/     the bridge client and view-model helpers, unit tested
    CoreBridge.swift            NDJSON JSON-RPC client: hello, calls, subscriptions
    CoreContract.swift          the bridge contract's types
    CoreService.swift           where the core runs (the LaunchAgent), behind a protocol
    LegacyMigration.swift       one-time moves of what earlier builds kept
    StatusDisplay.swift         uploader stage -> what a person reads
    SessionUploadState.swift    upload state per session row, filters, summary
    UploadStages.swift          the reading / collecting / uploading checklist
    RelayPresence.swift         a session's relay switch state
    Filters.swift               search on the sessions table
    SessionNavigation.swift     Cloud links for uploaded sessions
    AppUpdate.swift             release metadata for self-update
    SVGPath.swift               M/L/C/Z path parser + the Agent Relay mark
    RelativeTime.swift          "3 minutes ago"
  Sources/RelayDesktop/         the app (SwiftUI + AppKit)
    RelayDesktopApp.swift       main Window + MenuBarExtra
    AppModel.swift              the bridge client: connection, account, uploader, settings
    RelayModel.swift            sessions, agents, roster, messages
    LaunchAgentService.swift    SMAppService.agent for the core
    LoginItem.swift             SMAppService.mainApp (open the app at login)
    AppUpdater.swift, AppUpdateInstaller.swift  self-update
    *View.swift, SessionsTable.swift, MessageSheet.swift, BrandMark.swift, ...  the UI
  Tests/RelayDesktopTests/      bridge client and view models against FakeCore,
                                an in-process fake of the core's socket
  install.sh                    the core without the app (Headless, below)
```

## How the views stay idle

The models (`AppModel`, `RelayModel`, `ChatModel`, …) are `@Observable`
classes; views take them from `@Environment(AppModel.self)` or as plain
properties. A view re-renders only when a property its body read changes, so:

- **Models assign only what changed.** Every answer from the core is compared
  before it is stored; an identical answer notifies no one.
- **Derived values are stored.** What a view would filter, sort or count
  (`phase`, `relayProblem`, `localSessionCountText`, `listedSessions`,
  `liveOnRelayCount`, the menu summary, the Sessions table's results) is kept
  by the model when its inputs change, or memoized on their revisions; a body
  reads it ready-made. The uploader status is read through slices
  (`uploadGate`, `statusViewModel`), so counters that move during uploads
  re-render only what shows them.
- **Nothing in a view runs on a clock.** The core pushes `status` and
  `invalidated` events. What it cannot announce (the request log, the roster,
  an open composer's recipient, a spawned agent's transcript, a summary Cloud
  is still generating) is read by the model, and only while it is on screen.
- **Continuous work follows visibility.** Each window's root applies
  `tracksWindowVisibility`; it tells the model and sets `windowIsVisible` for
  every view inside, which `TimelineView`s and repeating animations read to
  stop while the window is closed, minimized or covered. The closed menu
  panel draws a placeholder.

`BodyEvaluations` counts the main views' body evaluations in debug builds;
`AppModelTests` uses it to prove an unrelated change re-renders nothing.

## Running it locally

`swift run` has no bundle and so no LaunchAgent; start a core on the default
socket in one terminal and the app in another, both from the repository root:

```sh
cargo run --manifest-path desktop-core/Cargo.toml --bin agent-relay-desktop-probe -- serve   # terminal 1
swift run --package-path macos RelayDesktop                  # terminal 2
```

- Closing the window keeps the menu-bar app; quitting the app leaves the core
  running.
- Agent Relay Dev (a build for a loopback site) never opens the app at login:
  Settings hides the toggle, and launching it removes a login item it
  registered earlier. Its core's LaunchAgent does run at login; turn off
  **Settings › General › Keep agents connected after quitting** and quit to
  remove it. A build for any other site is a release build and opens at login
  like production's.
- Launch at login and the LaunchAgent need a signed `.app` bundle
  (`scripts/build-app.sh`, ad-hoc is enough locally).
- Production builds register the LaunchAgent only from `/Applications` or
  `~/Applications`. Launching a copy on a mounted DMG can leave BTM pointing
  at the ejected volume; move the app first.
- If macOS loses the bundled agent's container path and launchd reports
  `spawn failed` with exit 78 for `Contents/Helpers/...`, the app switches to
  `~/Library/LaunchAgents/com.agentrelay.desktop.core.local.plist`, whose
  `Program` is the helper's absolute path. It is reloaded after app updates
  and removed when the app quits with **Keep agents connected after quitting**
  off.
- App logs: `log stream --predicate 'subsystem == "com.agentrelay.desktop"'`.
  Core diagnostics: `~/Library/Logs/Agent Relay/core.log` (Agent Relay Dev
  uses `~/Library/Logs/Agent Relay Dev/core.log`). These credential-redacted
  diagnostics include Connect failures, even under the bundled or fallback
  LaunchAgent. The core rotates at 1 MiB and keeps `core.log.1` through
  `core.log.3`; a record may exceed the cap by at most its bounded line size.
  Logging failures do not prevent the core from starting or serving requests.
  Raw child stdout/stderr and request or message bodies are not captured.
  Live core state: `agent-relay-probe call status.get --json`.

Tests never start a core, broker or harness:

```sh
cd macos
swift test
```

For end-to-end development, point a compatible local Agent Relay service stack
at this checkout with `DEV_DESKTOP_DIR`. Agent Relay Dev uses its own bundle ID,
socket (`~/.agentworkforce/desktop/core.dev.sock`) and data directory
(`~/Library/Application Support/com.agentrelay.desktop.dev`), leaving production
connections untouched. A build with any other bundle ID (`--bundle-id`) has
its own in the same way: label `<id>.core`, socket `core.<id>.sock` and data
directory `<id>`.

## How the bundle is assembled

`scripts/build-app.sh` (in the repository root) does the assembly. What the
build is made for is fixed here and nothing at run time changes it:
`--site-url` (default `https://agentrelay.com`), `--url-scheme` (default
`agentrelay`), `--product-name` (default `Agent Relay`), `--bundle-id` (default
`com.agentrelay.desktop`), `--releases-repository` (where the app checks for
updates, default `AgentWorkforce/relay-desktop-releases`) and `--icon` (an SVG
or 1024×1024 PNG in place of the repository's mark). `--dev-site-url URL` is
the shorthand for Agent Relay Dev: `--site-url URL` for a loopback site with
the `agentrelay-dev` scheme, the name `Agent Relay Dev` and the bundle ID
`com.agentrelay.desktop.dev`. The probe in `--probe` must be built for the
same site and scheme (`scripts/build-probe.sh --site-url … --url-scheme …`);
the refusal names the command.

1. `swift build -c release --arch arm64|x86_64` in `macos/`
2. copies `RelayDesktop` to `<product name>.app/Contents/MacOS/`
3. copies the core for that architecture to `Contents/Helpers/<product name> Helper`
   (`agent-relay-probe` under the name Activity Monitor shows; its signature
   keeps the identifier `agent-relay-probe`), and once it is signed hard-links
   it as `Contents/Helpers/<product name> Helper (Sessions)`, the name the core
   starts its session collector (`run`) under
   (0755) with `Contents/Resources/agent-relay-probe.sha256` and
   `Contents/Resources/probe-version.txt`
4. writes the core's LaunchAgent to
   `Contents/Library/LaunchAgents/<bundle id>.core.plist`
   (`BundleProgram` the helper, `serve`, `RunAtLoad`, `KeepAlive`; a
   `--socket` and `--app-data-dir` for any bundle ID but production's)
5. renders `Contents/Resources/AppIcon.icns`, fills in `Contents/Info.plist`
   (identifier, name, link scheme) and records the site, scheme and update
   repository in `Contents/Resources/site-url.txt`, `url-scheme.txt` and
   `releases-repository.txt` (and, with `--icon`, `mark.txt`: the app icon is
   the mark the app draws), which `AppInfo.swift` reads
6. signs with the hardened runtime using `RelayDesktop.entitlements`, or ad-hoc
   when no signing identity is configured.

Only a loopback site makes a development build (`AppInfo.isDevelopment`: no
launch at login, the green menu bar mark); a build for any other site is a
release build like production's. The sign-in screen's **Development** badge
names the site of a core for any site but production, the core's own once it
reported one (`AppModel.developmentBadge`), and a refused link is worded as on
Linux and Windows (`AppModel.linkRefusal`).

`--debug` builds the debug configuration instead and signs with the keychain's
Apple Development identity when there is one (ad-hoc otherwise), so Keychain
access survives rebuilds; it never uses Developer ID credentials.
`scripts/dev-reload.sh` rebuilds such a bundle in place: it replaces the
executable and resource bundles and re-signs with the bundle's existing
identity. The core's binary and LaunchAgent stay as built.
