import XCTest
@testable import AgentRelayiPad

final class SummarySearchTests: XCTestCase {
    func testFeatureFlagDefaultsOffAndAcceptsBuildSettingValues() {
        XCTAssertFalse(SummarySearchFeature.enabled(in: nil))
        XCTAssertFalse(SummarySearchFeature.enabled(in: [:]))
        XCTAssertTrue(SummarySearchFeature.enabled(in: [SummarySearchFeature.infoKey: true]))
        XCTAssertTrue(SummarySearchFeature.enabled(in: [SummarySearchFeature.infoKey: "YES"]))
        XCTAssertFalse(SummarySearchFeature.enabled(in: [SummarySearchFeature.infoKey: "NO"]))
    }

    func testQueryNormalizationTrimsAndCollapsesWhitespace() {
        XCTAssertEqual(SummarySearchQuery.normalize("  release\n  checklist  "), "release checklist")
        XCTAssertEqual(SummarySearchQuery.minimumLength, 2)
        XCTAssertGreaterThanOrEqual(SummarySearchQuery.debounceMilliseconds, 300)
    }

    func testResponseDecodesContractFallbacksAndCursorAndDropsInvalidRows() {
        let response: JSONObject = ["items": [
            ["sessionId": "session-1", "source": "codex", "summary": "Search screen",
             "agentName": "builder", "search": ["snippet": "Built the search screen", "matchedBy": ["semantic"]],
             "createdAt": "2026-10-18T17:30:00Z", "lastTs": "2026-10-18T18:30:00.000Z",
             "sessionURL": "https://agentrelay.com/sessions/session-1"],
            ["sessionId": "session-2", "source": "claudecode", "summary": "Semantic review",
             "agentName": NSNull(), "search": ["snippet": NSNull(), "matchedBy": ["keyword"]],
             "createdAt": "2026-10-17T16:15:00Z", "lastTs": NSNull(),
             "sessionURL": "https://agentrelay.com/sessions/session-2"],
            ["sessionId": "session-unknown-time", "source": "codex", "summary": "Unknown time",
             "agentName": "reviewer", "search": ["snippet": NSNull(), "matchedBy": ["semantic"]],
             "createdAt": NSNull(), "lastTs": NSNull(),
             "sessionURL": "https://agentrelay.com/sessions/session-unknown-time"],
            ["sessionId": "session-null-summary", "source": "codex", "summary": NSNull(),
             "agentName": "archivist", "search": ["snippet": "Matched archived deployment notes", "matchedBy": ["semantic"]],
             "createdAt": "2026-10-16T12:00:00Z", "lastTs": NSNull(),
             "sessionURL": "https://agentrelay.com/sessions/session-null-summary"],
            ["sessionId": "session-3", "source": "codex", "summary": "Unsafe link",
             "search": ["snippet": "This row must not open.", "matchedBy": []],
             "createdAt": "2026-10-18T17:30:00Z", "lastTs": "2026-10-18T18:30:00Z",
             "sessionURL": "http://example.com/session-3"],
        ] as [JSONObject], "members": [], "error": false, "ranking": "hybrid",
                                    "now": 1_760_860_800_000]
        var paged = response
        paged["nextCursor"] = "page-2"

        let page = SummarySearchPage(paged)
        let results = page.results
        XCTAssertEqual(results.map(\.sessionID), ["session-1", "session-2", "session-unknown-time", "session-null-summary"])
        XCTAssertEqual(page.nextCursor, "page-2")
        XCTAssertEqual(results[1].summarySnippet, "Semantic review")
        XCTAssertEqual(results[1].agentName, "Claudecode")
        XCTAssertEqual(results[1].displayTimestamp, results[1].createdAt)
        XCTAssertNil(results[2].displayTimestamp)
        XCTAssertEqual(results[0].displayTimestamp, results[0].lastTimestamp)
        XCTAssertEqual(results[0].matchedBy, ["semantic"])
        XCTAssertEqual(results[0].agent.name, "builder")
        XCTAssertEqual(results[0].agent.sessionID, "session-1")
        XCTAssertEqual(results[0].agent.cli, "codex")
        XCTAssertEqual(results[3].title, "Matched archived deployment notes")
        XCTAssertEqual(results[3].summarySnippet, "Matched archived deployment notes")
    }

    func testRequestUsesAgreedPathAndPercentEncodesQueryAndCursor() throws {
        let workspace = "70A31BA9-5E75-4904-B452-9EA67AE376B8"
        let path = try XCTUnwrap(SummarySearchRequest.path(
            workspaceID: workspace, query: "  C++ & semantic  ", cursor: "next/page+2"))
        XCTAssertTrue(path.contains("q=C%2B%2B%20%26%20semantic"))
        XCTAssertTrue(path.contains("cursor=next/page%2B2"))
        let url = try XCTUnwrap(URL(string: "https://agentrelay.com/cloud/api/v1" + path))
        let components = try XCTUnwrap(URLComponents(url: url, resolvingAgainstBaseURL: false))
        XCTAssertEqual(components.path, "/cloud/api/v1/workspaces/\(workspace)/sessions")
        XCTAssertEqual(Dictionary(uniqueKeysWithValues: (components.queryItems ?? []).map { ($0.name, $0.value ?? "") }), [
            "q": "C++ & semantic", "ranking": "hybrid", "limit": "25", "cursor": "next/page+2",
        ])
        XCTAssertNil(SummarySearchRequest.path(workspaceID: "not-a-workspace", query: "search", cursor: nil))
        XCTAssertNil(SummarySearchRequest.path(workspaceID: workspace, query: "x", cursor: nil))
        XCTAssertNil(SummarySearchRequest.path(workspaceID: workspace, query: String(repeating: "x", count: 501), cursor: nil))
    }
}
