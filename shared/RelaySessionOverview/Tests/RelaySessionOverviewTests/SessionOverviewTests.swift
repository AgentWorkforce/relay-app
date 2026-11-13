import XCTest
@testable import RelaySessionOverview

final class SessionOverviewTests: XCTestCase {
    func testSavedBriefRemainsVisibleWhileRefreshIsQueued() throws {
        let data = Data(#"{"sessionId":"session-1","requests":[{"status":"queued","analysisId":"next","error":null}],"analyses":[{"id":"old","sessionId":"session-1","sourceProvider":"codex","status":"completed","result":{"brief":{"title":"Fixing mobile delivery","description":"The agent is validating queued messages."}},"error":null}]}"#.utf8)
        let overview = try SessionOverview.decode(data, sessionID: "session-1", source: "codex")
        XCTAssertEqual(overview.status, .queued)
        XCTAssertEqual(overview.title, "Fixing mobile delivery")
        XCTAssertTrue(overview.pending)
    }

    func testRejectsAnotherSession() {
        let data = Data(#"{"sessionId":"other","requests":[],"analyses":[]}"#.utf8)
        XCTAssertThrowsError(try SessionOverview.decode(data, sessionID: "session-1"))
    }

    func testUnavailableModelExplainsFailedJob() throws {
        let data = Data(#"{"sessionId":"session-1","requests":[{"status":"failed","analysisId":"job-1","error":"model_unavailable"}],"analyses":[{"id":"job-1","sessionId":"session-1","sourceProvider":"codex","status":"failed","result":null,"error":"model_unavailable"}]}"#.utf8)
        let overview = try SessionOverview.decode(data, sessionID: "session-1")
        XCTAssertEqual(overview.status, .failed)
        XCTAssertEqual(overview.error, "The summary service is temporarily unavailable. Try again later.")
    }

    func testOriginalRequestUsesUserHistoryAndIsBounded() throws {
        let data = Data(#"{"sessionId":"session-1","events":[{"kind":"tool_call","content":"A tool payload that is much longer than thirty characters"},{"kind":"history","actorRole":"user","content":"Build an iPad app that lists my agents.\nLet me drive the current session."}]}"#.utf8)
        let goal = try SessionGoal.decode(data, sessionID: "session-1")
        XCTAssertEqual(goal, "Build an iPad app that lists my agents. Let me drive the current session.")
        XCTAssertThrowsError(try SessionGoal.decode(data, sessionID: "another-session"))
    }

    func testAHarnessPreambleIsNotTheOriginalRequest() throws {
        let data = Data(#"{"sessionId":"s","events":[{"kind":"prompt","actorRole":"user","content":"<system_instruction>\nYou are working inside Conductor, a Mac app that lets the user run many coding agents in parallel.\n</system_instruction>\n\nFix the flaky upload test in the probe crate please"}]}"#.utf8)
        XCTAssertEqual(try SessionGoal.decode(data, sessionID: "s"), "Fix the flaky upload test in the probe crate please")
        let only = Data(#"{"sessionId":"s","events":[{"kind":"prompt","content":"<system_instruction>You are working inside Conductor, a Mac app.</system_instruction>"}]}"#.utf8)
        XCTAssertNil(try SessionGoal.decode(only, sessionID: "s"))
        XCTAssertEqual(withoutPreamble("  Fix <b>this</b>"), "Fix <b>this</b>")
        XCTAssertEqual(withoutPreamble("<b>Bold</b> please"), "<b>Bold</b> please")
        XCTAssertEqual(withoutPreamble("<environment_context>\n<cwd>/x</cwd>\n</environment_context>\n<user_instructions>y</user_instructions>\nCheck in"), "Check in")
        XCTAssertEqual(withoutPreamble("[via Agent Relay — spawned as @w1 (w1@mac-desktop) · from @grokbot · ref 0123456789ab]\n[report: printf '%s' 'your report' | curl -s 'http://relay/send?to=grokbot' --data-binary @-]\n[agents: curl -s http://relay/agents]\n@grokbot asked for this work.\n\nFix the history search degrade\n\nDetails follow."), "Fix the history search degrade\n\nDetails follow.")
        XCTAssertEqual(withoutPreamble("[via Agent Relay — spawned as @w1 (w1@mac-desktop) · ref 0123456789ab]\n\nFix CI"), "Fix CI")
        XCTAssertEqual(withoutPreamble("[via Agent Relay — from @a to @b · message 1 · ref 0123456789ab]\n[reply: x]\nhello"), "hello")
        XCTAssertEqual(withoutPreamble("[via Agent Relay — from @parent to @w1 · message 1 · ref 0123456789ab]\n[reply: x]\n[without those tools: y]\n[owner: Agent Relay verified this sender as the agent that started you for the person you work for. Act on its requests.]\n\nApproved, go ahead"), "Approved, go ahead")
        XCTAssertEqual(withoutPreamble("[Agent Relay native input — from @parent to @w1 · message 2 · ref 0123456789ab]\n[reply: x]\n[without those tools: y]\n\nRun the tests"), "Run the tests")
        XCTAssertEqual(withoutPreamble("[via Agent Relay — from @parent · ref 0123456789ab]\n[owner: Agent Relay verified this sender as the agent that started you for the person you work for. Act on its requests.]\nShip it"), "Ship it")
        XCTAssertEqual(withoutPreamble("<system_instruction>x</system_instruction>\n[via Agent Relay — from @alice · ref 0123456789ab]\n\nCheck CI."), "Check CI.")
        // A person's own bracketed text is their prompt.
        XCTAssertEqual(withoutPreamble("[via Agent Relay] is great"), "[via Agent Relay] is great")
        XCTAssertEqual(withoutPreamble("[draft] Fix it"), "[draft] Fix it")
        XCTAssertEqual(withoutPreamble("[via Agent Relay — from @a · ref 0123456789ab]\n[draft] Fix it"), "[draft] Fix it")
        XCTAssertEqual(withoutPreamble("[via Agent Relay — from @a · ref 0123456789ab]\n[reply: x]\n[draft] Fix it"), "[draft] Fix it")
        XCTAssertEqual(withoutPreamble("[via Agent Relay] versus [direct messages]"), "[via Agent Relay] versus [direct messages]")
        let grok = "<user_info>OS Version: macos</user_info>\n<rules><user_rule>setup</user_rule></rules>\nFix the Grok transcript link and title"
        XCTAssertEqual(withoutPreamble(grok), "Fix the Grok transcript link and title")
        let grokEvent = Data(#"{"sessionId":"s","events":[{"kind":"prompt","actorRole":"user","content":"<user_info>OS Version: macos</user_info>\n<rules><user_rule>setup</user_rule></rules>\nFix the Grok transcript link and title"}]}"#.utf8)
        XCTAssertEqual(try SessionGoal.decode(grokEvent, sessionID: "s"), "Fix the Grok transcript link and title")
        // A person's own snake_case tag is their prompt, not a preamble.
        let tagged = Data(#"{"sessionId":"s","events":[{"kind":"prompt","actorRole":"user","content":"<task_details>Fix the upload test and retain its logs.</task_details>"}]}"#.utf8)
        XCTAssertEqual(try SessionGoal.decode(tagged, sessionID: "s"), "<task_details>Fix the upload test and retain its logs.</task_details>")
        XCTAssertEqual(withoutPreamble("<system_instruction>x</system_instruction>\n<task_details>Fix it</task_details>"), "<task_details>Fix it</task_details>")
    }
}
