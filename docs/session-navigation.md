# Returning to a local session

A session row's **Open** button returns the person to a coding-agent session:
it focuses the session if it is still open, reopens or resumes it if it was
closed or archived, and otherwise says why it cannot. The decision and the
native actions run **in the app's own process**, never in the core daemon: a
background service cannot raise windows on Windows, and macOS must attribute
terminal-scripting (Automation) permission to the app the person clicked.

A failed focus or link is reported, never turned into a launch. A launch is
reserved durably before it is dispatched, so a second click cannot start a
second agent process while the first may still be starting.

## Ownership

| Where | Responsibility |
| --- | --- |
| `session-navigation/core` (crate `session-navigation`) | Pure rules: evidence, `decide`, plans, the open cycle (observe → decide → reserve → act → observe → verify), and a deterministic simulator with an independent oracle. No side effects. See [its README](../session-navigation/README.md). |
| `session-navigation/native` (crate `session-navigation-native`) | The platform adapters (`MacPorts`, `LinuxPorts`, `WindowsPorts`) over a fakeable `System` seam, plus launch reservations. Each OS builds only its own adapter. |
| `desktop-core/src/session_open` | The app's layer: the JSON contract both apps use, every word the button shows (`wording.rs`), the history feed, legacy pending resumes, Claude desktop id mapping, one open per session at a time, and the `session-open` helper command. |
| `desktop-core/src/session_locations` | `sessions.location` and the background observations it stores (`session-locations.sqlite3`: each session's first and latest host). This history is the feed the engine reads. |
| macOS app | Runs `agent-relay-probe session-open plan|open|release` (the helper it bundles) as its own child and draws the answer. |
| GTK app (Linux, Windows) | Calls `agent_relay_desktop_core::session_open::{plan, open, release_launch}` in process, each off the UI thread. |

## The contract

`plan` answers what the button would do now: `state`, `action` (`focus`,
`show_application`, `open_link`, `resume`, `take_over`, `attach`, `choose`,
`confirm`, `unavailable`), `title`, `enabled`, `reason_code` and `message`,
`precision`, `destination` (`terminal`, `desktop_app`, `none`: the button
shows the terminal icon exactly for `terminal`), `question` (the dialog text
for `confirm` and `take_over`, with `elsewhere` one per line),
`can_resume_default`, `pending_launch`, `choices`, `elsewhere` and `strings`.
`open` runs one click and answers `outcome` (`verified`,
`dispatched_unverified`, `refused`, `failed`), `precision`, `reason_code`,
`message`, and the button after the attempt from a fresh look. `release`
(`release_launch` in process) only clears a pending launch and answers a
fresh `plan`. The request carries `prefer_terminal` (below) besides the
one-time answers (`use_default_terminal`, `confirmed`, `take_over`,
`selected`; `launch_gone` is still accepted on `open`, which then releases
and opens in one call, but the apps do not send it). The full schema, with
examples, is in the module documentation of
`desktop-core/src/session_open.rs`.

- **`plan` is read-only and prompt-free.** It is one look and `decide`; it
  writes nothing. On macOS the look is *passive*: it never runs a terminal
  AppleScript (the front-pane read of Ghostty, Terminal and iTerm2 is
  skipped), so viewing a session row cannot raise an Automation prompt. The
  JXA app registry query it still runs names no Automation target.
- **`open` blocks** for the whole cycle and is run off the UI thread. A
  second open of the same session while one runs, in the same process or in
  another helper process, is refused (`already_opening`).
- **One set of words.** Every core reason and outcome maps to one code and
  one sentence in `wording.rs`, so macOS and GTK show the same text. The
  mapping is an exhaustive `match`: a new reason does not compile without its
  sentence. The words no single answer carries (the button before and while
  an answer is awaited, "Resume in default terminal", "Resolve earlier
  launch…" and its question and answers, Cancel, the take-over question, and
  what to say when no answer could be had: helper missing, request
  rejected, no answer to a look, open outcome unknown, open not started,
  already opening) are `wording::strings()`: in every answer as `strings`,
  printed by `agent-relay-probe session-open strings`, and checked in as
  `shared/session-open-strings.json`. A Rust test fails when the file and the
  table differ, the GTK model tests compare the file with the table it uses,
  and `SessionOpenStringsTests` fails when the macOS fallback copy
  (`SessionOpen.Strings.fallback`, used only when no answer could be had)
  differs from the file. Regenerate the file with
  `cargo run --manifest-path desktop-core/Cargo.toml --bin agent-relay-desktop-probe -- session-open strings > shared/session-open-strings.json`.
- **No answer is never retried.** When the helper is missing, times out,
  exits without an answer or writes one that cannot be read (macOS), or the
  worker thread panics or is dropped (GTK), the app shows the shared
  sentence, enables the button again and looks again. An `open` without an
  answer says its outcome is unknown and is never sent again by itself.
- **Releasing a pending launch.** When `pending_launch` is set with
  `reason_code` `launch_pending`, the apps offer "Resolve earlier launch…" and
  ask "Did you close or stop the earlier launch?". "I closed or stopped it"
  calls `release`, which releases the engine's reservation and any pending
  resume an earlier app version wrote, under the same one-at-a-time guard as
  `open` (a release while an open of the session runs, or the reverse, is
  refused with `already_opening`), and answers a fresh plan. It opens
  nothing; the person then clicks Open.

## Terminal choice

A resume (or a tmux attach) opens the terminal the session was last seen in,
else the one it was first seen in, else the default terminal; with
`use_default_terminal` the default is used directly (offered when
`can_resume_default`). The default is the request's `default_terminal`
(`ghostty`, `apple_terminal`, `iterm2`, `kitty`, `wezterm`, `alacritty`,
`gnome_terminal`, `konsole`, `xterm`, `windows_terminal`, `console_host`),
else the operating system's default terminal as the adapter reads it.
Neither app has a terminal setting yet, so both send none and the system's
default applies. There is no built-in terminal: with none, the launch is
refused (`no_default_terminal`). A terminal is never swapped for another.

### The terminal preference (`prefer_terminal`)

Settings' per-provider "Open with: Automatic / Terminal" for Claude Code and
Codex (macOS `sessionOpening.claude` / `sessionOpening.codex`; GTK Settings
› Opening sessions) is sent as `prefer_terminal` and is the core's
`Preferences::prefer_terminal`. Earlier versions' planner (`stopped.rs`)
used it only for a stopped session: no desktop link or `--desktop` handoff,
a CLI resume instead, in the recorded terminal when the session was last in
one, else the default. The core now does the same where its own safety rules
allow:

- a stopped, unarchived Codex thread started in the Codex app resumes with
  `codex resume` in a terminal instead of opening in Codex (the same resume
  `use_default_terminal` gives such a thread, except that a known last-seen
  terminal is kept rather than replaced by the default);
- an archived Claude Desktop session resumes with `claude --resume` in a
  terminal instead of reviving the record in Claude;
- an unarchived Claude Desktop session still opens in Claude: Claude may
  reopen it by itself, so a terminal resume would be a second writer;
- the terminal is chosen as for any resume, so a known last-seen terminal
  host is never overridden;
- a running session is never affected (a take-over included), a refusal of
  the resume leaves the plan as it would have been (the preference never
  turns a plan into a refusal), and a desktop path the core refuses (a
  hijacked link handler, another account) may be resumed in a terminal
  instead. Grok and OpenCode ignore it (Grok always resumes in a terminal;
  OpenCode has no setting).

There is no other `--desktop` handoff: a Claude session started in a
terminal resumes in a terminal with either setting. The simulator draws the
preference on a third of its opens, and a metamorphic check
(`sim/preference.rs`, invariant `PreferenceScoped`) decides each such open's
snapshot again without it and accepts only these changes; the oracle's
`RightTerminal` also rejects any launch outside a known last-seen terminal
unless the default was asked for.

## How often the button looks

Both apps look (`plan`) when a session's Open button appears, when the
window or app comes to the front, and every 30 seconds while the button is
shown and the window is in front (macOS `SessionOpenModel.pollInterval`,
GTK `models::session_open::PLAN_EVERY`); an open or release answers its own
fresh plan, and no look runs while one of them does. Nothing is polled while
the button is not shown. The macOS app used to look every 5 seconds whenever
a session's detail pane was open, the app in front or not.

Measured on an Apple-silicon Mac (release build of the helper, 20 runs
each, read-only `plan` only): one `session-open plan` takes about 0.07 s of
wall time and 0.15 s of CPU (user and system, the helper's threads
together), for a made-up id and for a real stopped Codex thread alike, with
about 89 MB peak resident memory; the first run after a build took about
0.6 s. Every 5 s that was about 3 % of one core for as long as a session was
shown; every 30 s, in front only, it is about 0.5 %, and nothing in the
background. GTK runs the same look in process, without the process start.

## History and identity

The engine's `last_seen_host` and `original_host` come from the
session-locations store, opened read-only from either process (the helper is
the same binary as the core and finds the store the same way, under
`~/.agentworkforce/probe`). A recorded host maps to a terminal, a desktop app
(Claude, Codex), tmux (its outer terminal was not recorded) or headless (a
Unix process with no tty, pane or window). Anything else, a missing store or
an unreadable record is unknown, never a guess. On Windows every process
lacks a tty, so a process with no recognised host stays unknown there.

The adapters search only the default provider homes. When the store recorded
the session's process with another provider store (`CLAUDE_CONFIG_DIR`,
`CODEX_HOME`, `CODEX_SQLITE_HOME`, `GROK_HOME`), what the adapter found is not
that store's, so the provider environment becomes unknown and a launch is
refused (`environment_unknown`) rather than opened in the wrong store.

