import XCTest
import AppKit
import Darwin
@testable import RelayDesktopCore
@testable import RelayDesktop

final class SessionLocationNavigationTests: XCTestCase {
    private func terminal(pid: Int = 30, start: Int = 31, tty: String = "/dev/ttys004") -> SessionLocation {
        SessionLocation(application: "Ghostty", bundleId: "com.mitchellh.ghostty", observedAtMs: 1000,
                        pid: pid, processStart: start, tty: tty)
    }

    func testTerminalScriptsRequireUniqueTTYAndVerifySelectedPane() throws {
        let script = try XCTUnwrap(SessionLocationLauncher.script(for: terminal()))
        XCTAssertTrue(script.contains("every terminal whose tty is \"/dev/ttys004\""))
        XCTAssertTrue(script.contains("(count of candidates) is not 1"))
        XCTAssertTrue(script.contains("tty of focused terminal of selected tab of front window"))
        for forbidden in ["input text", "send key", "do script", "working directory", "name contains"] { XCTAssertFalse(script.contains(forbidden)) }
        let other = SessionLocation(application: "Unknown", bundleId: "unknown", observedAtMs: 1, pid: 1, processStart: 1, tty: "/dev/ttys004")
        XCTAssertNil(SessionLocationLauncher.script(for: other))
        XCTAssertFalse(SessionLocationLauncher.processIsCurrent(terminal(pid: 2_000_000_000)))
    }

    func testSameTitleAndDirectoryPanesAreSelectedByExactTTY() throws {
        let first = try XCTUnwrap(SessionLocationLauncher.script(for: terminal()))
        let second = try XCTUnwrap(SessionLocationLauncher.script(for: terminal(tty: "/dev/ttys005")))
        XCTAssertNotEqual(first, second)
        XCTAssertFalse(second.contains("/dev/ttys004"))
        for bundle in ["com.apple.Terminal", "com.googlecode.iterm2"] {
            let location = SessionLocation(application: "Terminal", bundleId: bundle, observedAtMs: 1, pid: 30, processStart: 31, tty: "/dev/ttys005")
            let script = try XCTUnwrap(SessionLocationLauncher.script(for: location))
            XCTAssertTrue(script.contains("(count of matches) is not 1"))
            XCTAssertTrue(script.contains("selection was not confirmed"))
        }
    }

    func testOriginalHostResumeCreatesNewSurfaceWithExactArgvAndDirectory() throws {
        let command = SessionResumeCommand(args: ["resume", "id'$(touch BAD)"], cwd: "/repo's `name`", executable: "codex")
        for adapter in SessionTerminalAdapter.allCases {
            if adapter.usesAppleEvents {
                let script = try XCTUnwrap(adapter.resumeScript(command))
                XCTAssertTrue(script.contains(adapter.rawValue))
                XCTAssertFalse(script.contains("input text"))
                XCTAssertFalse(script.contains("write text"))
                XCTAssertFalse(script.contains("selected tab"))
                XCTAssertTrue(script.contains("codex"))
                if adapter == .terminal { XCTAssertFalse(script.contains("do script ") && script.contains(" in ")) }
                if adapter == .iterm {
                    XCTAssertTrue(script.contains("command \"/bin/zsh -lic"))
                    XCTAssertFalse(script.contains("command \"exec "), "iTerm launches argv directly; exec is a shell builtin")
                }
            } else {
                let args = try XCTUnwrap(adapter.arguments(command))
                XCTAssertTrue(args.contains(command.cwd))
                XCTAssertEqual(Array(args.suffix(3)), ["/bin/zsh", "-lic", SessionTerminalAdapter.shellCommand(command)])
            }
        }
        XCTAssertNil(SessionTerminalAdapter(rawValue: "dev.warp.Warp-Stable"))
        XCTAssertEqual(SessionTerminalAdapter.shellCommand(command), "cd -- '/repo'\"'\"'s `name`' && exec 'codex' 'resume' 'id'\"'\"'$(touch BAD)'")
        let shell = SessionTerminalAdapter.shellCommand(command)
        var error: NSDictionary?
        let roundtrip = NSAppleScript(source: "return " + SessionTerminalAdapter.appleString(shell))?.executeAndReturnError(&error)
        XCTAssertNil(error)
        XCTAssertEqual(roundtrip?.stringValue, shell, "AppleScript serialization must preserve the complete command without evaluating metadata")
    }

