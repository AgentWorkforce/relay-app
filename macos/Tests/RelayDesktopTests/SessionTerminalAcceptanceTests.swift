import AppKit
import Carbon
import Darwin
import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

/// Opt in on a desktop with Terminal Automation already granted:
/// RELAY_RUN_TERMINAL_NAVIGATION_FIXTURE=1 swift test --package-path macos --filter SessionTerminalAcceptanceTests
/// This creates and closes only UUID-marked fixture windows, never user sessions.
@MainActor final class SessionTerminalAcceptanceTests: XCTestCase {
    func testDisposableTerminalPanesAndClosedTargetResume() async throws {
        guard ProcessInfo.processInfo.environment["RELAY_RUN_TERMINAL_NAVIGATION_FIXTURE"] == "1" else {
            throw XCTSkip("Opt-in disposable Terminal acceptance fixture")
        }
        try requireExistingAutomationPermission()
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("relay-terminal-fixture-\(UUID())").resolvingSymlinksInPath()
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let marker = "Relay fixture \(UUID())"
        var owned: [(URL, SessionLocation)] = []
        defer {
            for (helper, location) in owned {
                if SessionLocationLauncher.processIsCurrent(location) { _ = Darwin.kill(Int32(location.pid), SIGTERM) }
                for _ in 0..<50 where SessionLocationLauncher.processIsCurrent(location) { usleep(50_000) }
                closeOwnedWindow(tty: location.tty!, marker: marker)
                try? FileManager.default.removeItem(at: helper)
            }
            try? FileManager.default.removeItem(at: directory)
        }
        let firstHelper = try writeHelper(directory, name: "first")
        let secondHelper = try writeHelper(directory, name: "second")
        let first = try await launchHelper(firstHelper, directory: directory)
        owned.append((firstHelper, first))
        try setTitle(first, marker: marker)
        let second = try await launchHelper(secondHelper, directory: directory)
        owned.append((secondHelper, second))
        try setTitle(second, marker: marker)
        XCTAssertNotEqual(first.tty, second.tty)
        for location in [second, first] {
            let failure = await SessionLocationLauncher.focus(location)
            XCTAssertNil(failure)
            let selected = try apple("tell application id \"com.apple.Terminal\" to return tty of selected tab of front window")
            XCTAssertEqual(selected, location.tty, "Identical title and cwd must not choose another fixture pane")
        }
        _ = Darwin.kill(Int32(first.pid), SIGTERM)
        for _ in 0..<100 where SessionLocationLauncher.processIsCurrent(first) {
            try await Task.sleep(for: .milliseconds(20))
        }
        closeOwnedWindow(tty: first.tty!, marker: marker)
        let closedFailure = await SessionLocationLauncher.focus(first)
        XCTAssertNotNil(closedFailure, "A closed target cannot report focus or start a replacement")

        try FileManager.default.removeItem(at: firstHelper.appendingPathExtension("ready"))
        let command = SessionResumeCommand(args: ["resume", "fixture-id'$(printf NOT_EXPANDED)"], cwd: directory.path, executable: firstHelper.path)
        let plan = SessionOpenPlan(action: .resume, canResumeDefault: false, choices: [], command: command, location: first)
        let executor = SessionPlanExecutor(store: SessionPendingResumeStore(file: directory.appendingPathComponent("pending-resumes.json")))
        let outcome = await executor.execute(key: "fixture:\(marker)", plan: plan) { plan }
        guard case .dispatched = outcome else { return XCTFail("Original Terminal resume failed: \(outcome)") }
        let resumed = try await readLocation(firstHelper)
        owned.append((firstHelper, resumed))
        try setTitle(resumed, marker: marker)
        let result = try String(contentsOf: firstHelper.appendingPathExtension("result"), encoding: .utf8)
        XCTAssertEqual(result, ([directory.path] + command.args).joined(separator: "\n") + "\n")
    }