Claude's desktop app shows `local_…` ids; the engine works on the CLI
conversation id the desktop record names. Only that record may join them,
never a title or a directory; none or several records refuse it
(`desktop_id_unmapped`).

## Reservations

Launch reservations live in the app's data directory
(`<app data>/session-navigation/reservations`, one file per session, written
atomically). An unreadable store blocks launches. A reservation resolves when
the window it opened is gone, when the session is found in that window, when
it was made in an earlier boot, or when the person confirms the earlier launch
was closed (`release`).

Pending resumes that earlier app versions wrote
(`session-pending-resumes.json` on macOS, `pending-session-resumes.json` in
GTK, both in the app's data directory) still block a duplicate launch after an
upgrade: an entry reads as a reservation with no tty, so only an owner the
plan returns to (Windows), a live owner of the session, or `release`
clears it. A damaged file blocks every launch, as before. Entries are removed
only when resolved, never silently.

## What has been verified natively

Most confidence comes from tests: the core's simulator and decision tables,
and every adapter replayed against `FakeSystem` with documented or recorded
output shapes. Run `bash scripts/test-session-navigation.sh`.

Checked on a real Mac (Ghostty 1.4.0, 2026-11-11): a Ghostty launch reports
its tty, arguments with quotes, `$`, backticks and spaces arrive unchanged,
the working directory and an allowed environment key reach the process, focus
is read back, a stale identity is refused before the terminal is touched, and
stop only signals an agent CLI with the exact identity. Read-only
observations produced correct plans for live and archived Claude desktop
sessions, a stopped Codex terminal thread, an OpenCode session and a missing
id.

**Not run against real applications:**

- the Linux adapter (X11 and Wayland) and the Windows adapter (native and
  WSL) at all: they were written and tested only against `FakeSystem`;
- on macOS, Terminal and iTerm2 scripting, Kitty and WezTerm remote control,
  Alacritty launches, and tmux;
- the app wiring in `desktop-core/src/session_open` against a real session.

Each adapter's assumptions and the command that would verify each are listed
in [session-navigation/README.md](../session-navigation/README.md). Kitty's
`allow_remote_control password` mode may ask the person to confirm a request
without a password; a plan look still lists Kitty windows, so this is the one
known way a look could show a dialog (UNVERIFIED).

WSL sessions are not routed to the WSL adapter yet: the history does not
record a session's distribution, so the Windows app observes the native
runtime only.

## Native acceptance procedure

Use disposable sessions and windows, with the installed app and terminal
versions recorded in the test result. Never use another person's live working
session as a fixture.

1. Start two sessions in the same directory and give them the same title.
   Record both provider ids and terminal pane identities.
2. From Relay, open the second session. Read the selected pane or
   conversation back through the host's API or UI. An app coming forward is
   not evidence the conversation was selected.
3. Reorder tabs and repeat. Leave a permission prompt open, close the target,
   then grant permission: no replacement process may be launched.
4. End the disposable session, close its window, restart Relay, and reopen
   it. Verify the resumed provider id, working directory, terminal and
   profile.
5. Remove or deny the original host. Verify the reason and the explicit
   default-terminal alternative; make the session live again before choosing
   the alternative and verify that the fresh plan prevents a duplicate.
6. Repeat on Windows and Linux (X11 and Wayland), recording unavailable
   capabilities explicitly. Test native Windows and WSL independently.

Reference contracts: [Claude desktop handoff](https://code.claude.com/docs/en/desktop#coming-from-the-cli),
[Windows Terminal targeting](https://learn.microsoft.com/en-us/windows/terminal/command-line-arguments),
[iTerm scripting](https://iterm2.com/documentation-scripting.html), and the
installed Ghostty `Ghostty.sdef`. Provider deep links are version-dependent.
