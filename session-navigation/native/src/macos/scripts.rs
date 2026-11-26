//! Every script the adapters run. Each is a constant: data reaches a script
//! only as `on run argv` arguments, never by building script source.
//!
//! Custom error numbers (7001…) let the executor map a script's own refusals
//! to a `Reason` without matching on text.

/// JXA: scheme handlers, installed apps (by bundle id, passed as argv),
/// the frontmost app, which of the asked-about apps run, and the default
/// handler of `public.unix-executable` (macOS has no terminal setting; this
/// is what "Make iTerm2 Default Term" sets, and Terminal when nothing did;
/// VERIFIED read-only on macOS 27.2, which returned `com.apple.Terminal`).
/// `UTType` is looked up by name: JXA cannot import its framework. A failed
/// read leaves `terminal` null (unknown). Needs no Automation permission (it
/// only talks to NSWorkspace).
pub const APPS_JXA: &str = r#"ObjC.import('AppKit');
function run(argv) {
  const ws = $.NSWorkspace.sharedWorkspace;
  const out = {schemes: {}, apps: {}, front: null, running: []};
  for (const s of ['claude','codex','opencode']) {
    const u = ws.URLForApplicationToOpenURL($.NSURL.URLWithString(s + '://x'));
    out.schemes[s] = u.isNil() ? null : ObjC.unwrap($.NSBundle.bundleWithURL(u).bundleIdentifier);
  }
  for (const b of argv) {
    const u = ws.URLForApplicationWithBundleIdentifier(b);
    if (u.isNil()) { out.apps[b] = null; continue; }
    const bundle = $.NSBundle.bundleWithURL(u);
    const exe = bundle.executablePath;
    out.apps[b] = {path: ObjC.unwrap(u.path), version: ObjC.unwrap(bundle.objectForInfoDictionaryKey('CFBundleShortVersionString')), executable: exe.isNil() ? null : ObjC.unwrap(exe)};
  }
  const f = ws.frontmostApplication;
  out.front = f.isNil() ? null : ObjC.unwrap(f.bundleIdentifier);
  const ra = ws.runningApplications;
  for (let i = 0; i < ra.count; i++) { const b = ra.objectAtIndex(i).bundleIdentifier; if (!b.isNil()) out.running.push(ObjC.unwrap(b)); }
  out.running = out.running.filter(b => argv.includes(b));
  out.terminal = null;
  try {
    const T = $.NSClassFromString('UTType');
    const t = T.isNil() ? T : T.typeWithIdentifier('public.unix-executable');
    if (!t.isNil()) {
      const u = ws.URLForApplicationToOpenContentType(t);
      const id = u.isNil() ? null : ObjC.unwrap($.NSBundle.bundleWithURL(u).bundleIdentifier);
      out.terminal = {bundle: typeof id === 'string' ? id : null};
    }
  } catch (e) { out.terminal = null; }
  return JSON.stringify(out);
}"#;

/// The focused Ghostty terminal's tty (read only).
pub const GHOSTTY_FRONT_TTY: &str = r#"on run argv
  with timeout of 5 seconds
    tell application id "com.mitchellh.ghostty"
      return tty of focused terminal of selected tab of front window
    end tell
  end timeout
end run"#;

/// The selected Terminal tab's tty (read only).
pub const TERMINAL_FRONT_TTY: &str = r#"on run argv
  with timeout of 5 seconds
    tell application id "com.apple.Terminal"
      return tty of selected tab of front window
    end tell
  end timeout
end run"#;

/// argv: tty. Exactly one terminal must have it; it is focused and checked.
pub const GHOSTTY_FOCUS: &str = r#"on run argv
  set wanted to item 1 of argv
  with timeout of 15 seconds
    tell application id "com.mitchellh.ghostty"
      set candidates to every terminal whose tty is wanted
      if (count of candidates) is not 1 then error "pane unavailable" number 7001
      focus (item 1 of candidates)
      activate
      if tty of focused terminal of selected tab of front window is not wanted then error "pane not confirmed" number 7002
    end tell
  end timeout
  return "focused"
end run"#;

/// argv: tty. Exactly one tab must have it; it is selected and brought front.
pub const TERMINAL_FOCUS: &str = r#"on run argv
  set wanted to item 1 of argv
  with timeout of 15 seconds
    tell application id "com.apple.Terminal"
      set matches to {}
      repeat with w in windows
        repeat with t in tabs of w
          if tty of t is wanted then set end of matches to {w, t}
        end repeat
      end repeat
      if (count of matches) is not 1 then error "tab unavailable" number 7001
      set match to item 1 of matches
      set w to item 1 of match
      set selected tab of w to item 2 of match
      set miniaturized of w to false
      set index of w to 1
      activate
      if tty of selected tab of front window is not wanted then error "tab not confirmed" number 7002
    end tell
  end timeout
  return "focused"
