import Foundation
import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

/// The Open button's logic over the helper's answers: which request each
/// answer leads to, and which of its words are shown.
@MainActor final class SessionOpenModelTests: XCTestCase {
    private func model(_ helper: FakeSessionHelper, directory: String? = "/work/repo") -> SessionOpenModel {
        SessionOpenModel(source: "codex", sessionId: "thread-1", directory: directory,
                         appDataDir: "/tmp/relay-data", engine: helper.engine())
    }

    /// Plans with `plan`, opens with `open`.
    private func helper(plan: [String: Any], open: [String: Any]? = nil) -> FakeSessionHelper {
        FakeSessionHelper(plan: plan, open: open)
    }

    private func result(_ outcome: String, message: String, precision: String? = "right_window",
                        after plan: [String: Any]) -> [String: Any] {
        ["outcome": outcome, "precision": precision ?? NSNull(), "reason_code": NSNull(), "message": message, "plan": plan]
    }

    func testPlanRequestNamesTheSessionAndDefersTheTerminalToTheSystem() async throws {
        let helper = helper(plan: FakeSessionHelper.plan(action: "resume", title: "Resume in Ghostty",
                                                         destination: "terminal"))
        let open = model(helper)
        XCTAssertNil(open.plan)
        XCTAssertEqual(open.title, SessionOpen.Strings.fallback.openSession)
        await open.refresh()
        let request = try XCTUnwrap(helper.calls("plan").first?.request)
        XCTAssertEqual(request["source"] as? String, "codex")
        XCTAssertEqual(request["session_id"] as? String, "thread-1")
        XCTAssertEqual(request["app_data_dir"] as? String, "/tmp/relay-data")
        XCTAssertEqual(request["directory"] as? String, "/work/repo")
        XCTAssertNil(request["default_terminal"], "No terminal setting exists: the system default is used")
        XCTAssertEqual(request["prefer_terminal"] as? Bool, false)
        XCTAssertEqual(request["launch_gone"] as? Bool, false, "A release is its own request")
        XCTAssertEqual(open.title, "Resume in Ghostty", "The title is the helper's")
        XCTAssertTrue(open.enabled)
        XCTAssertTrue(open.showsTerminal)
        XCTAssertTrue(helper.calls("open").isEmpty, "Showing a row never opens it")
        let undirected = model(helper, directory: "")
        await undirected.refresh()
        XCTAssertNil(helper.calls("plan").last?.request["directory"])
    }

    func testAFailedLookIsShownBesideTheLastPlan() async {
        let helper = FakeSessionHelper { _ in .init(status: 2, stdout: Data()) }
        let open = model(helper)
        await open.refresh()
        XCTAssertNil(open.plan)
        XCTAssertEqual(open.readError, SessionOpen.Strings.fallback.rejected)
        let silent = model(FakeSessionHelper { _ in .init(status: 1, stdout: Data()) })
        await silent.refresh()
        XCTAssertEqual(silent.readError, SessionOpen.Strings.fallback.planFailed)
    }

    func testTheTerminalPreferenceIsSentAndOnlyChangesTheRequest() async throws {
        let helper = helper(plan: FakeSessionHelper.plan(action: "resume", title: "Resume in Ghostty"))
        let open = SessionOpenModel(source: "codex", sessionId: "thread-1", directory: "/work/repo",
                                    appDataDir: "/tmp/relay-data", preferTerminal: true, engine: helper.engine())
        await open.refresh()
        await open.press()
        XCTAssertEqual(helper.calls("plan").last?.request["prefer_terminal"] as? Bool, true)
        XCTAssertEqual(helper.calls("open").last?.request["prefer_terminal"] as? Bool, true)
        XCTAssertEqual(helper.calls("open").last?.request["use_default_terminal"] as? Bool, false)
        open.preferTerminal = false
        await open.refresh()
        XCTAssertEqual(helper.calls("plan").last?.request["prefer_terminal"] as? Bool, false)
    }

