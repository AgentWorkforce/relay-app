# session-navigation

Decides how to get a person back to a coding-agent session: focus it if it
is still open, reopen or resume it if it was closed or archived, and
otherwise refuse with a specific reason. Providers: Claude Code (Claude.app
and the CLI), Codex (desktop app and CLI), OpenCode (desktop app and TUI)
and Grok (CLI only). Platforms: macOS, Linux (X11 and Wayland) and Windows
(native and WSL). Terminals: Ghostty, Terminal.app, iTerm2, Kitty, WezTerm,
Alacritty, GNOME Terminal, Konsole, xterm, Windows Terminal and the classic
console host, plus tmux as a multiplexer layer inside any Unix terminal.

The workspace has two members:

| Member | Crate | Role |
|---|---|---|
| `core/` | `session-navigation` (`session_navigation`) | Pure rules: evidence, decide, plans, the open cycle, and a deterministic simulator with an independent oracle. No real side effects: it spawns no processes, runs no scripts, opens no URLs and reads no files outside its test fixtures. |
| `native/` | `session-navigation-native` | Real adapters behind a fakeable `System` seam: `src/system/` (`System`, `RealSystem`, `FakeSystem`), `src/common/` (OS-independent: quoting, reservations, tool resolution, Claude/Codex/OpenCode/Grok observers, provider-path config), `src/macos/` (kernel reads or ps/sysctl/lsof, JXA, lsappinfo, AppleScript, executor, `MacConfig`, `MacPorts`), `src/linux/` (see its section) and `src/windows/` (PowerShell and `wsl.exe` helpers, `WindowsConfig`, `WindowsPorts`). Features `macos`, `linux`, `windows` (all default) gate the platform modules; only the FFI files in `src/system/` are `cfg(target_os)`-gated. |

Build output goes to the repository's shared `target/`
(`.cargo/config.toml` at the repository root). Most of the confidence comes from the
tests.

## The four steps

1. **Observe** (`ports::Observe`) produces an immutable `evidence::Snapshot`
   for one session. Every fact in it is `Fact::Known(x)` or `Fact::Unknown`
   and is never defaulted. The providers keep their own shapes:
   - Claude: a per-pid registry, plus desktop records that own a set of CLI
     ids.
   - Codex: writer-lock holders, the thread row, and IPC window ownership.
   - OpenCode: server state, existence confirmed by the API or the database,
     and tab hints.
2. **Decide** (`decide::decide`) is pure and total. The shared guards are
   id validation (Claude and Codex ids are compared lower-case), evidence
   completeness, selection, version gating and launch reservations. Provider
   policy lives in `providers::{claude,codex,opencode}` behind the
   `ProviderPolicy` trait. The result is a `Decision { state, plan }`. A plan
   is data: steps, a precision (`ExactConversation`, `RightWindow` or
   `AppOnly`), and the post-condition that would prove it worked. A plan is
   `Act`, `Choose` (several live places), `Confirm` (safe only if the person
   confirms the session is not open in the processes it names; the same
   request with `confirm_not_open_elsewhere` then proceeds) or `Unavailable`.
   A launch carries the resolved absolute executable and only the
   provider-home variables of the tool's allowlist (`Tool::environment_keys`);
   an unknown executable is `ToolNotInstalled`, an unknown environment
   `EnvironmentUnknown`.
3. **Act** (`ports::Execute`) dispatches one step at a time. The result is
   `Dispatched`, `Opened { tty }`, `Refused` or `Failed`. A dispatch never
   proves success.
4. **Verify** (`engine::open`) runs the whole cycle: observe, decide, write a
   durable reservation, dispatch, observe again, then check the post-condition
   against both snapshots. The outcome is `Verified`, `DispatchedUnverified`,
   `Failed(reason)`, `Blocked(reason)` or `NeedsChoice` (also for `Confirm`).
   The engine never turns a failed focus or link into a launch. A take-over
   observes up to `MAX_EXIT_OBSERVATIONS` times for the stopped owner to exit
   and never resumes if the exit is not proven. While a reservation is
   unresolved, another launch is blocked. A reservation resolves when the
   window it opened is gone, when the session is found in that window, when
   it was made in an earlier boot, or (for a Claude `resume` link) when the
   desktop record is non-archived and openable. A `Refused` dispatch (the
   Automation prompt was declined; nothing ran) releases it.

   What Claude.app shows is not observable, but the record it focuses is: a
   Claude desktop link is `Verified` when a non-archived record holding the
   session as its current transcript has a `lastFocusedAt` newer than in the
   plan's snapshot (native N1, N5), or when a registry claim that was not
   there before names the session and such a record. It is
   `Failed(WrongDestination)` when every such record is still archived (a
   `continue` link on an archived record is a silent no-op, N3), and
   otherwise `DispatchedUnverified`. The engine observes up to
   `MAX_LINK_OBSERVATIONS` times before settling. An unchanged
   `lastFocusedAt` on a live record is not called a failure: re-focusing the
   record already on screen and a slow app start were not observed natively.

   A Codex resume is `Verified` only when a writer lock for the thread
   appears that was not there before (the daemon's, N8). A TUI waiting at the
   "Update available" menu holds none (N12), and a second client of the
   daemon adds none (N10), so both stay `DispatchedUnverified`; the argv-named
   window they opened is focused next time instead of launching again.

## Layout (`core/`)

| Path | Role |
|---|---|
| `src/model.rs`, `evidence.rs`, `plan.rs` | Vocabulary (platforms, runtimes, providers, terminals, apps and their per-platform identifiers), snapshot and plan data |
| `src/hosts.rs` | Terminal hosts: `PaneRef`, `TerminalSupport`/`Ability`, pane-API, tmux and window listings (`Surfaces`) |
| `src/parse/` | Pure parsers and id validators. URL builders emit no unvalidated id |
| `src/decide/` | Shared guards, ownership interpretation, locating a live pane (`locate.rs`), version gate |
| `src/providers/` | One policy module per provider |
| `src/ports.rs`, `src/engine/` | Ports, the open cycle, post-condition checks |
| `src/sim/` | World truth, lossy observation, hazardous execution, event generator, oracle, runner and shrinker; `hosts*.rs`, `tmux.rs`, `starts.rs`, `grok.rs` model platforms, terminals, tmux, WSL and Grok; `defaults.rs` the machine's default terminal and its oracle check |
| `tests/simulation.rs` | Seeded random runs, every first-round world-variable combination, every new-variable combination, regressions, replay |
| `tests/hosts.rs` | Host decision table: each terminal and capability per platform, tmux, Windows windows and tabs, WSL, Grok, hazards, the launch-window race |
| `tests/contract_core.rs` | Snapshot-level contract: unknown apps/schemes/front, paths, Windows consoles, Grok parsers and records |
| `tests/adversarial_*.rs` | The reviewers' findings and probes |
| `tests/findings_*.rs` | Regression tests for each fixed finding not pinned by the reviewers' tests |
| `tests/native_*.rs` | Behaviour verified on the real apps (native N1-N19): parsers, decisions, post-conditions, simulator |
| `tests/suite/` | Decision table (one test per host × state), the default-terminal rule (`default_terminal.rs`), the terminal preference (`prefer_terminal.rs`), end-to-end scenarios, parser and fixture tests, totality fuzzing (`totality_hosts.rs` covers the host space) |

To add a provider or host, add a module under `src/providers/`, add one entry
to `providers::POLICIES`, and give the simulator the matching shapes.

## Running

```sh
export PATH=~/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH
cargo test --workspace                       # core + adapters, under ten seconds once built
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
for f in macos linux windows; do
  cargo test -p session-navigation-native --no-default-features --features $f
done
```

When a random run fails, the test panics and prints the seed and the
world-variable bits, the invariant and its cause (check and provider), a
minimal event sequence and a replay line. Generated event targets are stable
picks: they follow the target's identity (a label derived from the event that
created it, `sim::labels`), not its position, so dropping an event while
shrinking does not retarget the others, and a candidate is kept only if it
fails for the same cause.

To replay a failure:

```sh
SESSION_NAVIGATION_REPLAY=<seed>:<vars> cargo test -p session-navigation --test simulation replay -- --nocapture
SESSION_NAVIGATION_TRACE=<seed>:<vars>:<len> cargo test -p session-navigation --test simulation trace -- --nocapture
```

`TRACE` shrinks the run and prints each event and every open. For each open
it shows the decision, the dispatched steps, the outcome and the truth that
followed. Once a seed is fixed, add it to `REGRESSIONS` in
`tests/simulation.rs`.

## Invariants (checked by `sim::oracle` against World truth, independent of `decide`)

No duplicates (at dispatch, and when a delayed launch or a dismissed update
menu actually starts a writer; a client of the Codex daemon or of the
OpenCode server is not a writer, native N10, N14; and no launch next to a
window that already shows the session and names it in argv) · no silent
creation · no stale handles · uncertainty
preserved (incl. a known provider environment) · honest precision ·
idempotent double open · converges after a verified resume · right directory
(a launch runs in the session's true directory) · right terminal (a launch
opens the default terminal, or a terminal the session was seen in unless the
default was asked for; `NoDefaultTerminal` only when no default exists, the
simulated machine's own default being drawn per run: set, another possibly
uninstalled terminal, none, or unreadable) · shared OpenCode server
intact (no engine-caused version-mismatch restart) · total · no input side
effects (no prompt text, take-over only on request and only after a verified
stop; a tmux command line may only `select-window`, `select-pane` and
`attach-session`) · same runtime (a launch runs in the session's own runtime:
WSL or native, never a foreign one).

Stale handles are judged on what each step actually hits by its raw
mechanism (a tty, a Kitty/WezTerm id, a window's selected tab, a tmux pane
id), for every step the executor did not refuse. The simulated executors
re-check every identity a `PaneRef` carries before acting, as the adapter
contract requires.

