# Returning to a local session

The desktop asks `sessions.open_plan` for a fresh decision when a person clicks
Open. The core returns one of `focus`, `show_application`, `open_link`, `resume`,
`choose`, or `unavailable`. Swift and GTK execute that decision through native
adapters. A failed focus or link never automatically becomes a second agent
process.

## Ownership

| Module | Responsibility |
| --- | --- |
| `desktop-core/src/session_locations/store.rs` | Durable original and latest observations, legacy migration, bounded history |
| `desktop-core/src/session_locations/providers.rs` | Provider session ownership, Claude desktop identity mapping, liveness evidence |
| `desktop-core/src/session_locations/hosts.rs` | Process ancestry, host identity, terminal endpoints, supported capabilities |
| `desktop-core/src/session_locations/planner.rs` | Shared policy, selected-target revalidation, structured resume commands |
| `macos/Sources/RelayDesktop/SessionPlanExecutor.swift` | In-flight exclusion, permission handling, final process checks, execution outcomes |
| `macos/Sources/RelayDesktop/SessionTerminalAdapter.swift` | New-window launch through each supported terminal's API |
| `gtk/src/session_navigation.rs` | Injectable Linux/Windows native execution and structured launch arguments |
| `gtk/src/session_navigation/windows.rs` | Verified HWND selection and Windows URL dispatch |

Keep provider policy in the planner. A new terminal needs host recognition,
capability evidence, an execution adapter, and tests using that terminal's real
response shape. Adding an executable name alone does not establish exact pane
navigation support.

## Identity and persistence

Observations retain the provider and session ID, PID/start time, boot identity,
host process incarnation, working directory, executable, and available
window/pane/profile/runtime information. Windows uses its absolute process
creation time; an estimated boot time is not treated as reboot evidence.
Capture reads only the identity
variables a host adapter needs; it does not save an environment dump.

Observed provider storage overrides (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`,
`CODEX_SQLITE_HOME`, `GROK_HOME`) travel with the structured resume command.
A known empty override map clears inherited overrides in the new terminal;
an unreadable original environment cannot silently become the default account.
Legacy origins without capture metadata retain the current configuration
behavior, rather than claiming a historical environment was verified.

The first observation remains provenance. The latest verified host determines
where a stopped session resumes, including a session moved between a terminal
and a desktop application. Delayed observations cannot replace newer origin or
process-history records; equal timestamps retain the already committed record. Old handles are history, never authority to focus a
live window. An unreadable process is different from an exited process.

Observation runs at discovery/registration-related invalidations with a timer
as a backstop. This is not a universal process-start hook: sessions that start
and exit before discovery can lack an origin. Their resume path requires
current liveness evidence and a known working directory.

Claude's desktop `local_…` identity differs from its CLI session ID. Only
provider-owned metadata may join them; matching a title or project directory
does not. Invalid or ambiguous mappings do not become guessed links. A mapped
desktop link and a CLI-to-desktop handoff are separate operations.

## Native capabilities

Capabilities depend on the installed application and available APIs, not just
the operating system name.

- macOS: Ghostty, Terminal and iTerm use exact TTY scripting and read back the
  selected target. Original-host resume adapters also cover Kitty, WezTerm and
  Alacritty. Automation denial is reported without starting another session.
- Linux: Kitty, WezTerm and tmux use recorded endpoints and pane evidence.
  X11 activation verifies the window's process. Generic Wayland window raising
  is not assumed; absent host/compositor support produces an explicit reason.
- Windows: native window activation verifies the host process and HWND.
  Multiple unidentifiable windows require a choice outside Relay rather than
  selecting an arbitrary one. A Windows Terminal session GUID is not treated
  as a pane-selection API. Resumes preserve the captured terminal profile.
- WSL, remote runtimes and detached multiplexers must retain their execution
  environment. An adapter that cannot restore it reports unavailable instead
  of running the command in a different environment.

If the original terminal cannot resume a stopped session, the person can
explicitly choose the default terminal. The core rechecks ownership for that
choice; a failed live focus does not offer this fallback.

URL dispatch, process spawn, and a terminal accepting a command are not proof
that the provider selected the requested conversation. Adapters report
dispatch separately from verified window/pane selection. Do not turn a
dispatch timeout into an automatic second attempt to resume.

Before a resume dispatch, both native clients atomically persist a per-session
reservation in their local configuration directory. It survives desktop restarts
while the shell starts. Storage failures block resume before native dispatch. Another Open cannot launch a replacement until
fresh ownership is observed, or the person confirms that they closed the earlier
launch. Adapter errors also retain this guard when dispatch may have happened;
a timeout or a cooldown does not establish that the earlier launch stopped.

## Regression tests

Run `bash scripts/test-session-navigation.sh`. The shared JSON scenarios in
`shared/session-navigation-cases.json` exercise real planner decisions; native
executor tests inject process evidence and platform operations. They cover:

- same title and directory in different panes;
- reused PIDs, changed boot identities, moved sessions and refreshed timestamps;
- ambiguous Claude identities and exact Codex URL escaping;
- stopped versus unknown owners and background processes;
- original versus latest host, explicit fallback, and preserved argument boundaries;
- permission denial, stale targets after permission prompts, and concurrent clicks;
- window targeting and missing/denied terminal remote-control APIs.

Run GTK UI tests on each supported OS as described in `workspace-layout.md`.
Cross-compilation and fake executors do not establish real native focus.

On macOS, `RELAY_RUN_TERMINAL_NAVIGATION_FIXTURE=1 swift test --package-path macos --filter SessionTerminalAcceptanceTests`
runs an opt-in disposable Terminal fixture. It requires Automation permission
already granted, never prompts to change it, and creates/cleans up only its own
marked windows. A skipped preflight is not evidence of native selection.

## Native acceptance procedure

Use disposable sessions and windows, with an installed app/terminal version
recorded in the test result. Never use another person's live working session
as a test fixture.

1. Start two sessions in the same directory and give them the same title.
   Record both provider IDs and terminal pane identities.
2. From Relay, open the second session. Read the selected pane/conversation
   identity through the host's API or UI. An app becoming foreground is
   insufficient evidence of conversation selection.
3. Reorder tabs and repeat. Leave a permission prompt open, close the target,
   then grant permission: no replacement process may be launched.
4. End the disposable session, close its window, restart Relay, and reopen it.
   Verify the resumed provider ID, working directory, terminal and profile.
5. Remove or deny the original host. Verify the reason and explicit alternative;
   make the session live again before choosing the alternative and verify that
   the fresh plan prevents a duplicate.
6. Repeat on Windows and Linux (X11 and Wayland), recording unavailable
   capabilities explicitly. Test native Windows and WSL independently.

Reference contracts: [Claude desktop handoff](https://code.claude.com/docs/en/desktop#coming-from-the-cli),
[Windows Terminal targeting](https://learn.microsoft.com/en-us/windows/terminal/command-line-arguments),
[iTerm scripting](https://iterm2.com/documentation-scripting.html), and the
installed Ghostty `Ghostty.sdef`. Provider deep links are version-dependent.