    func testTheTerminalIconFollowsTheDestination() async {
        for (destination, terminal) in [("terminal", true), ("desktop_app", false), ("none", false), ("moon", false)] {
            let open = model(helper(plan: FakeSessionHelper.plan(action: "focus", title: "Open in Ghostty",
                                                                 destination: destination)))
            await open.refresh()
            XCTAssertEqual(open.showsTerminal, terminal, destination)
        }
    }

    func testOpenShowsTheOutcomesWords() async {
        let plan = FakeSessionHelper.plan(action: "focus", title: "Open in Ghostty", state: "live")
        let cases: [(String, String?, String?, String?)] = [
            // outcome, precision, notice, error
            ("verified", "exact_conversation", nil, nil),
            ("verified", "right_window", "Brought forward.", nil),
            ("dispatched_unverified", "right_window", "Sent.", nil),
            ("refused", nil, nil, "Refused."),
            ("failed", nil, nil, "Failed."),
            ("from_the_future", nil, nil, "Something new."),
        ]
        for (outcome, precision, notice, error) in cases {
            let message = notice ?? error ?? "Opened this session."
            let open = model(helper(plan: plan, open: result(outcome, message: message, precision: precision, after: plan)))
            await open.refresh()
            await open.press()
            XCTAssertEqual(open.notice, notice, outcome)
            XCTAssertEqual(open.error, error, outcome)
            XCTAssertFalse(open.opening)
        }
    }

    func testAnOpenUsesThePlanItReturns() async {
        let before = FakeSessionHelper.plan(action: "resume", title: "Resume in Ghostty")
        let after = FakeSessionHelper.plan(action: "unavailable", title: "Session opening unavailable", enabled: false,
                                           state: "launch_pending", reasonCode: "launch_pending",
                                           message: "A resume request was already sent.", pendingLaunch: true)
        let helper = helper(plan: before, open: result("dispatched_unverified", message: "Sent the resume request.", after: after))
        let open = model(helper)
        await open.refresh()
        await open.press()
        XCTAssertEqual(open.plan?.state, .launchPending)
        XCTAssertEqual(open.reason, "A resume request was already sent.")
        XCTAssertFalse(open.enabled)
        XCTAssertTrue(open.offersLaunchGone)
        XCTAssertEqual(helper.calls("plan").count, 1, "The open's own plan is used, not another look")
    }

    func testChooseSendsTheSelectedChoice() async throws {
        let helper = helper(plan: FakeSessionHelper.plan(action: "choose", title: "Open running session",
                                                         state: "live_multiple", reasonCode: "several_locations",
                                                         choices: [["id": "a", "label": "Ghostty (/dev/ttys004)"],
                                                                   ["id": "b", "label": "Claude"]]))
        let open = model(helper)
        await open.refresh()
        XCTAssertEqual(open.choices?.map(\.label), ["Ghostty (/dev/ttys004)", "Claude"])
        await open.press()
        XCTAssertTrue(helper.calls("open").isEmpty, "The button of a choice is a menu")
        await open.choose("b")
        let request = try XCTUnwrap(helper.calls("open").first?.request)
        XCTAssertEqual(request["selected"] as? String, "b")
        XCTAssertEqual(request["confirmed"] as? Bool, false)
    }

    func testConfirmAsksTheHelpersQuestionBeforeSendingConfirmed() async throws {
        let question = "This session may already be open in another process. Confirm it is not open there before resuming it here."
        let helper = helper(plan: FakeSessionHelper.plan(action: "confirm", title: "Resume in Ghostty",
                                                         reasonCode: "possibly_open_elsewhere", message: question,
                                                         question: question + "\nTerminal (/dev/ttys009)"))
        let open = model(helper)
        await open.refresh()
        await open.press()
        XCTAssertEqual(open.prompt, .confirm(question + "\nTerminal (/dev/ttys009)"), "The helper's question")
        XCTAssertTrue(helper.calls("open").isEmpty, "Nothing is sent before the person answers")
        await open.confirmNotOpenElsewhere()
        XCTAssertNil(open.prompt)
        let request = try XCTUnwrap(helper.calls("open").first?.request)
        XCTAssertEqual(request["confirmed"] as? Bool, true)
        XCTAssertEqual(request["take_over"] as? Bool, false)
    }

