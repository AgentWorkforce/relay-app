import AppKit
import Darwin
import RelayDesktopCore

/// Native navigation never types into a terminal or starts another agent.
enum SessionLocationLauncher {
    static func processIsCurrent(_ location: SessionLocation) -> Bool {
        guard location.pid > 0, location.pid <= Int(Int32.max), location.processStart >= 0 else { return false }
        var info = proc_bsdinfo()
        let size = Int32(MemoryLayout<proc_bsdinfo>.size)
        let read = proc_pidinfo(Int32(location.pid), PROC_PIDTBSDINFO, 0, &info, size)
        guard read == size && info.pbi_uid == getuid() && info.pbi_status != UInt32(SZOMB)
            && UInt64(info.pbi_start_tvsec) * 1_000_000 + UInt64(info.pbi_start_tvusec) == UInt64(location.processStart) else { return false }
        if let tty = location.tty {
            var device = stat()
            guard tty.hasPrefix("/dev/"), stat(tty, &device) == 0,
                  device.st_mode & S_IFMT == S_IFCHR, info.e_tdev == UInt32(bitPattern: device.st_rdev) else { return false }
        }
        if let pid = location.hostPid, let start = location.hostProcessStart {
            guard pid > 0, pid <= Int(Int32.max), start >= 0 else { return false }
            var host = proc_bsdinfo()
            guard proc_pidinfo(Int32(pid), PROC_PIDTBSDINFO, 0, &host, size) == size,
                  host.pbi_uid == getuid(), host.pbi_status != UInt32(SZOMB), UInt64(host.pbi_start_tvsec) * 1_000_000
                    + UInt64(host.pbi_start_tvusec) == UInt64(start) else { return false }
        }
        return true
    }

    /// The executor resolves any Automation prompt, then re-reads the shared
    /// plan and checks the kernel identity before issuing the focus command.
    @MainActor static func focus(_ location: SessionLocation) async -> String? {
        guard processIsCurrent(location) else { return "This session’s process has ended or changed. Refresh the session and try again." }
        guard let script = script(for: location) else {
            return "Relay found this session in \(location.application), but could not identify an exact terminal pane."
        }
        return await execute(script, expected: "focused")
    }

    static func authorize(_ bundle: String) async -> String? {
        guard SessionTerminalAdapter(rawValue: bundle)?.usesAppleEvents == true else { return nil }
        let script = "with timeout of 15 seconds\ntell application id \"\(bundle)\" to count windows\nend timeout"
        return await execute(script, expected: nil)
    }

    static func execute(_ script: String, expected: String?) async -> String? {
        await Task.detached {
            var error: NSDictionary?
            let result = NSAppleScript(source: script)?.executeAndReturnError(&error)
            if (error?[NSAppleScript.errorNumber] as? Int) == -1743 {
                return "Allow Agent Relay to control the original terminal in System Settings → Privacy & Security → Automation, then try again."
            }
            if error == nil, let result, expected == nil || result.stringValue == expected { return nil as String? }
            return "Couldn’t select or open the original terminal surface. It may have closed, or this app version may not support this operation. No alternative session was started."
        }.value
    }

    /// Bundle IDs are allowlisted; only a kernel TTY is interpolated, as an
    /// escaped AppleScript string, never as code or a shell command.
    static func script(for location: SessionLocation) -> String? {
        guard let tty = location.tty, tty.hasPrefix("/dev/"), !tty.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains) else { return nil }
        let value = "\"" + tty.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"") + "\""
        let body: String
        switch location.bundleId {
        case "com.mitchellh.ghostty":
            body = """
            set candidates to every terminal whose tty is \(value)
            if (count of candidates) is not 1 then error "Original pane unavailable"
            focus (item 1 of candidates)
            if tty of focused terminal of selected tab of front window is not \(value) then error "Pane selection was not confirmed"
            """
        case "com.apple.Terminal":
            body = """
            set matches to {}
            repeat with w in windows
                repeat with t in tabs of w
                    if tty of t is \(value) then set end of matches to {w, t}
                end repeat
            end repeat
            if (count of matches) is not 1 then error "Original tab unavailable"
            set match to item 1 of matches
            set w to item 1 of match
            set selected tab of w to item 2 of match
            set miniaturized of w to false
            set index of w to 1
            activate
            if tty of selected tab of front window is not \(value) then error "Tab selection was not confirmed"
            """
        case "com.googlecode.iterm2":
            body = """
            set matches to {}
            repeat with w in windows
                repeat with t in tabs of w
                    repeat with s in sessions of t
                        if tty of s is \(value) then set end of matches to {w, t, s}
                    end repeat
                end repeat
            end repeat
            if (count of matches) is not 1 then error "Original pane unavailable"
            set match to item 1 of matches
            select (item 3 of match)
            select (item 2 of match)
            select (item 1 of match)
            activate
            if tty of current session of current tab of current window is not \(value) then error "Pane selection was not confirmed"
            """
        default: return nil
        }
        return "with timeout of 15 seconds\ntell application id \"\(location.bundleId)\"\n\(body)\nreturn \"focused\"\nend tell\nend timeout"
    }
}
