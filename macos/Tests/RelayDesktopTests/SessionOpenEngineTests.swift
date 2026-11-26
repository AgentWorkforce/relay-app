import Foundation
import XCTest
@testable import RelayDesktopCore

/// The app's side of `desktop-core/src/session_open/{contract,command}.rs`:
/// what it sends the helper and how it reads every answer.
final class SessionOpenEngineTests: XCTestCase {
    private func json(_ request: SessionOpen.Request) throws -> [String: Any] {
        try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(request)) as? [String: Any])
    }

    private let base = SessionOpen.Request(source: "codex", sessionId: "01900000-0000-7000-8000-000000000002",
                                           appDataDir: "/tmp/relay-data")

    func testRequestEncodesOnlyTheContractsFieldsInSnakeCase() throws {
        let encoded = try json(base)
        XCTAssertEqual(Set(encoded.keys), ["source", "session_id", "app_data_dir", "use_default_terminal",
                                           "prefer_terminal", "confirmed", "launch_gone", "take_over"],
                       "Absent optionals are omitted; the helper refuses unknown fields")
        XCTAssertEqual(encoded["source"] as? String, "codex")
        XCTAssertEqual(encoded["session_id"] as? String, "01900000-0000-7000-8000-000000000002")
        XCTAssertEqual(encoded["app_data_dir"] as? String, "/tmp/relay-data")
        for flag in ["use_default_terminal", "prefer_terminal", "confirmed", "launch_gone", "take_over"] {
            XCTAssertEqual(encoded[flag] as? Bool, false, flag)
        }
        let every = SessionOpen.Request(source: "claude", sessionId: "s", appDataDir: "/d", directory: "/work/repo",
                                        defaultTerminal: "ghostty", selected: "choice-1")
        let full = try json(every)
        XCTAssertEqual(Set(full.keys), ["source", "session_id", "app_data_dir", "directory", "use_default_terminal",
                                        "prefer_terminal", "default_terminal", "confirmed", "launch_gone",
                                        "take_over", "selected"])
        XCTAssertEqual(full["directory"] as? String, "/work/repo")
        XCTAssertEqual(full["default_terminal"] as? String, "ghostty")
        XCTAssertEqual(full["selected"] as? String, "choice-1")
    }

    func testEachResendFlagIsEncodedAlone() throws {
        let cases: [(String, (inout SessionOpen.Request) -> Void)] = [
            ("use_default_terminal", { $0.useDefaultTerminal = true }),
            ("prefer_terminal", { $0.preferTerminal = true }),
            ("confirmed", { $0.confirmed = true }),
            ("launch_gone", { $0.launchGone = true }),
            ("take_over", { $0.takeOver = true }),
        ]
        for (flag, set) in cases {
            var request = base
            set(&request)
            let encoded = try json(request)
            for other in ["use_default_terminal", "prefer_terminal", "confirmed", "launch_gone", "take_over"] {
                XCTAssertEqual(encoded[other] as? Bool, other == flag, "\(flag) sets only \(flag)")
            }
        }
    }

    private func plan(_ fields: [String: Any]) throws -> SessionOpen.Plan {
        try JSONDecoder().decode(SessionOpen.Plan.self, from: JSONSerialization.data(withJSONObject: fields))
    }

    func testDecodesEveryActionStatePrecisionAndOutcome() throws {
        let actions: [String: SessionOpen.Action] = [
            "focus": .focus, "show_application": .showApplication, "open_link": .openLink, "resume": .resume,
            "take_over": .takeOver, "attach": .attach, "choose": .choose, "confirm": .confirm,
            "unavailable": .unavailable, "teleport": .unknown("teleport"),
        ]
        for (raw, action) in actions { XCTAssertEqual(try plan(["action": raw]).action, action, raw) }
        let states: [String: SessionOpen.State] = [
            "live": .live, "live_multiple": .liveMultiple, "live_not_locatable": .liveNotLocatable,
            "live_headless": .liveHeadless, "stopped": .stopped, "archived": .archived, "not_found": .notFound,
            "launch_pending": .launchPending, "unknown": .unknownState, "invalid_id": .invalidId,
            "unsupported": .unsupported, "hibernating": .unknown("hibernating"),
        ]
        for (raw, state) in states { XCTAssertEqual(try plan(["state": raw, "action": "focus"]).state, state, raw) }
        let precisions: [String: SessionOpen.Precision] = [
            "exact_conversation": .exactConversation, "right_window": .rightWindow, "app_only": .appOnly,
            "same_galaxy": .unknown("same_galaxy"),
        ]
        for (raw, precision) in precisions {
            XCTAssertEqual(try plan(["action": "focus", "precision": raw]).precision, precision, raw)
        }
        let outcomes: [String: SessionOpen.Outcome] = [
            "verified": .verified, "dispatched_unverified": .dispatchedUnverified, "refused": .refused,
            "failed": .failed, "maybe": .unknown("maybe"),
        ]
        for (raw, outcome) in outcomes {
            let data = try JSONSerialization.data(withJSONObject: ["outcome": raw, "message": "m", "plan": ["action": "focus"]])
            XCTAssertEqual(try JSONDecoder().decode(SessionOpen.OpenResult.self, from: data).outcome, outcome, raw)
        }
    }

    func testDecodesTheContractsExamplesAndToleratesMissingOrExtraFields() throws {
        // desktop-core/src/session_open.rs, module documentation.
        let example = """
        {"state": "stopped", "action": "resume", "title": "Resume in Ghostty",
         "enabled": true, "reason_code": null, "message": null,
         "precision": "right_window", "destination": "terminal", "question": null,
         "can_resume_default": false, "pending_launch": false, "choices": [],
         "elsewhere": [], "strings": {"open_session": "Open session", "cancel": "Abbrechen"}}
        """
        var strings = SessionOpen.Strings.fallback
        strings.cancel = "Abbrechen"
        XCTAssertEqual(try JSONDecoder().decode(SessionOpen.Plan.self, from: Data(example.utf8)),
                       SessionOpen.Plan(state: .stopped, action: .resume, title: "Resume in Ghostty",
                                        precision: .rightWindow, destination: .terminal, strings: strings),
                       "A word the answer leaves out is this build's own")
        let destinations: [String: SessionOpen.Destination] = [
            "terminal": .terminal, "desktop_app": .desktopApp, "none": .none, "hologram": .unknown("hologram"),
        ]
        for (raw, destination) in destinations {
            XCTAssertEqual(try plan(["action": "focus", "destination": raw]).destination, destination, raw)
        }
        XCTAssertEqual(try plan(["action": "confirm", "question": "Open here?"]).question, "Open here?")
        XCTAssertEqual(try plan(["action": "focus", "strings": 7]).strings, .fallback, "A bad table is ignored")
        let choose = try plan(["state": "live_multiple", "action": "choose", "title": "Open running session",
                               "enabled": true, "reason_code": "several_locations",
                               "message": "This session is open in more than one place. Choose where to open it.",
                               "choices": [["id": "t1", "label": "Ghostty (/dev/ttys004)"]],
                               "elsewhere": ["Claude"], "added_in_a_newer_helper": 1])
        XCTAssertEqual(choose.choices, [SessionOpen.Choice(id: "t1", label: "Ghostty (/dev/ttys004)")])
        XCTAssertEqual(choose.elsewhere, ["Claude"])
        XCTAssertEqual(choose.reasonCode, "several_locations")
        let bare = try plan([:])
        XCTAssertEqual(bare.action, .unknown(""))
        XCTAssertEqual(bare.destination, .none)
        XCTAssertEqual(bare.strings, .fallback)
        XCTAssertFalse(bare.enabled)
        XCTAssertFalse(bare.actionable, "An answer without an action is never sent")
        XCTAssertFalse(try plan(["action": "teleport", "enabled": true]).actionable)
        XCTAssertFalse(try plan(["action": "unavailable", "enabled": true]).actionable)
        XCTAssertTrue(try plan(["action": "focus", "enabled": true]).actionable)

        let open = """
        {"outcome": "dispatched_unverified", "precision": "right_window", "reason_code": null,
         "message": "Sent the resume request for this session. The provider has not confirmed it opened.",
         "plan": {"state": "launch_pending", "action": "unavailable", "title": "Session opening unavailable",
                  "enabled": false, "pending_launch": true, "reason_code": "launch_pending"}}
        """
        let result = try JSONDecoder().decode(SessionOpen.OpenResult.self, from: Data(open.utf8))
        XCTAssertEqual(result.outcome, .dispatchedUnverified)
        XCTAssertEqual(result.plan?.state, .launchPending)
        XCTAssertEqual(result.plan?.pendingLaunch, true)
    }

    // MARK: - Running the helper

    private func engine(status: Int32, stdout: String) -> SessionOpenEngine {
        SessionOpenEngine(runner: { _, _, _ in .init(status: status, stdout: Data(stdout.utf8)) })
    }

    private func failure(_ body: () async throws -> Void) async -> SessionOpenEngine.Failure? {
        do {
            try await body()
            return nil
        } catch {
            return error as? SessionOpenEngine.Failure
        }
    }

    func testExitCodes() async throws {
        let answer = #"{"action":"focus","title":"Open in Ghostty","enabled":true}"# + "\n"
        let plan = try await engine(status: 0, stdout: answer).plan(base)
        XCTAssertEqual(plan.title, "Open in Ghostty")
        let rejected = await failure { _ = try await self.engine(status: 2, stdout: "").plan(self.base) }
        XCTAssertEqual(rejected, .rejected)
        let unwritten = await failure { _ = try await self.engine(status: 1, stdout: "").open(self.base) }
        XCTAssertEqual(unwritten, .noAnswer(.open, 1))
        let crashed = await failure { _ = try await self.engine(status: -1, stdout: answer).plan(self.base) }
        XCTAssertEqual(crashed, .noAnswer(.plan, -1))
        let released = try await engine(status: 0, stdout: answer).release(base)
        XCTAssertEqual(released.title, "Open in Ghostty")
    }

    func testMalformedStdoutIsAFailureNotACrash() async {
        for stdout in ["", "not json", "[1,2]", #"{"action": 7}"#, #"{"outcome":"verified"}{"#] {
            let planned = await failure { _ = try await self.engine(status: 0, stdout: stdout).plan(self.base) }
            XCTAssertEqual(planned, .malformedAnswer(.plan), stdout)
        }
    }

    func testRunsTheVerbItWasAskedWithTheRequestOnStdin() async throws {
        let helper = FakeSessionHelper(plan: FakeSessionHelper.plan(action: "focus", title: "Open in Ghostty"))
        let engine = helper.engine()
        _ = try await engine.plan(base)
        _ = try await engine.open(base)
        _ = try await engine.release(base)
        XCTAssertEqual(helper.calls.map(\.arguments),
                       [["session-open", "plan"], ["session-open", "open"], ["session-open", "release"]])
        XCTAssertEqual(helper.calls.first?.request["session_id"] as? String, base.sessionId)
    }

    func testWithoutAHelperEveryCallIsUnavailable() async {
        let engine = SessionOpenEngine(runner: nil)
        XCTAssertFalse(engine.isAvailable)
        let planned = await failure { _ = try await engine.plan(self.base) }
        XCTAssertEqual(planned, .unavailable)
        let released = await failure { _ = try await engine.release(self.base) }
        XCTAssertEqual(released, .unavailable)
        XCTAssertEqual(SessionOpenEngine.Failure.unavailable.errorDescription,
                       SessionOpen.Strings.fallback.helperMissing)
    }

    func testOneOpenPerSessionAtATime() async throws {
        let gate = Gate()
        let answer = Data(#"{"outcome":"verified","message":"Opened this session.","plan":{"action":"focus"}}"#.utf8)
        let engine = SessionOpenEngine(runner: { arguments, _, _ in
            if arguments.last == "open" { await gate.wait() }
            return .init(status: 0, stdout: arguments.last == "open" ? answer : Data(#"{"action":"focus"}"#.utf8))
        })
        let first = Task { try await engine.open(self.base) }
        while !engine.isOpening(source: base.source, sessionId: base.sessionId) { await Task.yield() }
        let second = await failure { _ = try await engine.open(self.base) }
        XCTAssertEqual(second, .alreadyOpening)
        let release = await failure { _ = try await engine.release(self.base) }
        XCTAssertEqual(release, .alreadyOpening, "A release never runs beside an open of the session")
        var other = base
        other.sessionId = "another"
        let otherGate = Task { try await engine.open(other) }
        _ = try await engine.plan(base)
        await gate.open()
        let firstOutcome = try await first.value.outcome
        let otherOutcome = try await otherGate.value.outcome
        XCTAssertEqual(firstOutcome, .verified)
        XCTAssertEqual(otherOutcome, .verified)
        XCTAssertFalse(engine.isOpening(source: base.source, sessionId: base.sessionId))
        let again = try await engine.open(base).outcome
        XCTAssertEqual(again, .verified, "A finished open frees its session")
    }

    // MARK: - The real process runner, on a stand-in script

    private func script(_ body: String) throws -> URL {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("session-open-\(UUID())")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: directory) }
        let file = directory.appendingPathComponent("helper")
        try Data("#!/bin/sh\n\(body)\n".utf8).write(to: file)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: file.path)
        return file
    }

    func testProcessRunnerPipesTheRequestAndReadsTheAnswer() async throws {
        // Echoes the verb and the request's session id back as the title.
        let helper = try script(#"""
        input=$(cat)
        id=$(printf '%s' "$input" | sed -n 's/.*"session_id":"\([^"]*\)".*/\1/p')
        printf '{"action":"focus","title":"%s %s %s","enabled":true}\n' "$1" "$2" "$id"
        """#)
        let plan = try await SessionOpenEngine(executable: helper).plan(base)
        XCTAssertEqual(plan.title, "session-open plan \(base.sessionId)")
    }

    func testProcessRunnerReportsExitCodesAndIgnoresStderr() async throws {
        let rejected = await failure {
            _ = try await SessionOpenEngine(executable: try self.script("cat >/dev/null; echo 'bad request' >&2; exit 2")).plan(self.base)
        }
        XCTAssertEqual(rejected, .rejected)
        let unwritten = await failure {
            _ = try await SessionOpenEngine(executable: try self.script("exit 1")).open(self.base)
        }
        XCTAssertEqual(unwritten, .noAnswer(.open, 1))
        let missing = await failure {
            _ = try await SessionOpenEngine(executable: URL(fileURLWithPath: "/nonexistent/helper")).plan(self.base)
        }
        XCTAssertEqual(missing, .launchFailed(.plan))
    }

    func testProcessRunnerTimesOutWithoutRetrying() async throws {
        let counter = try script("cat >/dev/null; echo x >> \"$(dirname \"$0\")/runs\"; sleep 30")
        let engine = SessionOpenEngine(runner: SessionOpenEngine.processRunner(executable: counter),
                                       planTimeout: 0.3, openTimeout: 0.3)
        let started = Date()
        let planned = await failure { _ = try await engine.plan(self.base) }
        XCTAssertEqual(planned, .timedOut(.plan))
        let opened = await failure { _ = try await engine.open(self.base) }
        XCTAssertEqual(opened, .timedOut(.open))
        XCTAssertLessThan(Date().timeIntervalSince(started), 10, "A stuck helper is stopped")
        let runs = try String(contentsOf: counter.deletingLastPathComponent().appendingPathComponent("runs"), encoding: .utf8)
        XCTAssertEqual(runs.split(separator: "\n").count, 2, "Each call ran the helper once")
        XCTAssertFalse(engine.isOpening(source: base.source, sessionId: base.sessionId))
    }

    func testAChildThatKeepsStdoutOpenDoesNotHoldTheAnswer() async throws {
        let helper = try script(#"cat >/dev/null; (sleep 30 &); printf '{"action":"focus","title":"t","enabled":true}\n'"#)
        let started = Date()
        let plan = try await SessionOpenEngine(executable: helper).plan(base)
        XCTAssertEqual(plan.title, "t")
        XCTAssertLessThan(Date().timeIntervalSince(started), 6)
    }

    /// Opt in with the helper built from desktop-core:
    /// `cargo build --manifest-path desktop-core/Cargo.toml --bin agent-relay-desktop-probe`, then
    /// `RELAY_SESSION_OPEN_HELPER=<path> swift test --filter SessionOpenEngineTests/testRealHelperPlansAMadeUpSession`.
    /// Runs `plan` only: read-only and prompt-free; never `open`.
    func testRealHelperPlansAMadeUpSession() async throws {
        guard let path = ProcessInfo.processInfo.environment["RELAY_SESSION_OPEN_HELPER"] else {
            throw XCTSkip("Set RELAY_SESSION_OPEN_HELPER to the built agent-relay-desktop-probe")
        }
        let data = FileManager.default.temporaryDirectory.appendingPathComponent("session-open-data-\(UUID())")
        try FileManager.default.createDirectory(at: data, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: data) }
        let engine = SessionOpenEngine(executable: URL(fileURLWithPath: path))
        for source in ["claude", "codex", "grok", "opencode"] {
            let request = SessionOpen.Request(source: source, sessionId: "01900000-0000-7000-8000-00000000dead",
                                              appDataDir: data.path)
            let plan = try await engine.plan(request)
            XCTAssertFalse(plan.title.isEmpty, source)
            if case .unknown(let raw) = plan.action { XCTFail("\(source): unknown action \(raw)") }
            if case .unknown(let raw) = plan.state { XCTFail("\(source): unknown state \(raw)") }
            XCTAssertFalse(plan.actionable, "\(source): a made-up session cannot be opened")
        }
        let unsupported = try await engine.plan(SessionOpen.Request(source: "nonesuch", sessionId: "x", appDataDir: data.path))
        XCTAssertEqual(unsupported.reasonCode, "unsupported_source")
        let invalid = try await engine.plan(SessionOpen.Request(source: "codex", sessionId: "x", appDataDir: "relative"))
        XCTAssertEqual(invalid.reasonCode, "invalid_request")
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: data.path), [], "plan writes nothing")
    }
}

/// Holds callers until opened.
actor Gate {
    private var isOpen = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    func wait() async {
        if isOpen { return }
        await withCheckedContinuation { waiters.append($0) }
    }

    func open() {
        isOpen = true
        waiters.forEach { $0.resume() }
        waiters = []
    }
}