    func testTakeOverIsSentOnlyAfterThePersonChoosesIt() async throws {
        let helper = helper(plan: FakeSessionHelper.plan(action: "take_over", title: "Take over and resume",
                                                         reasonCode: "running_headless", message: "A background owner runs it.",
                                                         question: "A background owner runs it."))
        let open = model(helper)
        await open.refresh()
        await open.press()
        XCTAssertEqual(open.prompt, .takeOver("A background owner runs it."))
        XCTAssertTrue(helper.calls("open").isEmpty)
        open.prompt = nil // Cancel
        XCTAssertTrue(helper.calls("open").isEmpty)
        await open.press()
        await open.confirmTakeOver()
        let request = try XCTUnwrap(helper.calls("open").first?.request)
        XCTAssertEqual(request["take_over"] as? Bool, true)
        XCTAssertEqual(request["confirmed"] as? Bool, false)
    }

    func testAPendingLaunchIsReleasedOnlyWhenThePersonSaysItClosedAndNothingOpens() async throws {
        let pending = FakeSessionHelper.plan(action: "unavailable", title: "Session opening unavailable", enabled: false,
                                             state: "launch_pending", reasonCode: "launch_pending",
                                             message: "A resume request was already sent.", pendingLaunch: true)
        let resumable = FakeSessionHelper.plan(action: "resume", title: "Resume in Ghostty", destination: "terminal")
        let helper = FakeSessionHelper { call in
            switch call.verb {
            case "release": return .init(status: 0, stdout: jsonData(resumable))
            case "open": return .init(status: 0, stdout: jsonData(["outcome": "dispatched_unverified", "message": "Sent.", "plan": pending]))
            default: return .init(status: 0, stdout: jsonData(pending))
            }
        }
        let open = model(helper)
        await open.refresh()
        XCTAssertTrue(open.offersLaunchGone)
        XCTAssertFalse(open.enabled)
        open.askLaunchGone()
        XCTAssertEqual(open.prompt, .launchGone)
        open.prompt = nil // Keep waiting
        XCTAssertTrue(helper.calls("release").isEmpty, "Keep waiting sends nothing")
        open.askLaunchGone()
        await open.confirmLaunchGone()
        XCTAssertNil(open.prompt)
        XCTAssertEqual(helper.calls("release").count, 1)
        XCTAssertTrue(helper.calls("open").isEmpty, "Saying it closed opens nothing by itself")
        XCTAssertEqual(helper.calls("release").first?.request["launch_gone"] as? Bool, false,
                       "The release is the whole answer; launch_gone is never sent")
        XCTAssertEqual(open.title, "Resume in Ghostty", "The release's own plan is shown")
        XCTAssertFalse(open.offersLaunchGone)
        XCTAssertFalse(open.releasing)
        await open.press()
        XCTAssertEqual(helper.calls("open").count, 1)
        XCTAssertEqual(helper.calls("open").first?.request["launch_gone"] as? Bool, false)
        XCTAssertTrue(open.offersLaunchGone, "The next launch is pending again")
    }

    func testAReleaseWithoutAnAnswerSaysSoAndLooksAgain() async {
        let pending = FakeSessionHelper.plan(action: "unavailable", title: "Session opening unavailable", enabled: false,
                                             state: "launch_pending", reasonCode: "launch_pending", pendingLaunch: true)
        let helper = FakeSessionHelper { call in
            if call.verb == "release" { throw SessionOpenEngine.Failure.timedOut(.release) }
            return .init(status: 0, stdout: jsonData(pending))
        }
        let open = model(helper)
        await open.refresh()
        await open.confirmLaunchGone()
        XCTAssertEqual(open.error, SessionOpen.Strings.fallback.planFailed)
        XCTAssertEqual(helper.calls("plan").count, 2, "Looked again")
        XCTAssertFalse(open.releasing)
        XCTAssertTrue(open.offersLaunchGone)
    }