The generator leaves out races no planner can prevent: while our own launch
for a session is still starting, the person deletes that session, resumes it
themselves, starts another client or server of the same provider, or
upgrades or removes its CLI (`World::races_engine_launch`). It does generate
the hazards between a decision and its first step (`Event::Race`): Kitty
remote control or the WezTerm socket switched off, macOS Automation revoked,
the target terminal restarted (stale socket, recycled ttys), the tmux server
restarted (pane ids recycle), every tmux client detached, and a window handle
reused by another program. It also generates windows that report their tty
before their first process (`Event::SlowStart`), tabs joining windows, two
tmux clients on one session, Kitty/WezTerm APIs switched on and off, tmux
sessions attached and detached, and WSL sessions. Half the seeded runs are
macOS, the rest Linux (X11, Wayland) and Windows (`sim::hosts::platform_for`).

## Platforms, hosts and the capability model

The snapshot carries the `platform` (`MacOs`, `Linux { session: Fact<X11 |
Wayland> }`, `Windows`) and the session's `runtime` (`Native`, `Wsl {
distro }`, `Foreign`). An `App` is identified per platform
(`App::identifier`: a macOS bundle id, a Linux desktop-file id and
executable, a Windows executable and AUMID); no provider desktop app is
modelled off macOS. What a terminal can do is never assumed from its name:
the adapter reports a `TerminalSupport` per terminal in
`Surfaces::terminals`, each ability `Available`, `NeedsSetting` (Kitty
`allow_remote_control` + `listen_on`, the WezTerm mux socket),
`NeedsPermission` (macOS Automation) or `Unsupported` (`NoPaneApi`,
`Wayland`, `NotOnThisPlatform`, `NotImplemented`), plus whether a window's
tabs can hide the session.

A live process is located from evidence only (`decide/locate.rs`). Its host
is the nearest ancestor that is a terminal, a tmux server or a desktop app;
on Windows a classic console is the `conhost.exe` child of an ancestor.

| What the host can do | Plan | Precision cap |
|---|---|---|
| `focus_pane`, tty-scripted terminal (macOS Ghostty, Terminal, iTerm2) | `FocusPane { Tty }` | exact |
| `focus_pane`, Kitty / WezTerm: exactly one listed pane with the agent's tty, or whose process is the agent or an ancestor below the terminal | `FocusPane { KittyWindow \| WezTermPane }` | exact |
| `raise_window` (X11, Windows): the window the process declares (`WINDOWID`) if its own terminal owns it, else the only window that terminal (or console) owns | `FocusPane { X11Window \| WindowsWindow }`, carrying the owner's identity | exact for a single-pane window; app-level with `PaneSelectionUnavailable` when tabs share it (a Windows Terminal session GUID selects nothing) |
| several candidate windows | `WindowAmbiguous` (the person picks it outside Relay), unless the app can be activated | — |
| `activate_app` only (macOS terminals without an adapter) | `BringForward`, the most specific reason as note | app-level |
| nothing (Wayland without a pane API, a disabled setting with no fallback, …) | refused with the most specific reason: `WaylandNoRaise`, `HostSettingDisabled`, `PermissionRequired`, `PlatformUnsupported`, `HostUnsupported` | — |
| tmux pane, a client attached | `FocusPane { TmuxPane { socket, server, pane, client_tty, outer } }` in the client's terminal; `outer` is located like any pane | the outer pane's |
| tmux pane, no client (detached) | `Attach`: launch `tmux -S <socket> select-window -t @W ; select-pane -t %P ; attach-session -t $S` in a new window; starts no agent, reserves nothing | the provider's |
| a process of a WSL or foreign runtime | not in this machine's windows: `LiveNotLocatable` | — |

Launches stay argv + cwd + allowlisted env, with a `LaunchContext`: the
session's own runtime (a WSL session resumes inside its distribution; WSL off
Windows and foreign runtimes are `ForeignRuntime`; an unknown runtime
`EvidenceIncomplete`) and the Windows Terminal profile it was seen with
(unknown → refused for Windows Terminal). Paths are checked for the runtime:
POSIX in Unix and WSL runtimes, `C:\…` natively on Windows; every path check
rejects all control characters.

The terminal a launch (a resume or a tmux attach) opens: on an explicit
`use_default_terminal` request, the default; otherwise the terminal the
session was last seen in, else the one it was first seen in (when the last
host is a terminal, tmux or unknown), else the default. A tmux origin
resumes in the terminal around it, outside tmux. The default is the
person's explicit Relay preference (`Preferences::default_terminal`,
`None` unless they chose one), else the operating system's default as the
adapter observed it (`Snapshot::default_terminal`); there is no built-in
terminal. With neither, the launch is refused: `NoDefaultTerminal` when the
system has none (or one this crate does not model), `EvidenceIncomplete`
when it could not be read. Whatever is chosen must be installed and
launchable (`AppNotInstalled`, `PlatformUnsupported`, `HostUnsupported`, …);
it is never swapped for another terminal. `last_seen_host` is the latest host and
`original_host` the first; `Host::Tmux(outer)` records tmux. `apps`,
`schemes` and `front.app` are `Fact`s: unknown is refused
(`EvidenceIncomplete`) wherever needed, and never verified.

`Preferences::prefer_terminal` (the apps' "Open with: Terminal"): a stopped
session the plan would open in Codex (an unarchived, desktop-started thread)
or revive in Claude (an archived desktop record), or would refuse to open
there, resumes with the CLI in the terminal chosen as above when that resume
is possible; otherwise the plan is unchanged, never a refusal instead. Never
for a live session (a take-over included) or an unarchived Claude record
(the app may revive it beside the CLI); Grok and OpenCode ignore it. Tested
in `tests/suite/prefer_terminal.rs`, and on every simulated open that draws
it by `sim/preference.rs` (`PreferenceScoped`), which decides the same
snapshot again without the preference and accepts only those changes.

Reservations: a launch's window counts as gone only after a process was seen
on its tty (`Reservation::seen`, set by the engine from its own
observations). A terminal reports a new window's tty before its first
process; an observation in that gap no longer releases the reservation
(`tests/hosts.rs`, regression seed 16). Without a tty (Windows) a reservation
resolves when the plan returns to an owner that started after it.

Grok (CLI only, `providers/grok.rs`, facts-grok.md): owners are live
`active_sessions.json` records (a Grok executable of this boot that started
no later than `opened_at`) and processes whose argv names the session;
focus is `RightWindow`; a stopped session resumes with `grok --resume <id>`
in its own `info.cwd` after `summary.json` confirms it, asks first next to
another Grok process in that directory (including `grok agent stdio`
clients, which register nowhere; `grok agent leader` is never a host),
refuses subagents, and is `PlatformUnsupported` on Windows.

## Adapter contract

A platform adapter implements the three ports in `core/src/ports.rs`:

```rust
pub trait Observe { fn observe(&mut self, session: &SessionRef) -> Snapshot; }
pub trait Execute { fn execute(&mut self, step: &Step) -> Dispatch; }
pub trait Reservations {
    fn reserve(&mut self, reservation: Reservation) -> Result<(), StoreError>;
    fn release(&mut self, session: &SessionRef) -> Result<(), StoreError>;
}
```

and `engine::open(&mut ports, &session, &request)` drives them. Types:
`core/src/evidence.rs` (`Snapshot`, `Fact`, `ProcessInfo`, `Front`, `AppInfo`,
`ToolInfo`, `SchemeHandler`, `Reservation`, `Capability`, provider
evidence), `core/src/hosts.rs` (`Surfaces`, `TerminalSupport`, `Ability`,
`Setting`, `Permission`, `Unsupported`, `PaneInfo`, `TmuxServer`,
`TmuxPaneInfo`, `TmuxClientInfo`, `WindowInfo`, `WindowId`, `PaneRef`),
`core/src/model.rs` (`Platform`, `DisplayServer`, `Runtime`, `TerminalKind`,
`App`, `AppIdentifier`, `Tool`, `Host`, `ProcessIdentity`, `BootId`,
`Reason`), `core/src/plan.rs` (`Step`, `Command`, `LaunchContext`,
`Dispatch`). Reuse `native/src/system` (`System`, `RealSystem`,
`FakeSystem`), `native/src/common` (provider observers, reservations store,
quoting, tool resolution, `ProviderPaths`) and the platform's
`provider_paths(..)` constructor.

### Every adapter, every snapshot

- **Never default a fact.** Anything not read is `Fact::Unknown`;
  `Known(empty)` means "read, nothing there" (or "this mechanism does not
  apply on this platform", documented per field below).
- `session`, `observed_at` (epoch seconds), `platform` (always known).
- `runtime`: `Known(Native)`, or `Known(Wsl { distro })` when the session's
  store is inside a WSL distribution (Windows only), `Known(Foreign { .. })`
  for anything that cannot be launched into. `processes`, `tools`, `cwd`,
  `cwd_exists` and `environment` all describe **that** runtime (for WSL: the
  distribution's own process table, the tools on its PATH as POSIX paths, its
  paths).
- `boot`: a per-boot id (macOS: `kern.boottime`; Linux:
  `/proc/sys/kernel/random/boot_id` hashed or btime; Windows: the system boot
  time). `processes`: the whole table, each row a `ProcessInfo { identity:
  (pid, start in whole seconds, boot), ppid, euid, ruid, exe, tty, argv,
  cwd, window }`. `exe` is classified from the kernel's executable (never
  argv[0]): the agent CLIs (`grok` or `grok-<version>` is `GrokCli`), desktop
  apps, every `TerminalKind`'s executable, `Tmux` (server and clients),
  `Login`, `Shell`, `Other`. `tty` is `/dev/…` or `None`; **always `None` on
  Windows**. `window`: the X11 `WINDOWID` from the process environment when
  readable, else `None` (always `None` on macOS and Windows).
- `apps: Fact<Vec<AppInfo>>`: one row per `App` that exists on the platform
  (`app.identifier(platform).is_some()`), with installed, version, running.
  `schemes: Fact<Vec<SchemeHandler>>`: registered URL scheme handlers,
  `bundle` = the handler string of the platform (`AppIdentifier::handler`).
  `front: Front { app, pane_tty, pane, window, view }`: `app` is the
  frontmost app (`Known(None)` for none, e.g. a locked screen); `pane_tty`
  the focused pane's tty when the front app is a terminal whose focus can be
  read (macOS scripted terminals; Kitty/WezTerm through their API), else
  `Unknown`, and `Known(None)` when not applicable; `pane` the focused
  Kitty/WezTerm pane as a `PaneRef` (`Known(None)` when the front app is not
  one of them; `Unknown` when its API is off); `window` the foreground X11 /
  Win32 window (`Known(None)` on macOS; `Unknown` on Wayland); `view`
  `Unknown` unless the desktop app's shown conversation is readable.
- `tools`: one `ToolInfo` per `Tool` (incl. `GrokCli`, `Tmux`): `version`
  (`Known(None)` = not installed), `path` = the resolved **absolute**
  executable in the session's runtime (`C:\…\x.exe` natively on Windows).
- `surfaces: Surfaces { terminals, panes, tmux, windows }`:
  - `terminals`: a `TerminalSupport` for every `TerminalKind` that exists on
    the platform, reporting what **this adapter can do now**: `focus_pane`,
    `raise_window`, `activate_app`, `launch` (each `Available`,
    `NeedsSetting(KittyRemoteControl | WezTermMuxSocket)`,
    `NeedsPermission(MacAutomation)`, `Unsupported(NoPaneApi | Wayland |
    NotOnThisPlatform | NotImplemented)`), and `tabs_share_window`. Report
    `Unsupported(NotImplemented)` for anything not built yet.
  - `panes`: every Kitty / WezTerm pane the API lists, as `PaneInfo {
    terminal, pane: KittyWindow { socket, id } | WezTermPane { socket, id },
    tty, pid }` (`pid` = the pane's own process, e.g. its shell). `Known(vec![])`
    when no API is enabled.
  - `tmux`: every server reachable by socket: `TmuxServer { socket, server:
    ProcessIdentity, panes: [{ pane: "%N", tty, session: "$N", window: "@N"
    }], clients: [{ tty, pid, session, active_pane }] }` (`list-panes -a -F
    '#{pane_id}\t#{pane_tty}\t#{session_id}\t#{window_id}'`, `list-clients
    -F '#{client_tty}\t#{client_pid}\t#{session_id}'` plus the active pane).
    `Unknown` when a socket exists but cannot be read.
  - `windows`: X11 / Windows only: every visible top-level window with its
    owner's full `ProcessIdentity` (X11 `_NET_WM_PID`; Win32
    `GetWindowThreadProcessId` + creation time). `Known(vec![])` on macOS
    and Wayland.
- `last_seen_host` (latest) and `original_host` (first) from the
  integrator's history (`Host::Terminal(kind)`, `Host::Tmux(Some(outer))`,
  `Host::Desktop(app)`, `Host::Headless`); `terminal_profile` the Windows
  Terminal profile (`WT_PROFILE_ID`) the session's window had (`Known(None)`
  when none or not Windows Terminal).
- `default_terminal: Fact<Option<TerminalKind>>`: the operating system's
  default terminal. `Known(Some(kind))` for a terminal this crate models
  (installed or not: `decide` checks), `Known(None)` when it was read and
  none is set or it is another terminal, `Unknown` when it could not be read.
  macOS: the LaunchServices handler of `public.unix-executable` (what "Make
  iTerm2 Default Term" sets; Terminal when nobody changed it), in the same
  JXA call as `apps`. Linux: the first of `xdg-terminals.list`
  (xdg-terminal-exec), the `x-terminal-emulator` alternative, GNOME's
  `default-applications.terminal exec` (under GNOME only); `$TERMINAL` is not
  read. Windows: `HKCU\Console\%%Startup` `DelegationConsole` /
  `DelegationTerminal`; "Let Windows decide" is `Known(None)` (not assumed to
  be Windows Terminal).
- `reservation` from the store (`common::reservations`; serde round-trips
  `seen`), `cwd`/`cwd_exists`/`environment` and `provider` evidence from
  `common` observers, `capabilities` only for natively verified,
  version-matched behaviours.

### Linux adapter must fill in

`platform: Linux { session: Known(X11 | Wayland) }` (from
`XDG_SESSION_TYPE` / `WAYLAND_DISPLAY`; `Unknown` if neither), the `/proc`
table with ttys (`/dev/pts/N`) and `WINDOWID` per process, `apps` for the
Linux terminals (desktop-file ids), `schemes` `Known(vec![])` until a
provider app exists there, `surfaces.terminals` for Ghostty, Kitty,
WezTerm, Alacritty, GNOME Terminal, Konsole, xterm (focus by API for
Kitty/WezTerm when enabled; `raise_window: Available` on X11 via `xdotool`,
`Unsupported(Wayland)` on Wayland; `activate_app` `Unsupported`; `launch`
per terminal: `kitty -e`, `wezterm start --cwd`, `gnome-terminal
--working-directory --`, `konsole -e`, `xterm -e`, `alacritty
--working-directory -e`), `panes` (Kitty `kitty @ --to <socket> ls`, WezTerm
`wezterm cli list --format json`), `tmux`, and on X11 `windows` plus
`front.window` (`xdotool getactivewindow`), and `default_terminal` (see
"Every adapter, every snapshot").

### Windows adapter must fill in

`platform: Windows`, a process table **without ttys** (identity = pid +
creation time; `boot` = system boot time), `exe` `WindowsTerminal` /
`ConsoleHost` (`conhost.exe`, `OpenConsole.exe`) / agent CLIs, `apps` for
Windows Terminal and the console host, `surfaces.terminals` for
`WindowsTerminal` (`focus_pane: Unsupported(NoPaneApi)`, `raise_window:
Available`, `tabs_share_window: true`, `launch: Available` via `wt.exe
new-tab [--profile <id>] --startingDirectory <cwd> -- <argv>`) and
`ConsoleHost` (raise, launch, `tabs_share_window: false`), `windows`
(EnumWindows visible top-level, owner identity), `front.window`
(`GetForegroundWindow`), `front.pane_tty: Known(None)`, `tmux: Known(vec![])`
unless observing a WSL runtime, `terminal_profile`, and for WSL sessions
`runtime: Wsl { distro }` with the distribution's process table and POSIX
tool paths, and `default_terminal` (the console delegation, also for a WSL
session: its terminal is a Windows one). Grok is refused by the core on
Windows.

### `Step`s each adapter executes

Every step re-checks the identities it carries against the machine
**before acting**, and answers `Dispatched`, `Opened { tty }` (a launch whose
new window's tty is known), `Refused` (a permission prompt was declined,
nothing ran) or `Failed(reason)` (`TargetGone` when an identity no longer
matches, `HostUnsupported` / `PlatformUnsupported` for anything not
implemented). Dispatch is never proof; the engine observes again.

| Step | macOS | Linux | Windows |
|---|---|---|---|
| `FocusPane { terminal, pane: Tty { tty }, process }` | Ghostty / Terminal / iTerm2: re-read the table, `process` must still be on `tty`, then the constant AppleScript | `HostUnsupported` | `HostUnsupported` |
| `FocusPane { pane: KittyWindow { socket, id } }` | as Linux, with `<kitty.app>/Contents/MacOS/kitty` | `kitty @ --to <socket> ls` must list `id` holding `process` (by tty or pid), then `focus-window --match id:<id>`; a stale socket is `TargetGone`, a refused remote control `Refused` | — |
| `FocusPane { pane: WezTermPane { socket, id } }` | as Linux (`cli --no-auto-start`), with `<WezTerm.app>/Contents/MacOS/wezterm` | `wezterm cli list` with `WEZTERM_UNIX_SOCKET=<socket>`, same checks, `activate-pane --pane-id` | — |
| `FocusPane { pane: X11Window { id, owner } }` | — | `_NET_WM_PID` of `id` must be `owner` (pid and start), then `xdotool windowactivate --sync` | — |
| `FocusPane { pane: WindowsWindow { hwnd, owner } }` | — | — | `IsWindow`, owner pid + creation time must match, `ShowWindow(SW_RESTORE)` + `SetForegroundWindow`; refused foreground → `Failed(DispatchFailed)` |
| `FocusPane { pane: TmuxPane { socket, server, pane, client_tty, outer } }` | as Linux, one tmux run with `;` words | `tmux -S <socket>`: the server process must be `server`, `%pane`'s tty must be `process`'s tty, a client on `client_tty` must show its session; `switch-client -c`, `select-window`, `select-pane`; then focus `outer` in `terminal` as above (the client is `outer`'s process) | — |
| `OpenUrl { url, bundle }` | `open -b <bundle>` after re-checking the scheme handler | only once a provider app exists | same |
| `ActivateApp { app }` | `open -b <bundle>` | `HostUnsupported` unless implemented | `HostUnsupported` unless implemented |
| `Launch { terminal, command, context }` | Ghostty / Terminal / iTerm2 by script; Kitty, WezTerm, Alacritty by `open -n -b <bundle> --args <argv…>`; `context` must be `Native` without profile; a tmux program only with the attach argv | the terminal's own argv form, cwd + allowlisted env, never a shell string; `Native` only | `wt.exe new-tab` (or a console) with `context.profile`; `context.runtime = Wsl { distro }` runs `wsl.exe -d <distro> --cd <cwd> -- <program> <args…>` |
| `Stop { process }` / `AwaitExit` | `kill -TERM` only to an agent CLI with the exact identity and the person's uid | `kill -TERM`, same checks | `TerminateProcess` only with the exact pid + creation time; `AwaitExit` is the engine's |

A tmux attach is a `Launch` whose program is the resolved `tmux` and whose
argv is exactly `-S <socket> select-window -t @W ; select-pane -t %P ;
attach-session -t $S` (each `;` its own argv element).

## Verified natively

Observed on the running apps with disposable sessions (`facts-native.md`,
2026-11-11). Plans rely on each only for the version shown; the version gate
degrades every other version.

| Behaviour | Version | Native | What the planner does |
|---|---|---|---|
| `claude://code/continue?session=<local>` focuses a live record and moves its `lastFocusedAt` | Claude.app 5.1231.2 | N1 | `lastFocusedAt` newer than before ⇒ `Verified` |
| Archiving sets `isArchived`, stops the process and deletes its `sessions/<pid>.json` | 5.1231.2 | N2 | The sim archives the same way |
| `continue` on an archived record changes nothing (no unarchive, no `lastFocusedAt`) | 5.1231.2 | N3 | Never used for archived records; still archived ⇒ `Failed(WrongDestination)` |
| `claude://claude.ai/epitaxy/<local>` shows an archived record without reviving it | 5.1231.2 | N4 | Recorded only (provider notes); no plan uses it |
| `claude://resume?session=<cli uuid>` revives the same archived record in place, `lastFocusedAt` updated, no new record, process started lazily | 5.1231.2 | N5 | Reopen for archived records, verified by `lastFocusedAt` |
| A plain `codex` TUI holds no lock and exposes no thread id; the daemon holds the lock and the rollout | Codex CLI 0.167.0 | N8, N9 | Argv-only TUIs are `RightWindow`; an unlocatable one does not block |
| A second `codex resume <id>` client is accepted and is not a second writer | 0.167.0 | N10 | Resume next to an unlocatable TUI, labelled `PossiblySecondView`; an argv-named TUI is focused instead |
| A launched TUI may stop at a blocking "Update available" menu | 0.167.0 | N12 | Launch stays `DispatchedUnverified`; the window is focused next time |
| An OpenCode TUI is a pure client of the shared server; starting one creates no session and keeps the server | OpenCode 2.2.6 | N13, N14 | Another TUI labels a resume `PossiblySecondView`, never refuses it |
| ghostty (uid 501) → `/usr/bin/login` (euid 0, ruid 501) → shell | Ghostty 1.4.0 | N15 | `ProcessInfo` has `euid` and `ruid`; the ancestor walk filters by neither |
| `every terminal whose tty is "<tty>"` finds exactly one live tty, none for an unknown one | 1.4.0 | N16 | Focus by tty |
| `new window with configuration` reports its tty within ~1.5 s; `focus` reads back as the front window's focused terminal | 1.4.0 | N17 | `Opened { tty }`, `PaneFocused` read-back |
| A configured command runs as `login -flp <user> /bin/bash … -c exec -l <command>`; the terminal's `pid` is that `login`; `working directory` is empty right after launch | 1.4.0 | N18 | Verification uses tty and the process table only, never `pid` or `working directory` |

