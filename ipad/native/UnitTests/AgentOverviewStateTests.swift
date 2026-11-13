import XCTest
@testable import AgentRelayiPad

final class AgentOverviewStateTests: XCTestCase {
    func testA404ForTheSessionMeansItIsMissingFromHistory() {
        for code in ["", "not_found", "session_not_found", "SESSION_NOT_FOUND"] {
            XCTAssertTrue(AgentOverviewState.isMissingFromHistory(status: 404, code: code), code)
        }
    }

    func testOtherFailuresAreNotMissingFromHistory() {
        XCTAssertFalse(AgentOverviewState.isMissingFromHistory(status: 404, code: "workspace_not_found"))
        XCTAssertFalse(AgentOverviewState.isMissingFromHistory(status: 500, code: ""))
        XCTAssertFalse(AgentOverviewState.isMissingFromHistory(status: 429, code: "not_found"))
        XCTAssertFalse(AgentOverviewState.isMissingFromHistory(status: nil, code: "session_not_found"))
    }

    func testMissingSessionExplainsItselfInsteadOfOfferingGenerate() {
        let hint = AgentOverviewState.hint(hasSessionID: true, hasObjective: true,
                                           canLoadSummaries: true, loading: false,
                                           missingFromHistory: true)
        XCTAssertEqual(hint, AgentOverviewState.missingFromHistory)
        XCTAssertFalse(hint.contains("Generate"))
        XCTAssertFalse(AgentOverviewState.offersGenerate(hasSessionID: true, canLoadSummaries: true,
                                                         missingFromHistory: true))
    }

    func testMissingSessionTakesPrecedenceOverLoading() {
        XCTAssertEqual(
            AgentOverviewState.hint(hasSessionID: true, hasObjective: false, canLoadSummaries: true,
                                    loading: true, missingFromHistory: true),
            AgentOverviewState.missingFromHistory)
    }

    func testHintForEachRemainingState() {
        XCTAssertEqual(
            AgentOverviewState.hint(hasSessionID: false, hasObjective: true, canLoadSummaries: true,
                                    loading: false, missingFromHistory: false),
            "No Cloud session summary is linked to this chat.")
        XCTAssertEqual(
            AgentOverviewState.hint(hasSessionID: false, hasObjective: false, canLoadSummaries: true,
                                    loading: false, missingFromHistory: false),
            "This agent has not published a session summary.")
        XCTAssertEqual(
            AgentOverviewState.hint(hasSessionID: true, hasObjective: true, canLoadSummaries: false,
                                    loading: false, missingFromHistory: false),
            "Cloud access needs approval. Enable summaries once in Settings.")
        XCTAssertEqual(
            AgentOverviewState.hint(hasSessionID: true, hasObjective: true, canLoadSummaries: true,
                                    loading: true, missingFromHistory: false),
            "Checking for a saved summary…")
        XCTAssertEqual(
            AgentOverviewState.hint(hasSessionID: true, hasObjective: true, canLoadSummaries: true,
                                    loading: false, missingFromHistory: false),
            "Generate a Cloud summary for more detail.")
        XCTAssertEqual(
            AgentOverviewState.hint(hasSessionID: true, hasObjective: false, canLoadSummaries: true,
                                    loading: false, missingFromHistory: false),
            "Generate a short summary to see what this agent is working on.")
    }

    func testGenerateIsOfferedOnlyForAReachableSession() {
        XCTAssertTrue(AgentOverviewState.offersGenerate(hasSessionID: true, canLoadSummaries: true,
                                                        missingFromHistory: false))
        XCTAssertFalse(AgentOverviewState.offersGenerate(hasSessionID: false, canLoadSummaries: true,
                                                         missingFromHistory: false))
        XCTAssertFalse(AgentOverviewState.offersGenerate(hasSessionID: true, canLoadSummaries: false,
                                                         missingFromHistory: false))
    }
}