    func testDefaultTerminalIsOfferedWhenThePlanAllowsIt() async throws {
        let unavailable = FakeSessionHelper.plan(action: "unavailable", title: "Session opening unavailable", enabled: false,
                                                 reasonCode: "app_not_installed", message: "Not installed.", canResumeDefault: true)
        let helper = helper(plan: unavailable)
        let open = model(helper)
        await open.refresh()
        XCTAssertTrue(open.offersDefaultResume)
        await open.resumeInDefaultTerminal()
        XCTAssertEqual(helper.calls("open").first?.request["use_default_terminal"] as? Bool, true)

        // An enabled plan offers it only after its own open failed.
        let resume = FakeSessionHelper.plan(action: "resume", title: "Resume in Kitty", canResumeDefault: true)
        let failing = model(self.helper(plan: resume, open: result("failed", message: "Kitty failed.", precision: nil, after: resume)))
        await failing.refresh()
        XCTAssertFalse(failing.offersDefaultResume)
        await failing.press()
        XCTAssertEqual(failing.error, "Kitty failed.")
        XCTAssertTrue(failing.offersDefaultResume)

        let pending = FakeSessionHelper.plan(action: "unavailable", title: "Session opening unavailable", enabled: false,
                                             reasonCode: "launch_pending", canResumeDefault: true, pendingLaunch: true)
        let blocked = model(self.helper(plan: pending))
        await blocked.refresh()
        XCTAssertFalse(blocked.offersDefaultResume, "A pending launch is resolved first")
    }

    func testDisabledAndUnknownActionsAreShownButNeverSent() async {
        for plan in [
            FakeSessionHelper.plan(action: "resume", title: "Resume in Ghostty", enabled: false, message: "Busy."),
            FakeSessionHelper.plan(action: "unavailable", title: "Session opening unavailable", enabled: false),
            FakeSessionHelper.plan(action: "teleport", title: "Teleport there"),
        ] {
            let helper = helper(plan: plan)
            let open = model(helper)
            await open.refresh()
            XCTAssertEqual(open.title, plan["title"] as? String)
            XCTAssertFalse(open.enabled)
            await open.press()
            XCTAssertTrue(helper.calls("open").isEmpty)
        }
    }

    func testOneOpenAtATimeAndTheButtonSaysSo() async throws {
        let gate = Gate()
        let plan = FakeSessionHelper.plan(action: "focus", title: "Open in Ghostty")
        let helper = FakeSessionHelper { call in
            if call.verb == "open" {
                await gate.wait()
                return .init(status: 0, stdout: jsonData(["outcome": "verified", "precision": "exact_conversation",
                                                      "message": "Opened this session.", "plan": plan]))
            }
            return .init(status: 0, stdout: jsonData(plan))
        }
        let open = model(helper)
        await open.refresh()
        let first = Task { await open.press() }
        while helper.calls("open").isEmpty { await Task.yield() }
        XCTAssertEqual(open.title, SessionOpen.Strings.fallback.opening)
        XCTAssertFalse(open.enabled)
        await open.open()
        await open.resumeInDefaultTerminal()
        XCTAssertEqual(helper.calls("open").count, 1, "A second click while opening sends nothing")
        await gate.open()
        await first.value
        XCTAssertFalse(open.opening)
        XCTAssertEqual(open.title, "Open in Ghostty")
    }

    func testATimedOutOpenIsNotRetriedAndLooksAgain() async {
        let plan = FakeSessionHelper.plan(action: "resume", title: "Resume in Ghostty")
        let helper = FakeSessionHelper { call in
            if call.verb == "open" { throw SessionOpenEngine.Failure.timedOut(.open) }
            return .init(status: 0, stdout: jsonData(plan))
        }
        let open = model(helper)
        await open.refresh()
        await open.press()
        XCTAssertEqual(helper.calls("open").count, 1)
        XCTAssertEqual(helper.calls("plan").count, 2, "After an unknown outcome the button is planned afresh")
        XCTAssertEqual(open.error, SessionOpen.Strings.fallback.openUnknown)
        XCTAssertFalse(open.opening)
        XCTAssertTrue(open.enabled, "The button works again")
    }