## UNVERIFIED assumptions and the world variable that models each

Every random run draws all of these variables. The invariants must hold under
both values of each.

| UNVERIFIED behaviour (facts file) | World variable | What the planner does |
|---|---|---|
| `claude://resume` unarchives on an app version other than 5.1231.2 (Claude B; VERIFIED for 5.1231.2, N5) | `claude_resume_link_unarchives` (only read for another version) | Never reached: the gate refuses the link on another version |
| `resume` with an id found only in prior ids imports a duplicate (Claude B, summary 2) | `claude_resume_prior_imports_duplicate` | Only with `allow_import`, and never if the record's current conversation runs elsewhere |
| A pre-warmed spare has a registry file before it is claimed (Claude summary 9) | `claude_spare_registry_before_claim` | Spare claims are never owners |
| The registry `sessionId` is rewritten after /clear or /resume (Claude summary 5; native N7 blocked: CLI not signed in) | `claude_registry_rewritten_on_clear` | Without the capability: `Confirm` when another Claude CLI may host the session; with it: no question |
| Selecting a desktop session starts its process at once (Claude C; N5 saw none within 14 s for `resume`) | `claude_continue_starts_process` | Nothing depends on it any more: `lastFocusedAt` verifies a link either way |
| A TUI started under another CLI version (or `--no-daemon`) holds its own lock (Codex 17; VERIFIED false for the 0.167.0 daemon, N8) | `codex_tui_holds_lock` (only for TUIs started under another version) | Own lock ⇒ `ExactConversation`; such launches are refused by the gate |
| A Codex TUI can switch thread after launch (Codex 15/17; Claude D.3 by analogy) | `codex_tui_switches_thread` | `ExactConversation` only with the TUI's own lock; argv-only is `RightWindow` |
| Codex refuses a second writer on one thread (Codex 26): moot between daemon clients (N10); open for the daemon next to the desktop's app-server or an own-lock TUI | `codex_second_writer_refused` | Never starts a second writer |
| `codex resume <archived id>` works without unarchiving (Codex 6) | `codex_resume_archived_works` | Refuses the CLI path for archived threads |
| The desktop link for an archived thread shows that thread (Codex 6, 9) | `codex_link_archived_shows_thread` | Uses the link; the post-condition catches a wrong destination |
| IPC `thread-owner-discovery` works on build 25894 (Codex 13) | `codex_ipc_owner_discovery_works` | Uses the answer when there is one |
| The OpenCode service outlives the desktop app (OpenCode A) | `opencode_server_survives_quit` | Version checks against the running server or the desktop |
| The TUI opens an archived or child id given to `-s` (OpenCode F) | `opencode_tui_opens_archived_or_child` | Refuses archived and child sessions |
| Which conversation a Codex or OpenCode desktop shows can be observed (Codex 13, OpenCode D) | `desktop_view_readable` | Never for Claude, whose record's `lastFocusedAt` is used instead |
| `grok --resume <id>` resumes the session; desktop-core uses it, relayhistory writes `grok resume <id>` (facts-grok 4) | `grok_resume_flag_works` | Resumes with `--resume`; a launch that never starts stays `DispatchedUnverified` and keeps its reservation |
| `--resume` with an unknown id silently creates it (facts-grok 4) | `grok_resume_missing_creates` | Launches only after `summary.json` positively confirms the id |
| A resume from another directory does not find the session (facts-grok 2) | `grok_resume_needs_cwd` | Always runs in the session's own `info.cwd`; another recorded directory is `CwdConflict`; an id stored under two directories is `Unknown` |
| The `active_sessions.json` record follows `/new` and `/resume` (facts-grok 3) | `grok_registry_follows_session` | Focus is `RightWindow`; a resume next to another Grok process in the directory asks first (`Confirm`) unless a version-matched `GrokRegistryFollowsSession` capability says the registry is authoritative |
| A TUI started with `--resume` records itself (facts-grok 3) | `grok_resumed_tui_registers` | A process whose argv names the session is an owner, record or not |
| `--resume <subagent id>` opens the child (facts-grok 6) | `grok_subagent_resumable` | Refuses `ChildSession` |
| Kitty `focus-window` / `wezterm cli activate-pane` also raise the OS window (X11, macOS; the old GTK code only claims `Dispatched`) | `pane_api_focus_raises` | Verified only when the front pane reads back; otherwise `Failed(WrongDestination)` |
| On Wayland, the compositor lets that raise through | `wayland_pane_focus_raises` | Same; generic Wayland raising is never assumed (`WaylandNoRaise`) |
| The X11 window manager honours `xdotool windowactivate` (`_NET_ACTIVE_WINDOW`) | `x11_activation_honored` | Verified only when the active window reads back |
| `SetForegroundWindow` from Relay is allowed (foreground lock) | `windows_foreground_allowed` | `Failed(DispatchFailed)` when refused, never a launch |
| A classic console window reports its first client (not `conhost.exe`) as owner | `console_window_owner_is_client` | Accepts either owner, both identity-checked |
| `wt new-tab` joins an existing window (`windowingBehavior`) | `wt_new_tab_joins_window` | Windows Terminal windows are app-level precision; several are `WindowAmbiguous` |