    private func requireExistingAutomationPermission() throws {
        var target = AEAddressDesc()
        let bundle = "com.apple.Terminal"
        let made = bundle.utf8CString.withUnsafeBufferPointer {
            AECreateDesc(DescType(typeApplicationBundleID), $0.baseAddress, $0.count - 1, &target)
        }
        guard made == noErr else { throw XCTSkip("Could not prepare Automation preflight") }
        defer { AEDisposeDesc(&target) }
        let permission = AEDeterminePermissionToAutomateTarget(&target, AEEventClass(typeWildCard), AEEventID(typeWildCard), false)
        guard permission == noErr else {
            throw XCTSkip("Terminal Automation is not already granted (\(permission)); no permission prompt was requested and no windows were created")
        }
    }

    private func writeHelper(_ directory: URL, name: String) throws -> URL {
        let helper = directory.appendingPathComponent(name)
        let script = """
        #!/bin/sh
        printf '%s\\n' "$PWD" "$@" > "$0.result"
        printf '%s' "$$" > "$0.pid"
        /usr/bin/tty > "$0.tty"
        : > "$0.ready"
        trap 'exit 0' TERM INT
        remaining=30
        while [ "$remaining" -gt 0 ]; do /bin/sleep 1; remaining=$((remaining - 1)); done
        """
        try Data(script.utf8).write(to: helper)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: helper.path)
        return helper
    }

    private func launchHelper(_ helper: URL, directory: URL) async throws -> SessionLocation {
        let command = SessionResumeCommand(args: ["initial"], cwd: directory.path, executable: helper.path)
        let error = await SessionTerminalAdapter.terminal.resume(command)
        XCTAssertNil(error)
        return try await readLocation(helper)
    }

    private func readLocation(_ helper: URL) async throws -> SessionLocation {
        for _ in 0..<100 where !FileManager.default.fileExists(atPath: helper.appendingPathExtension("ready").path) {
            try await Task.sleep(for: .milliseconds(50))
        }
        let pid = try XCTUnwrap(Int(String(contentsOf: helper.appendingPathExtension("pid"), encoding: .utf8)))
        let tty = try String(contentsOf: helper.appendingPathExtension("tty"), encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)
        var info = proc_bsdinfo()
        let size = Int32(MemoryLayout<proc_bsdinfo>.size)
        XCTAssertEqual(proc_pidinfo(Int32(pid), PROC_PIDTBSDINFO, 0, &info, size), size)
        return SessionLocation(application: "Terminal", bundleId: "com.apple.Terminal", observedAtMs: 1,
            pid: pid, processStart: Int(info.pbi_start_tvsec) * 1_000_000 + Int(info.pbi_start_tvusec), tty: tty)
    }

    private func setTitle(_ location: SessionLocation, marker: String) throws {
        _ = try apple("""
        tell application id "com.apple.Terminal"
            repeat with w in windows
                repeat with t in tabs of w
                    if tty of t is \(SessionTerminalAdapter.appleString(location.tty!)) then set custom title of t to \(SessionTerminalAdapter.appleString(marker))
                end repeat
            end repeat
        end tell
        """)
    }

    private func closeOwnedWindow(tty: String, marker: String) {
        // Both the UUID marker and captured TTY must match, and the window
        // must still contain only the tab this test created.
        _ = try? apple("""
        tell application id "com.apple.Terminal"
            repeat with w in windows
                if (count of tabs of w) is 1 then
                    set t to tab 1 of w
                    if tty of t is \(SessionTerminalAdapter.appleString(tty)) and custom title of t is \(SessionTerminalAdapter.appleString(marker)) then close w saving no
                end if
            end repeat
        end tell
        """)
    }

    private func apple(_ script: String) throws -> String? {
        var error: NSDictionary?
        let result = NSAppleScript(source: script)?.executeAndReturnError(&error)
        if let error { throw NSError(domain: "TerminalFixture", code: 1, userInfo: [NSLocalizedDescriptionKey: error.description]) }
        return result?.stringValue
    }
}