    func testAnOpenWithoutAnAnswerIsNotRetriedAndUsesTheLastAnswersWords() async {
        var plan = FakeSessionHelper.plan(action: "resume", title: "Resume in Ghostty")
        var words = try! JSONSerialization.jsonObject(with: JSONEncoder().encode(SessionOpen.Strings.fallback)) as! [String: Any]
        words["open_unknown"] = "Unknown outcome (helper's words)."
        plan["strings"] = words
        let answer = plan
        for status in [Int32(1), -1] {
            let helper = FakeSessionHelper { call in
                .init(status: call.verb == "open" ? status : 0, stdout: call.verb == "open" ? Data() : jsonData(answer))
            }
            let open = model(helper)
            await open.refresh()
            await open.press()
            XCTAssertEqual(helper.calls("open").count, 1, "Never retried")
            XCTAssertEqual(open.error, "Unknown outcome (helper's words).")
            XCTAssertEqual(helper.calls("plan").count, 2)
            XCTAssertTrue(open.enabled)
        }
    }

    func testTheHelperIsFoundThroughTheBundledLaunchAgent() throws {
        let bundle = FileManager.default.temporaryDirectory.appendingPathComponent("Relay-\(UUID()).app")
        defer { try? FileManager.default.removeItem(at: bundle) }
        let agents = bundle.appendingPathComponent("Contents/Library/LaunchAgents")
        let helpers = bundle.appendingPathComponent("Contents/Helpers")
        try FileManager.default.createDirectory(at: agents, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: helpers, withIntermediateDirectories: true)
        let plist = agents.appendingPathComponent(LaunchAgentService.plistName)
        func write(program: String) throws {
            try PropertyListSerialization.data(fromPropertyList: ["BundleProgram": program, "ProgramArguments": ["serve"]],
                                               format: .xml, options: 0).write(to: plist)
        }
        try write(program: "Contents/Helpers/Relay Helper")
        XCTAssertNil(SessionOpenEngine.liveHelper(bundleURL: bundle, environment: [:]), "No executable yet")
        let helper = helpers.appendingPathComponent("Relay Helper")
        try Data("#!/bin/sh\n".utf8).write(to: helper)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: helper.path)
        XCTAssertEqual(SessionOpenEngine.liveHelper(bundleURL: bundle, environment: [:])?.path,
                       helper.standardizedFileURL.path)
        XCTAssertEqual(SessionOpenEngine.liveHelper(bundleURL: bundle, environment: ["AGENT_RELAY_SESSION_HELPER": "/bin/sh"])?.path,
                       helper.standardizedFileURL.path, "A bundled helper always wins")
        for escape in ["Contents/Helpers/../../../outside", "Contents/MacOS/Relay", "/bin/sh"] {
            try write(program: escape)
            XCTAssertNil(LaunchAgentService.bundledHelper(in: bundle), escape)
        }
        let bare = FileManager.default.temporaryDirectory.appendingPathComponent("NoBundle-\(UUID())")
        XCTAssertNil(SessionOpenEngine.liveHelper(bundleURL: bare, environment: [:]))
        XCTAssertEqual(SessionOpenEngine.liveHelper(bundleURL: bare, environment: ["AGENT_RELAY_SESSION_HELPER": "/bin/sh"])?.path, "/bin/sh")
        XCTAssertNil(SessionOpenEngine.liveHelper(bundleURL: bare, environment: ["AGENT_RELAY_SESSION_HELPER": "relative/helper"]))
    }

    func testOpenCodeSessionsUseTheHelper() {
        XCTAssertEqual(SessionOpenButton.sources, ["claude", "codex", "grok", "opencode"])
    }
}

private func jsonData(_ object: [String: Any]) -> Data { try! JSONSerialization.data(withJSONObject: object) }