The Codex "Update available" menu (N12) is world state, not a variable: a
quarter of the seeded worlds have an update available, and every TUI started
there waits at the menu until the person dismisses it (`DismissUpdate`).

### Capabilities (version-gated, switched on by a native adapter)

`evidence::Capability` records an assumption an adapter has verified natively
for one component version; policy relies on it only while the observed
version matches.

| Capability | Effect when present for the installed version | Modelled by |
|---|---|---|
| `ClaudeRegistryFollowsSession { cli_version }` | The registry is authoritative: no `Confirm` for unrelated Claude CLIs | `claude_registry_rewritten_on_clear` (the sim reports it for 2.1.321 in those worlds) |

### Assumptions handled by policy instead of a variable

- **Two OpenCode clients prompting one session at once (OpenCode A; N14 did
  not test it).** A second client is not a second writer (N13, N14), so
  another TUI only labels a resume `PossiblySecondView`. The planner still
  never starts a TUI next to a running desktop (it brings the desktop forward
  instead), while the server reports the session busy or cannot say, for a
  missing, archived or child id, or across a server version mismatch. It
  never launches the desktop (which restores its saved tabs) while a turn
  runs on one of them, while one of our pending launches holds one, while
  the tabs are unknown, or (`ServerVersionMismatch`) while a TUI launch of
  ours is still pending and the desktop's version differs from the CLI's:
  that TUI would replace the desktop's new server (found by the simulator,
  regression seeds 1832153 and 5126032). The same open question applies to two Codex clients
  of the daemon (N10).
- **OpenCode resume directory (OpenCode F).** The TUI runs in the session's
  own `directory`; other evidence naming another directory is `CwdConflict`.
- **Claude `continue` / `resume` for another account (Claude summary 3).**
  A record of another account gets no link (`OtherAccount`); an unknown
  current account gets none either.
- **Terminal.app's version.** It was not recorded, so Terminal plans are not
  version-gated.
- **The `entrypoint` value of a terminal-started Claude CLI.** The simulator
  writes `cli`. Any value other than `claude-desktop` is treated as
  terminal-origin.
- **Where the CLIs live.** No `claude` is on PATH on the surveyed Mac (Claude
  correction 4). Plans run the resolved absolute path from evidence; the
  generator removes CLIs (`UninstallTool`) so `ToolNotInstalled` is exercised.

## Verify natively first

(The measured counts below are from the first, macOS-only round; the
current per-provider and per-host numbers are printed by `long_random_runs`.
Add for the new hosts: the Kitty/WezTerm raise on focus, X11 activation
and Windows foreground rules, the console window owner, `wt new-tab`
window joining, and every Grok row above, since Grok was not installed.)

Ordered by how much usefulness or safety each unknown costs today (measured
counts are from `long_random_runs`; see the end of `cargo test --test
simulation long_random_runs -- --nocapture`).