end run"#;

/// argv: cwd, command, then zero or more `KEY=VALUE`. Opens a new window and
/// polls up to ~3 s for its tty. Prints `tty:<tty>` or `dispatched`.
pub const GHOSTTY_LAUNCH: &str = r#"on run argv
  set theCwd to item 1 of argv
  set theCommand to item 2 of argv
  set theEnv to {}
  repeat with i from 3 to (count of argv)
    set end of theEnv to item i of argv
  end repeat
  set theTty to ""
  with timeout of 15 seconds
    tell application id "com.mitchellh.ghostty"
      set cfg to new surface configuration
      set initial working directory of cfg to theCwd
      set command of cfg to theCommand
      if (count of theEnv) > 0 then set environment variables of cfg to theEnv
      set w to new window with configuration cfg
      activate
      repeat 30 times
        try
          set theTty to tty of terminal 1 of w
        end try
        if theTty is not missing value and theTty is not "" then exit repeat
        delay 0.1
      end repeat
    end tell
  end timeout
  if theTty is missing value or theTty is "" then return "dispatched"
  return "tty:" & theTty
end run"#;

/// argv: the shell command. `do script` without `in` opens a new window.
pub const TERMINAL_LAUNCH: &str = r#"on run argv
  set theCommand to item 1 of argv
  set theTty to ""
  with timeout of 15 seconds
    tell application id "com.apple.Terminal"
      set t to do script theCommand
      activate
      repeat 30 times
        try
          set theTty to tty of t
        end try
        if theTty is not missing value and theTty is not "" then exit repeat
        delay 0.1
      end repeat
    end tell
  end timeout
  if theTty is missing value or theTty is "" then return "dispatched"
  return "tty:" & theTty
end run"#;

/// The current iTerm2 session's tty (read only). UNVERIFIED: iTerm2 was not
/// installed; `current session of current window` and `tty` are from
/// iTerm2's AppleScript dictionary (iterm2.com/documentation-scripting).
pub const ITERM_FRONT_TTY: &str = r#"on run argv
  with timeout of 5 seconds
    tell application id "com.googlecode.iterm2"
      return tty of current session of current window
    end tell
  end timeout
end run"#;

/// argv: tty. Exactly one iTerm2 session must have it; its window, tab and
/// session are selected, the app activated, and the current session's tty
/// read back. UNVERIFIED (iTerm2 dictionary: `select` on window, tab and
/// session).
pub const ITERM_FOCUS: &str = r#"on run argv
  set wanted to item 1 of argv
  with timeout of 15 seconds
    tell application id "com.googlecode.iterm2"
      set matches to {}
      repeat with w in windows
        repeat with t in tabs of w
          repeat with s in sessions of t
            if tty of s is wanted then set end of matches to {w, t, s}
          end repeat
        end repeat
      end repeat
      if (count of matches) is not 1 then error "session unavailable" number 7001
      set match to item 1 of matches
      select (item 1 of match)
      select (item 2 of match)
      select (item 3 of match)
      activate
      if tty of current session of current window is not wanted then error "session not confirmed" number 7002
    end tell
  end timeout
  return "focused"
end run"#;

/// argv: the command line for `create window with default profile
/// command`. Polls up to ~3 s for the new session's tty. Prints
/// `tty:<tty>` or `dispatched`. UNVERIFIED: iTerm2 splits `command` into
/// argv itself, honouring single quotes (the command is
/// `'/bin/sh' '-c' '<constant>' 'sh' '<cwd>' '<program>' '<arg>'…`).
pub const ITERM_LAUNCH: &str = r#"on run argv
  set theCommand to item 1 of argv
  set theTty to ""
  with timeout of 15 seconds
    tell application id "com.googlecode.iterm2"
      set w to (create window with default profile command theCommand)
      activate
      repeat 30 times
        try
          set theTty to tty of current session of w
        end try
        if theTty is not missing value and theTty is not "" then exit repeat
        delay 0.1
      end repeat
    end tell
  end timeout
  if theTty is missing value or theTty is "" then return "dispatched"
  return "tty:" & theTty
end run"#;

/// The constant `sh -c` program of an iTerm2 launch: `$1` is the cwd, the
/// rest the argv. Data never becomes shell text.
pub const ITERM_SH: &str = r#"cd -- "$1" || exit 1; shift; exec "$@""#;

/// AppleScript error: the person has not allowed Automation for this app.
pub const NOT_AUTHORIZED: i32 = -1743;
/// The scripts' own "target pane not found exactly once".
pub const PANE_UNAVAILABLE: i32 = 7001;
/// The scripts' own "focused, but the read-back named another pane".
pub const PANE_UNCONFIRMED: i32 = 7002;