    func testNativeProcessIdentityRejectsAReusedPIDAndDetachedTTY() {
        var info = proc_bsdinfo()
        let size = Int32(MemoryLayout<proc_bsdinfo>.size)
        XCTAssertEqual(proc_pidinfo(getpid(), PROC_PIDTBSDINFO, 0, &info, size), size)
        let start = Int(info.pbi_start_tvsec) * 1_000_000 + Int(info.pbi_start_tvusec)
        let current = SessionLocation(application: "Test", bundleId: "test", observedAtMs: 1,
                                      pid: Int(getpid()), processStart: start)
        XCTAssertTrue(SessionLocationLauncher.processIsCurrent(current))
        let reused = SessionLocation(application: "Test", bundleId: "test", observedAtMs: 1,
                                     pid: Int(getpid()), processStart: start + 1)
        XCTAssertFalse(SessionLocationLauncher.processIsCurrent(reused))
        let detached = SessionLocation(application: "Test", bundleId: "test", observedAtMs: 1,
                                       pid: Int(getpid()), processStart: start, tty: "/dev/null")
        XCTAssertFalse(SessionLocationLauncher.processIsCurrent(detached))
    }

    func testResumeShellPreservesWorkingDirectoryAndLiteralArguments() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("relay resume ' \(UUID())")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let value = "id'$(printf UNEXPECTED)`printf UNEXPECTED`"
        let command = SessionResumeCommand(args: ["-c", "printf '%s\\n%s' \"$PWD\" \"$1\"", "fixture", value],
                                           cwd: directory.path, executable: "/bin/sh")
        let process = Process(), output = Pipe()
        process.executableURL = URL(fileURLWithPath: "/bin/zsh")
        process.arguments = ["-lic", SessionTerminalAdapter.shellCommand(command)]
        process.standardOutput = output
        try process.run()
        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        XCTAssertEqual(process.terminationStatus, 0)
        // /tmp is canonically /private/tmp on macOS.
        XCTAssertEqual(String(decoding: data, as: UTF8.self), directory.resolvingSymlinksInPath().path + "\n" + value)
    }

    func testCapturedProviderStoreOverridesLoginShellAndVerifiedAbsenceClearsIt() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("relay-env-\(UUID())").resolvingSymlinksInPath()
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        try Data("export CLAUDE_CONFIG_DIR='/wrong/store'\n".utf8).write(to: directory.appendingPathComponent(".zshrc"))
        let literal = "/captured store's $(printf WRONG) `printf WRONG`"
        for captured in [["CLAUDE_CONFIG_DIR": literal], [:]] {
            let command = SessionResumeCommand(args: ["-c", "printf '%s' \"${CLAUDE_CONFIG_DIR-unset}\""], cwd: directory.path,
                                               environment: captured, executable: "/bin/sh")
            let process = Process(), output = Pipe()
            process.executableURL = URL(fileURLWithPath: "/bin/zsh")
            process.arguments = ["-lic", SessionTerminalAdapter.shellCommand(command)]
            var environment = ProcessInfo.processInfo.environment
            environment["ZDOTDIR"] = directory.path
            environment["CLAUDE_CONFIG_DIR"] = "/also-wrong"
            process.environment = environment
            process.standardOutput = output
            try process.run()
            let result = output.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            XCTAssertEqual(process.terminationStatus, 0)
            XCTAssertEqual(String(decoding: result, as: UTF8.self), captured["CLAUDE_CONFIG_DIR"] ?? "unset")
        }
        XCTAssertFalse(SessionTerminalAdapter.validEnvironment(SessionResumeCommand(args: [], cwd: directory.path,
            environment: ["PATH": "/unsupported"], executable: "codex")))
    }

    func testInstalledTerminalDictionariesCompileNavigationAndResumeScripts() throws {
        let command = SessionResumeCommand(args: ["resume", "test-id"], cwd: "/tmp", executable: "codex")
        for adapter in SessionTerminalAdapter.allCases where adapter.usesAppleEvents {
            guard NSWorkspace.shared.urlForApplication(withBundleIdentifier: adapter.rawValue) != nil else { continue }
            let location = SessionLocation(application: "Terminal", bundleId: adapter.rawValue, observedAtMs: 1, pid: 1, processStart: 1, tty: "/dev/ttys004")
            for script in [SessionLocationLauncher.script(for: location), adapter.resumeScript(command)] {
                var error: NSDictionary?
                XCTAssertTrue(NSAppleScript(source: try XCTUnwrap(script))!.compileAndReturnError(&error), "\(adapter): \(String(describing: error))")
            }
        }
    }
}