1. **Claude registry follows /clear and /resume (summary 5, native N7:
   blocked, the CLI was not signed in).** Without it, Claude resumes and
   desktop `continue` links next to another Claude CLI in the same repo need
   a confirmation (1.7% of Claude opens; 5% of `continue` links, 14% of
   `resume` links), and Claude terminal focus stays `RightWindow`. One
   experiment (/clear in a signed-in CLI, read `sessions/<pid>.json`)
   switches on the capability.
2. **Two clients prompting one session at once (Codex N10, OpenCode N14 left
   it untested).** Plans now open a second client next to an unlocatable
   Codex or OpenCode TUI (labelled `PossiblySecondView`). If simultaneous
   prompting from two clients misbehaves, those plans must go back to
   refusing (they were 19,854 OpenCode `PossiblyOpenElsewhere` refusals and
   most of Codex's 5,008 `LiveNotLocatable`).
3. **Codex lock while the "Update available" menu shows, and `--no-daemon`
   (N12, N8).** Codex resume verification assumes a TUI at the menu has not
   made the daemon take the lock. Today 57% of Codex resumes end
   `DispatchedUnverified` (menu worlds, second views, slow launches).
4. **Claude `continue` on the record already on screen, and slow app starts
   (N1 only saw null → timestamp).** Decides whether an unchanged
   `lastFocusedAt` could be called `Failed`; it is `DispatchedUnverified`
   today.
5. **Codex desktop link and `codex resume` for archived or unknown threads,
   IPC owner discovery (Codex 6, 9, 13).** 1,728 Codex `Archived` refusals.
6. **OpenCode `-s` on archived or child ids, server lifetime after desktop
   quit, version-mismatch restart (OpenCode A, F, G).** 5,726 `Archived` and
   4,491 `ChildSession` refusals; 149 `ServerVersionMismatch`.
7. **Claude links for another account, `resume` with a prior-only id, idle
   timeout (summaries 2, 3).** 1,733 `PriorIdentityOnly` refusals; other
   accounts are refused by policy.
8. **Terminal.app tab focus and version.** Not gated; every Terminal focus
   trusts the executor.

## Native adapters (`native/`)

`native/` (crate `session-navigation-native`) implements `Observe`, `Execute`
and `Reservations` for a real Mac (`src/macos/`); `MacPorts` bundles them for
`engine::open`. The OS-independent observers (Claude, Codex, OpenCode, Grok),
quoting, reservations and tool resolution live in `src/common/`; provider
homes come from `common::config::ProviderPaths`, with defaults per platform
(`macos::provider_paths`, `linux::provider_paths`,
`windows::provider_paths`; the Linux and Windows values are UNVERIFIED). The
Linux and Windows modules hold only those defaults so far. The mac adapter
scripts Ghostty, Terminal and iTerm2 by tty, drives Kitty and WezTerm
through their own APIs, activates and launches Alacritty (no pane API),
lists and focuses tmux panes, and reports each terminal's `TerminalSupport`
from what it observed (`macos/hosts/`); the OS-independent Kitty, WezTerm and
tmux parsers and argv builders are `common::{kitty, wezterm, tmux}`. It
refuses any other `PaneRef`, terminal or launch context with
`HostUnsupported` / `PlatformUnsupported` without running anything.
All I/O goes through the `System` trait (run an absolute program with an argv
vector, read/list/stat files, write atomically, clock). `RealSystem` uses
`std::process::Command` with no shell, a cleared environment plus
`PATH=/usr/bin:/bin:/usr/sbin:/sbin`, a polling watchdog that kills on
timeout, and capped output. `FakeSystem` replays scripted runs and an
in-memory file tree and records every call; every adapter test uses it, none
touches the machine. All defaults live in `MacConfig::for_user(home, uid)`.

What runs:

| Source | Command or file | Failure → |
|---|---|---|
| boot | `/usr/sbin/sysctl -n kern.boottime` | boot and table `Unknown` |
| process table | `TZ=UTC /bin/ps -axww -o …` three times (`ucomm`, `comm`, `args`), joined on pid + start | whole table `Unknown` |
| agent CLI cwds | `/usr/sbin/lsof -a -d cwd -Fpn -p <pids>` | that cwd `Unknown` |
| apps, schemes, front, running, default terminal | one `osascript -l JavaScript -e <constant>` with bundle ids as argv; the default terminal is `NSWorkspace URLForApplicationToOpenContentType(UTType 'public.unix-executable')` (`UTType` by `NSClassFromString`) mapped by bundle id | `apps`, `schemes`, `front.app`, `default_terminal` `Unknown`; `decide` refuses what needs them |
| front pane tty | `osascript -e <constant>` (Ghostty / Terminal / iTerm2); never on a passive observer (`MacObserver::set_passive`) | `Unknown`; -1743 also makes that terminal's focus and launch `NeedsPermission(MacAutomation)` |
| Kitty panes, front pane | sockets `/private/tmp/<name>-<kitty pid>`; `<kitty.app>/Contents/MacOS/kitty @ --to unix:<sock> ls` | no socket, stale or refused → no panes, `focus_pane: NeedsSetting(KittyRemoteControl)`; any other failure → `panes` `Unknown` |
| WezTerm panes, front pane | sockets `~/.local/share/wezterm/gui-sock-<gui pid>`; `wezterm cli --no-auto-start list` / `list-clients --format json` with `WEZTERM_UNIX_SOCKET` | as Kitty, `NeedsSetting(WezTermMuxSocket)` |
| tmux | sockets in `<TMUX_TMPDIR or /private/tmp>/tmux-<uid>`; `tmux -S <sock> list-sessions -F '#{pid}'`, `list-panes -a -F …`, `list-clients -F …`; server identity from the table | no directory → `Known([])`; stale socket skipped; anything unreadable → `Unknown` |
| tools | search path, executable files; `<path> --version` (`tmux -V`; Grok's version from its canonical `grok-X.Y.Z-…` name, never run), cached per (path, mtime, size) | `Unknown` with path |
| Grok | `$GROK_HOME` (else `~/.grok`) `/active_sessions.json`; `…/sessions/<encoded cwd>/<id>/summary.json`, the directory name must decode to `info.cwd`; a non-default home is recorded as `environment = {GROK_HOME}` | per source `Unknown`; an id under two directories, or a name/cwd mismatch, `Unknown` |
| Claude | `~/.claude/sessions/<pid>.json`, `…/claude-code-sessions/<account>/<org>/local_*.json`, `~/.claude/projects/*/<id>.jsonl` (1 MiB prefix) | per source `Unknown` |
| Codex | `/usr/bin/sqlite3 -readonly -json` on `state_5.sqlite` (id validated, `[0-9a-f-]` only); rollout file if no database; `lsof -Fp -- <lock>` | `Unknown` |
| OpenCode | `service.json`; `/usr/bin/curl` GETs to loopback only, Basic auth via `--config -` on stdin; `sqlite3 -readonly` on `session_v2` when down; `drafts.sqlite` tabs; `tui/tabs.json` | `Unknown` |
| reservations | `~/Library/Application Support/session-navigation/reservations/<provider>-<hex id>.json`, dir 0700, temp + rename | `Unknown` (blocks launches) |

A **passive** observer (`MacObserver::set_passive(true)`, or
`ports.observer.set_passive(true)` on `MacPorts`) never runs terminal
AppleScript: the front-pane read is skipped and `front.pane_tty` is
`Unknown`. The only look that can raise a macOS Automation prompt is that
read, so an integrator uses a passive look for anything that runs without a
click (drawing a button). The JXA registry query names no Automation target
and still runs (`tests/macos_passive.rs`). Kitty and WezTerm are still
listed through their own APIs; a Kitty set to `allow_remote_control
password` may ask the person to confirm a request without one (UNVERIFIED).

Steps: `OpenUrl` → `/usr/bin/open -b <bundle> <url>` after re-checking scheme,
bundle and characters; `ActivateApp` → `open -b`; `FocusPane` re-reads the
table, then runs a constant AppleScript with the tty as argv (-1743 →
`Refused`); `Launch` checks program, cwd and the tool's environment
allowlist, POSIX-quotes the argv and passes cwd/command/env as script argv;
`Stop` re-reads the table and sends `/bin/kill -TERM` only to an agent CLI
with the same identity and real uid.

Verified on this Mac: the `sysctl`, `ps` (including `ucomm` being the kernel
name and `comm` being argv[0]) and JXA output shapes. Not verified natively
(the adapter scripts were never run): the focus and launch scripts, Ghostty
`environment variables` and `initial working directory` with the quoted
command, Terminal `do script` returning a tab tty, `lsof` exit codes, curl
`--config -` against the OpenCode server, `sqlite3 -json` on the live
databases, and `<tool> --version` with an empty environment. The Codex
desktop window owner (IPC) is not implemented and always `Unknown`; the
Claude desktop's current account is `Unknown` unless exactly one
`<account>/<org>` directory exists.

#### macOS terminal hosts: UNVERIFIED assumptions

None of iTerm2, Kitty, WezTerm, Alacritty, tmux or Grok was installed on the
Mac this adapter was written on. Every row is documentation-derived; the
tests replay documentation-derived fixtures (`native/tests/fixtures/macos/`,
`fixtures/grok/`, each with a `PROVENANCE.txt`). One action verifies each.

| Assumption (where in code) | Verify with |
|---|---|
| iTerm2 `tty of current session of current window`, `select` on window/tab/session, and `create window with default profile command` returning a window whose `current session` has the new tty (`macos/scripts.rs`) | Run `ITERM_FRONT_TTY`, `ITERM_FOCUS <tty>` and `ITERM_LAUNCH` via `osascript` with iTerm2 open |
| iTerm2 splits `command` into words honouring single quotes (`execute/launch.rs` `iterm_command`) | Launch an argv with a space, `'`, `$` through `ITERM_LAUNCH`; compare `ps -o args=` |
| Kitty appends `-<pid>` to a kitty.conf `listen_on unix:/tmp/<name>` (`common/kitty.rs` `socket_owner_pid`) | `listen_on unix:/tmp/kitty` + `allow_remote_control yes`; `ls /tmp/kitty-*` |
| `kitty @ ls` JSON shape (ids, `pid`, `is_focused` per level) and its error texts for a disabled / denied / unreachable socket (`common/kitty.rs`) | `kitty @ --to unix:/tmp/kitty-<pid> ls`, then with remote control off and with Kitty quit |
| `kitty @ focus-window` raises the OS window on macOS (`pane_api_focus_raises`) | Focus a window in a background Kitty; check the front app and `ls` `is_focused` |
| `kitty -o env=K=V --directory <cwd> <program> <args>` via `open -n -b net.kovidgoyal.kitty --args …` (`common/kitty.rs` `launch_args`) | Run that `open` line with a test argv; `ps -E` the child |
| WezTerm GUI socket `~/.local/share/wezterm/gui-sock-<pid>`; `cli --no-auto-start list` / `list-clients` JSON (`tty_name`, `focused_pane_id`, the GUI as its own client) (`common/wezterm.rs`) | `ls ~/.local/share/wezterm`; `WEZTERM_UNIX_SOCKET=<sock> wezterm cli --no-auto-start list-clients --format json` |
| `wezterm start --cwd <cwd> -- <argv>` accepted by the app's bundle executable through `open -n -b com.github.wez.wezterm --args` | Run that `open` line |
| `alacritty --working-directory <cwd> -e <argv>` through `open -n -b org.alacritty --args` | Run that `open` line |
| tmux socket dir `/private/tmp/tmux-<uid>`, `list-sessions -F '#{pid}'`, `#{pane_id}` in `list-clients` being the client's active pane, the "no server running" text (`common/tmux.rs`) | `tmux -S /private/tmp/tmux-$(id -u)/default list-clients -F '#{client_tty} #{pane_id}'` with two windows |
| `switch-client -c <tty> -t $S ; select-window -t @W ; select-pane -t %P` in one run selects the pane in that client (`common/tmux.rs` `focus_args`) | Run it against a two-window session and read `list-clients` back |
| Grok store layout and `GROK_HOME` handling (`common/grok.rs`) | With Grok installed: `ls $GROK_HOME/sessions/*/`, `cat …/summary.json`, `cat $GROK_HOME/active_sessions.json` |
| A Grok TUI under another `GROK_HOME` than Relay's is not observed (a process's own environment is not read on macOS) | Start `GROK_HOME=/tmp/g grok`; observe; its record is absent |

Read-only check: `cargo run -p session-navigation-native --example observe --
<provider> <session-id> [--terminal <name>]` prints the snapshot (argv
redacted), the decision with no terminal preference (or the one given), and
which default terminal that decision would use; it executes nothing.

#### macOS fast path (kernel reads) and one look's cost

When `System::kernel()` answers (`RealSystem` on macOS; a `FakeSystem` given a
`ScriptedKernel`), the process table, boot and agent cwds are read directly
instead of by `sysctl`, three `ps` and `lsof`. The only `unsafe` of the macOS
adapter is `native/src/system/kernel_macos.rs` (`libc` FFI; every block has a
`// SAFETY:` note, Rust-owned buffers with their length passed, returned
lengths checked before slicing, no transmute); the byte parsers are safe code
in `system/kernel_parse.rs`. `#![forbid(unsafe_code)]` stays in `core`; the
native crate denies it except in its FFI files.

| Field | Kernel read | Same as `ps` because |
|---|---|---|
| boot | `sysctlbyname("kern.boottime")` | the same value `sysctl -n` prints |
| pids | `proc_listallpids` (pid 0 skipped, as `ps -ax`) | |
| ppid, euid, ruid, start, tty dev, `ucomm` | `proc_pidinfo(PROC_PIDTBSDINFO)` for the person's live processes; `sysctl {KERN_PROC, KERN_PROC_PID}` (`kinfo_proc`, fixed LP64 offsets, length and pid checked) for other users' processes (bsdinfo is `EPERM`) and zombies | `ps` reads `kinfo_proc`; `ucomm` is the first 16 bytes of `pbi_name` (`pbi_comm` keeps only 15) |
| tty name | `/dev/tty*` / `/dev/console` scanned by device number, rescanned on a miss | `devname` |
| executable path | `proc_pidpath` | |
| argv | `sysctl KERN_PROCARGS2` (argc + exec path + argv; the environment is never read) | `ps` joins argv with spaces and escapes control bytes (`\012`); the kernel's vector is the real one. Another user's argument area is closed to non-root (`ps` is setuid root): those rows carry `[exe path]`; core reads argv only of the person's agent CLIs |
| agent cwd | `proc_pidinfo(PROC_PIDVNODEPATHINFO)` | what `lsof -d cwd` prints |

A pid that vanishes mid-scan, or whose start time changes between the two
reads of its basics, is left out; a pid neither call can read makes the table
`None`, and the adapter then runs `sysctl` / `ps` / `lsof` exactly as without
a kernel (the snapshot says nothing different). `kernel_agreement`
(opt-in, read-only) checks, on this Mac, every row of the kernel table against
the `ps` table (ppid, uids, tty, `exe` class, argv words where readable), the
agent cwds against `lsof`, boot against `sysctl`, and the test's own
pid/ppid/start/exe/argv/cwd.

Reads run at once where independent (`System::concurrent()`: `RealSystem`
yes; `FakeSystem` no unless `set_concurrent(true)`, so its calls keep one
stable order): the kernel table, the registry, the tools and (except Claude,
whose evidence needs the table) the provider stores; then Claude's evidence,
the front pane and the Kitty / WezTerm / tmux probes. The front app comes
from `/usr/bin/lsappinfo front` + `lsappinfo info -only bundleID <asn>`
(~6 ms each, both LaunchServices, like `NSWorkspace.frontmostApplication`;
the answer must still say `(in front)`), else from a fresh JXA query. The
running apps are told from the kernel table: an installed app runs when a
process of the person executes its bundle executable (the JXA query now
also reports `NSBundle.executablePath`, canonicalized).

**What a look reuses (at most 2 s, inside one observer, so the 2nd–4th look
of one `engine::open` share it; `set_reuse(Duration)` changes the window,
zero turns it off):** installed apps and their versions and executables,
URL scheme handlers, the system default terminal (macOS: the JXA registry
report, fast path only; Linux: `xdg-terminals.list` / `x-terminal-emulator`
/ `gsettings`), resolved tool paths and `--version` output (both platforms;
versions are also cached per (path, mtime, size) for the observer's life,
as before), Linux helper paths (`which` of launchers, `xdotool`, `wmctrl`,
`env`, `kill`) and installed apps, and on Linux the `environ` identity keys
of a process instance (boot, pid, start ticks) already read.

**Fresh every look:** boot, the process table (incl. tty, argv, `window`),
agent cwds, the front app, front pane / tty, Kitty and WezTerm panes, tmux
servers, X11 windows and the active window, running apps, reservations, and
every provider store (sqlite3, files, curl, lock holders).

Measured on this Mac (release build, `cargo run --release --example timing
-- codex <id>`; a Claude session is within a few ms):

| | before | after |
|---|---|---|
| process table | 3 × `ps` ≈ 37 ms each + `sysctl` 7 ms + `lsof` 43 ms, in turn | one kernel scan ≈ 8 ms alone, 15–19 ms while the other reads run |
| front + running apps | in the JXA query, every look (60–80 ms) | `lsappinfo` ≈ 13 ms + the table |
| first look of a new observer | 275–430 ms | 63–113 ms (JXA ≈ 60 ms, first `--version` runs up to ~100 ms, both at once) |
| later look, same observer, within 2 s | ≈ 217 ms | ≈ 13.5 ms |
| a look after the window | ≈ 217 ms | ≈ 75–90 ms (the JXA query again) |
| `engine::open` with 3 looks | ≈ 0.65–0.9 s | ≈ 90–140 ms |
| the same with Ghostty frontmost (its front-pane AppleScript, ≈ 110 ms, is evidence of now and runs every look) | ≈ 1.0–1.1 s | ≈ 0.42–0.47 s |

### Linux adapter (`native/src/linux/`): UNVERIFIED

`LinuxPorts` (`LinuxObserver`, `LinuxExecutor`, `FileReservations` under
`$XDG_STATE_HOME/session-navigation/reservations`) implements the contract
above for Linux. `LinuxConfig::from_env()` keeps only the desktop-session
variables (`XDG_SESSION_TYPE`, `WAYLAND_DISPLAY`, `DISPLAY`, `XAUTHORITY`,
`XDG_RUNTIME_DIR`, `DBUS_SESSION_BUS_ADDRESS`, `HOME`, `PATH`, …); X11
helpers get `DISPLAY` and `XAUTHORITY`, a launched terminal all of them.
Observing reads `/proc` (`stat`, `status`, `exe`, `cmdline`, `environ` for
`WINDOWID`, `KITTY_LISTEN_ON`, `WEZTERM_UNIX_SOCKET` and `TMUX` only, `cwd`
of agent CLIs), runs `wmctrl -lp` and `xdotool getactivewindow` on X11,
`kitty @ --to <addr> ls`, `wezterm cli --no-auto-start list` /
`list-clients`, and `tmux -S <socket> list-sessions|list-panes|list-clients`.
A launch is spawned detached (`System::spawn_detached`: the terminals stay
in the foreground) and never waited on.

A look runs its independent reads at once on `RealSystem` (the `/proc`
table, the tools, the slow-changing facts, the non-Claude provider stores;
then Claude's evidence, the Kitty / WezTerm APIs, tmux and the X11 window
reads) and reuses the facts listed under "What a look reuses" for at most
2 s. Every own process's `environ` is still read (its `WINDOWID` is the
row's `window`, and any process can carry `KITTY_LISTEN_ON` / `TMUX`), but
only once per process instance within that window. UNVERIFIED and
unmeasured on Linux: no timing exists; the reuse is checked only against
`FakeSystem` (`tests/linux_reuse.rs`).

It was written on macOS and **never run on Linux**; every test drives
`FakeSystem` with the documented formats in `native/tests/fixtures/linux/`.
Each assumption, and the command that would verify it:

| UNVERIFIED assumption (where) | Verify with |
|---|---|
| `/proc/<pid>/stat`, `status`, `/proc/stat` `btime`, `boot_id` formats; `comm` cut at the last `)` (`procfs.rs`) | `cat /proc/self/stat /proc/self/status; grep btime /proc/stat; cat /proc/sys/kernel/random/boot_id` |
| `CLK_TCK` is 100 (`LinuxConfig::clock_ticks`) | `getconf CLK_TCK` |
| `btime` can move with a clock step, so it is read once per boot id (`table::BootClock`) | compare `grep btime /proc/stat` before and after `sudo date -s "+1 sec"` |
| pts majors 136–143, minor split per proc(5) (`procfs::tty_path`) | `ls -l /dev/pts/0; awk '{print $7}' /proc/self/stat` |
| `exe` reads `… (deleted)` after an upgrade; another user's `exe`/`environ`/`cwd` are denied, kernel threads' `exe` is missing (`table.rs`) | `readlink /proc/1/exe; readlink /proc/2/exe; readlink /proc/$$/exe` |
| Install layouts: Claude native installer `~/.local/share/claude/versions/<v>`, Codex `codex-<triple>`, Grok `grok-<v>-linux-<arch>`; npm Claude is `node` (`classify.rs`) | `readlink /proc/$(pgrep -n claude)/exe` (likewise codex, grok) |
| Desktop-file ids (`kitty`, `org.wezfurlong.wezterm`, `Alacritty`, `org.gnome.Terminal`, `org.kde.konsole`, `xterm`, `com.mitchellh.ghostty`); a launcher on the program path also counts as installed (`apps.rs`) | `ls /usr/share/applications ~/.local/share/applications` |
| xterm, Kitty, Alacritty, Konsole export `WINDOWID` (decimal) to children | `echo $WINDOWID` in each |
| `wmctrl -lp` row shape, `-1` desktop, pid `0` without `_NET_WM_PID`; `xdotool getactivewindow` / `getwindowpid` print decimals and fail without a pid (`x11.rs`) | `wmctrl -lp; xdotool getactivewindow; xdotool getwindowpid $(xdotool getactivewindow)` |
| `WM_CLIENT_MACHINE` other than this host means another pid space; such windows are left out | `xprop -id <id> WM_CLIENT_MACHINE` for an `ssh -X` client |
| Kitty sets `KITTY_LISTEN_ON`, WezTerm `WEZTERM_UNIX_SOCKET` in every pane (`panes.rs`) | `tr '\0' '\n' < /proc/$$/environ \| grep -e KITTY_LISTEN -e WEZTERM_UNIX` |
| Kitty's `is_focused` on an OS window means it has the desktop's focus (front pane on Wayland) | `kitty @ ls \| jq '.[].is_focused'` with another app focused |
| Each terminal's launch argv (`launch.rs` table: `kitty --directory`, `wezterm start --cwd`, `alacritty --working-directory … -e`, `gnome-terminal --working-directory=… --`, `konsole --workdir … -e`, `xterm -e env -C …`, `ghostty --working-directory=… -e`) | the commands in the `launch.rs` table |
| `env -C` (GNU coreutils 8.28+) for xterm's directory | `env -C /tmp pwd` |
| A spawned terminal survives the caller and needs only the session variables (`spawn_detached`) | launch through `LinuxExecutor` from a GUI session, then quit the caller |
| `xdotool windowactivate --sync` returns once the WM honoured it; a refusing WM times out (`DispatchFailed`) | `xdotool windowactivate --sync <id>; echo $?` |
| Kitty / WezTerm / tmux output shapes and error texts (`common/{kitty,wezterm,tmux}.rs`, shared with macOS) | see those modules |
| The observer user's uid is the owner of `$HOME` (`from_env`) | `stat -c %u ~; id -u` |
| Default terminal, first source: `xdg-terminals.list` in `$XDG_CONFIG_HOME`, `$XDG_CONFIG_DIRS`, then `<data dir>/xdg-terminal-exec/`, `<desktop>-xdg-terminals.list` before `xdg-terminals.list`, one `<id>.desktop[:action]` per line, `-id` excluded, first id with an installed desktop file wins; an unmodelled id is `Known(None)` (`default_terminal.rs`) | `ls ~/.config/*xdg-terminals.list /etc/xdg/*xdg-terminals.list /usr/share/xdg-terminal-exec/`; `xdg-terminal-exec --print-id` |
| Default terminal, second source: `/etc/alternatives/x-terminal-emulator` names the program (`gnome-terminal.wrapper` is GNOME Terminal) | `readlink /etc/alternatives/x-terminal-emulator; update-alternatives --display x-terminal-emulator` |
| Default terminal, third source, GNOME only (the key has a schema default everywhere the schema is installed): `gsettings get org.gnome.desktop.default-applications.terminal exec` prints `'<program> …'`; "No such schema/key" means absent, any other failure unreadable | `XDG_CURRENT_DESKTOP=GNOME gsettings get org.gnome.desktop.default-applications.terminal exec` |

`SESSION_NAVIGATION_NATIVE=1 cargo test -p session-navigation-native --no-default-features --features linux --test linux_native_acceptance -- --nocapture`
runs one read-only observation on a real Linux machine and checks the
process table against the test's own process; anywhere else it is skipped.

Not covered: `OpenUrl` is `PlatformUnsupported` and `ActivateApp`
`HostUnsupported` (no provider app or generic activation on Linux);
`Launch` reports `Dispatched`, never `Opened { tty }` (no terminal CLI
reports the new tty); a missing `xdotool` or `wmctrl` makes `raise_window`
`Unsupported(NotImplemented)` (the contract has no "tool missing"
ability); Ghostty on Linux reports `tabs_share_window: true` (GTK tabs),
unlike the simulator.

### Windows adapter (`native/src/windows/`): UNVERIFIED

`WindowsPorts` (`WindowsObserver`, `WindowsExecutor`, `FileReservations`
under `%LOCALAPPDATA%\session-navigation\reservations`) implements the
contract above for Windows, natively or for one WSL distribution
(`WindowsConfig::native(..)`, `WindowsConfig::wsl(&native, distro,
linux_home)`; one configuration per runtime).

**Fast path (Win32).** When `System::win32()` answers (`RealSystem` on
Windows), observe, the identity re-check, focus and launch are direct
`windows-sys` calls behind the plain-data `system::Win32` trait
(`system/win32.rs`; the only `unsafe` is in `system/win32_real.rs` and
`system/win32_real/`): `CreateToolhelp32Snapshot` (pid, ppid, image name),
`OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `GetProcessTimes` +
`QueryFullProcessImageNameW` + `NtQueryInformationProcess(ProcessCommandLineInformation)`
+ `ProcessIdToSessionId`, boot = `RegGetValueW` of the same
`PrefetchParameters\BootId` (else `NtQuerySystemInformation(SystemTimeOfDayInformation)`
boot time, which also feeds file-mtime checks), `EnumWindows` /
`GetForegroundWindow` / `IsWindow` / `SetForegroundWindow`, `RegGetValueW`
of `HKCU\Console\%%Startup`, `GetPackagesByPackageFamily` for Windows
Terminal, `CreateEnvironmentBlock` + `CreateProcessW` (`CREATE_NEW_CONSOLE`,
or `CREATE_NO_WINDOW` for `wt.exe`; a `.cmd`/`.bat` program runs as
`cmd.exe /d /s /c "<line>"`, its words already `cmd_safe`). `windows/fast.rs`
turns the answers into the scripts' own `Report`, so identity, owner and
default-terminal rules are shared. Any Win32 failure before acting falls
back to the PowerShell path below (a whole observe report, the identity
read, the focus or the launch); a refusal while acting does not. Windows
Terminal's package and the delegation values, and a WSL tools probe that
answered, are reused for at most 2 s within one observer; the table,
windows, foreground, reservations and stores are read every look. A WSL
session's `wsl.exe` probes run on threads beside the native reads.

**Slow path (PowerShell).** Without Win32, every Win32 call lives in
constant Windows PowerShell 5.1 scripts (`windows/scripts.rs`), run as
`powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass
-EncodedCommand <constant>` with any data as JSON on stdin; WSL scripts go
to `wsl.exe -d <distro> --exec /bin/sh -s` on stdin. A Windows Terminal
launch with provider-home variables still runs `ENV_WRAPPER` (PowerShell)
as the new tab's own process on both paths.

| Source | What runs | Failure → |
|---|---|---|
| boot, table, windows, front, Windows Terminal, default terminal | one `OBSERVE` script (also `HKCU:\Console\%%Startup`): `PrefetchParameters\BootId` (else `LastBootUpTime`), `Get-CimInstance Win32_Process`, an `Add-Type` helper (`EnumWindows` visible unowned windows, `GetWindowThreadProcessId`, `GetProcessTimes` creation time, `GetForegroundWindow`), `Get-AppxPackage Microsoft.WindowsTerminal` | each section on its own: `Unknown` |
| identities | pid + creation time (whole Unix seconds) + boot id; `tty: None`; `euid`/`ruid` carry the session id (Windows has no uids); `cwd` `Unknown` (another process's cwd is in its PEB) | — |
| windows | kept only when the helper's owner (pid, creation time) is exactly a table row; a reused pid's window belongs to nobody | `Unknown` with the table |
| tools | `<dir>\<tool>.exe`, then `<dir>\<tool>.cmd` (`common::tools::PathStyle::Windows`), `--version` | as on macOS |
| WSL | `wsl.exe -l -q` (UTF-16LE), a `/proc` probe (`btime + starttime/CLK_TCK`, `/dev/pts/N`), a tools probe over POSIX directories; files through `\\wsl.localhost\<distro>` | `Unknown` |
| focus | `IDENTITY` (fresh table: owner and agent identities in this boot) → `FOCUS` (`IsWindow`, owner pid + creation, agent creation, `ShowWindow(SW_RESTORE)` if minimised, `SetForegroundWindow`, read back) | `gone` → `TargetGone`; refused or not read back → `DispatchFailed` |
| launch | `LAUNCH`: rebuilds the person's environment (volatile, machine, user), then `Start-Process` (`windows/launch.rs` table: `wt.exe -w 0 new-tab [--profile] --startingDirectory -- …`, a new console, `wsl.exe -d <distro> --cd <cwd> --exec [/usr/bin/env K=V…] …`) | `DispatchFailed` |
| stop | native: `HostUnsupported`, nothing runs (no graceful stop); WSL: `/proc` re-read, then `wsl.exe -d <distro> --exec /bin/kill -TERM <pid>` only to an agent CLI with the exact identity and the probe's uid | `TargetGone` |

It was written on macOS and **never run on Windows**. Its `cfg(windows)`
code (`RealSystem`'s Windows branches, `system/win32_real*`) is
type-checked and clippy-clean for `x86_64-pc-windows-msvc`
(`cargo check`/`cargo clippy --target x86_64-pc-windows-msvc`) but has
never been linked or run. The fast path is tested through `FakeSystem`'s
`ScriptedWin32` (`tests/windows_fast_*.rs`: every end-to-end scenario,
fallbacks, refusals, launch lines, reuse). Every other test
drives `FakeSystem` with PowerShell 5.1 shapes in
`native/tests/fixtures/windows/` (`"\/Date(ms)\/"` and `{value, DateTime}`
dates, a bare object for a one-element array, a UTF-8 BOM, UTF-16LE).

| UNVERIFIED assumption (where) | Verify with |
|---|---|
| The scripts parse and run under Windows PowerShell 5.1 with a cleared environment (`scripts.rs`, `RealSystem`); the Win32 table agrees with `Win32_Process` on pid, ppid, creation second and image, and the boot id with the script's; the timings of both paths | `SESSION_NAVIGATION_NATIVE=1 cargo test -p session-navigation-native --no-default-features --features windows --test windows_native_acceptance -- --nocapture` |
| Win32 fast path: `NtQueryInformationProcess(ProcessCommandLineInformation)` with `PROCESS_QUERY_LIMITED_INFORMATION` reads other processes' command lines (Windows 8.1+); `SystemTimeOfDayInformation`'s boot time floors to `LastBootUpTime`'s second; `GetPackagesByPackageFamily("Microsoft.WindowsTerminal_8wekyb3d8bbwe")` matches `Get-AppxPackage`; `CreateEnvironmentBlock` matches the `LAUNCH` script's rebuilt environment; `CreateProcessW` starts the `wt.exe` app execution alias; `wt.exe` shows no console under `CREATE_NO_WINDOW` (`system/win32_real/`, `windows/fast.rs`) | the acceptance command above, then one focus and one launch from Relay |
| `-EncodedCommand` leaves stdin to `[Console]::In` (`powershell.rs`) | `echo {"a":1} \| powershell -NoProfile -EncodedCommand <Base64 of "[Console]::In.ReadToEnd()">` |
| `PrefetchParameters\BootId` exists and increments per boot; `LastBootUpTime` moves with clock corrections (`boot!`) | `Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Memory Management\PrefetchParameters' BootId` before and after a reboot |
| `ConvertTo-Json -Depth 5` shapes: `\/Date(ms)\/`, `{value, DateTime}`, single-element unwrap (`report.rs`) | `Get-CimInstance Win32_Process \| Select -First 1 ProcessId,CreationDate \| ConvertTo-Json` |
| `Win32_Process.CreationDate` and `GetProcessTimes` agree to the second (`report::owner`) | compare `(Get-CimInstance Win32_Process -Filter "ProcessId=$PID").CreationDate` with `(Get-Process -Id $PID).StartTime` |
| A classic console window's owner is its first client, or `conhost.exe` (both accepted) | `GetWindowThreadProcessId` on a `cmd.exe` window (the `OBSERVE` helper's `Windows()`) |
| `conhost.exe`/`OpenConsole.exe --headless` is a pseudo-console host, never a console window (`classify.rs`) | `Get-CimInstance Win32_Process -Filter "Name='OpenConsole.exe'" \| Select CommandLine` inside Windows Terminal |
| Windows Terminal's shells are children of `WindowsTerminal.exe`; a classic console's `conhost.exe` is a child of its first client | `Get-CimInstance Win32_Process \| Select ProcessId,ParentProcessId,Name` |
| Install paths: Claude desktop `AnthropicClaude\`/`AnthropicPBC.Claude_`, Codex desktop `OpenAI.Codex_`, OpenCode desktop `OpenCode.exe` vs CLI `opencode.exe`, npm Claude is `node … cli.js` (`classify.rs`) | `Get-CimInstance Win32_Process -Filter "Name like '%claude%' or Name like '%codex%' or Name like '%opencode%'" \| Select ExecutablePath,CommandLine` |
| `SetForegroundWindow` from a child of the foreground app is allowed; a refusal returns false or does not read back | open a session from Relay with another window in front |
| `wt.exe -w 0 new-tab --profile <guid> --startingDirectory <dir> -- <argv>` joins the last window; Windows Terminal splits on `;` and re-quotes words with spaces (`quote::wt_safe`) | `wt -w 0 new-tab -- cmd /c echo "a b" ^& pause` |
| A Windows Terminal tab does not inherit `wt.exe`'s environment, so provider homes go through `ENV_WRAPPER` | `set X=1 && wt -w 0 new-tab cmd /k echo %X%` with Windows Terminal running |
| `Start-Process` gives a console program a new console window, and the rebuilt environment matches a logon's | `Start-Process cmd` from the `LAUNCH` script; `set` in the new window |
| `cmd.exe` runs `.cmd` shims and reinterprets `% ! ^ & \| < > ( ) "` (`quote::cmd_safe`); npm shims find `node` without the person's PATH (version probe) | `"%APPDATA%\npm\claude.cmd" --version` from a cleared environment |
| `wsl.exe --exec` passes argv unchanged (MSVC rules) and forwards stdin; `--cd` takes a Linux path; `-l -q` is UTF-16LE (`wsl.rs`) | `wsl -d Ubuntu --cd /tmp --exec /usr/bin/printf "[%s]" "a b" 'c"d'`; `wsl -l -q \| Format-Hex` |
| `\\wsl.localhost\<distro>` serves the distribution's files (older builds: `\\wsl$`) | `dir \\wsl.localhost\Ubuntu\home` |
| Default homes and the tool search path (`provider_paths`) | `dir %USERPROFILE%\.claude %USERPROFILE%\.local\bin %APPDATA%\npm` |
| WezTerm and Alacritty install under `%ProgramFiles%` (installed = found or running) | `dir "%ProgramFiles%\WezTerm" "%ProgramFiles%\Alacritty"` |
| Default terminal: `HKCU\Console\%%Startup` `DelegationConsole`/`DelegationTerminal` are `{2EACA947-7F5F-4CFA-BA87-8F7FBEEFBE69}`/`{E12CFF52-A866-4C77-9A90-F570A7AA2C6B}` for Windows Terminal, both `{B23D10C0-E52E-411E-9D5B-C09FDF709C7D}` for the console host, all zeros or missing for "Let Windows decide" (reported `Known(None)`); Preview and third-party classes are `Known(None)` (`report::default_terminal`) | choose each option in Settings > System > For developers > Terminal, then `Get-ItemProperty 'HKCU:\Console\%%Startup'` |
| "Let Windows decide" picks Windows Terminal on Windows 11 22H2+ when installed; not relied on | as above on a 22H2+ machine with no value set, then start `cmd.exe` from Run |

Not covered: `OpenUrl` is `PlatformUnsupported` (no provider app is
modelled on Windows) and `ActivateApp` `HostUnsupported`; `schemes` is
`Known(empty)` for the same reason; `terminal_profile` comes from
`HostHistory::terminal_profile` (the integrator's `WT_PROFILE_ID`), since
another process's environment is not read; WezTerm and Alacritty can be
raised but not launched; tmux inside WSL is `Unknown`; a launch reports
`Dispatched`, never `Opened`. Observing a WSL session starts its
distribution's VM if it is stopped.

## Known limits

- The Linux and Windows adapters were written without either machine; see
  their UNVERIFIED tables above.
- Generic X11 activation covers the named terminals only (GNOME Terminal,
  Konsole, xterm, Alacritty, Kitty, WezTerm, Ghostty); an unrecognised
  terminal is `LiveNotLocatable`.
- Windows: without ttys, a process with no recognised host is never assumed
  headless, so no take-over is offered there.
- A detached-tmux attach reserves nothing: two quick opens may attach two
  clients (harmless: no agent starts).
- OpenCode TUI detection uses the tty, so on Windows another OpenCode TUI
  does not label a resume `PossiblySecondView`.

- Claude terminal focus is `RightWindow`, not exact, even with the capability:
  the person can also `/resume` another conversation in that pane.
- The simulator has one Claude account and only default provider homes; the
  account and environment rules are pinned by unit tests
  (`tests/findings_*.rs`), not by random runs.
- Simulator fidelity is limited to the facts files. Passing here does not
  prove native behaviour.

### Checked on a real Mac (2026-11-11)

`SESSION_NAVIGATION_NATIVE=1 cargo test -p session-navigation-native --test native_acceptance -- --nocapture`
opens two disposable Ghostty windows through the real executor and closes
them again. It passed on Ghostty 1.4.0 and confirmed: a launch reports its
tty; arguments with quotes, `$`, backticks and spaces arrive unchanged; the
working directory and an allowed environment key reach the process; focus is
read back; a stale identity is refused before the terminal is touched; stop
only signals an agent CLI with the exact identity.

`cargo run -p session-navigation-native --example observe -- <provider> <id>`
(read-only) produced correct plans for a live and an archived Claude desktop
session, a stopped Codex terminal thread, an OpenCode session and a missing id.

The default-terminal read (2026-11-12, macOS 27.2): the JXA part of the
apps query, run alone with `osascript -l JavaScript`, returned the handler of
`public.unix-executable` as `com.apple.Terminal`
(`/System/Applications/Utilities/Terminal.app`) without any Automation
prompt; `ObjC.import('UniformTypeIdentifiers')` fails in JXA ("nothing found
to import"), so `UTType` is looked up with `NSClassFromString`. The full
`APPS_JXA` with three bundle ids returned `"terminal":{"bundle":"com.apple.Terminal"}`.

Found this way and fixed: `ps` prints the `nobody` uid as `-2`. Also seen:
Ghostty keeps a window open after its command exits, and the front app reads
as none while the screen is locked.

Not checked natively: Apple Terminal scripts (they would raise an Automation
prompt), `OpenUrl` through the executor (the same link was checked by hand,
native N1 and N5), and a take-over of a real headless run.
