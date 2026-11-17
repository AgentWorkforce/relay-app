import XCTest
@testable import AgentRelayiPad

final class SessionSummaryTests: XCTestCase {
    private let workspace = "70A31BA9-5E75-4904-B452-9EA67AE376B8"

    func testReadyLongSummaryDecodesWithProvenance() throws {
        let summary = try SessionSummary([
            "summary": ["status": "ready", "kind": "long", "summary": "  Moved the login flow to OAuth.\n",
                        "model": "summariser-1", "builtAt": "2026-11-17T09:00:00.000Z", "state": NSNull()],
            "provenance": ["source": "session_summary", "revision": "r7", "truncated": true,
                           "model": "summariser-1", "generatedAt": "2026-11-17T09:00:00.000Z"],
        ])
        XCTAssertEqual(summary.kind, .long)
        XCTAssertEqual(summary.text, "Moved the login flow to OAuth.")
        XCTAssertEqual(summary.model, "summariser-1")
        XCTAssertNotNil(summary.builtAt)
        XCTAssertEqual(summary.provenance, SessionSummary.Provenance(source: "session_summary", truncated: true))
        XCTAssertEqual(SessionSummaryState(summary), .ready(text: "Moved the login flow to OAuth.", isShort: false))
    }

    func testReadyShortSummaryIsMarkedShortAndProvenanceIsOptional() throws {
        let brief = try SessionSummary([
            "summary": ["status": "ready", "kind": "short", "summary": "Fixed the redirect.",
                        "model": "brief", "builtAt": "2026-11-17T09:00:00Z", "state": NSNull()],
            "provenance": ["source": "session_brief", "revision": NSNull(), "truncated": false],
        ])
        XCTAssertEqual(brief.provenance, SessionSummary.Provenance(source: "session_brief", truncated: false))
        XCTAssertEqual(SessionSummaryState(brief), .ready(text: "Fixed the redirect.", isShort: true))

        let withoutProvenance = try SessionSummary([
            "summary": ["status": "ready", "kind": "short", "summary": "Fixed the redirect.",
                        "model": "brief", "builtAt": "2026-11-17T09:00:00Z", "state": NSNull()],
        ])
        XCTAssertNil(withoutProvenance.provenance)
        let nullProvenance = try SessionSummary([
            "summary": ["status": "ready", "kind": "short", "summary": "Fixed the redirect.", "state": NSNull()],
            "provenance": NSNull(),
        ])
        XCTAssertNil(nullProvenance.provenance)
    }

    func testEmptySummaryShowsNoSummaryYet() throws {
        for kind in ["short", "long"] {
            let summary = try SessionSummary([
                "summary": ["status": "empty", "kind": kind, "state": NSNull()], "provenance": NSNull(),
            ])
            XCTAssertNil(summary.text)
            XCTAssertEqual(SessionSummaryState(summary), .empty)
        }
    }

    func testTheInnerSummaryObjectIsParsedStrictly() {
        let rejected: [JSONObject] = [
            [:],
            ["summary": NSNull()],
            ["summary": ["status": "ready", "kind": "short", "summary": "Text", "extra": 1]],
            ["summary": ["status": "empty", "kind": "short", "summary": "Text"]],
            ["summary": ["status": "ready", "kind": "short", "summary": "   "]],
            ["summary": ["status": "ready", "kind": "medium", "summary": "Text"]],
            ["summary": ["status": "pending", "kind": "short"]],
            ["summary": ["kind": "short", "summary": "Text"]],
        ]
        for response in rejected {
            XCTAssertThrowsError(try SessionSummary(response), "\(response)")
        }
    }

    func testFailuresFallBackToShortHideOrShowUnavailable() {
        for code in ["summary_kind", "summary_fallback_not_configured"] {
            XCTAssertNil(SessionSummaryState.afterFailure(status: 501, code: code, requested: .long))
            XCTAssertEqual(SessionSummaryState.afterFailure(status: 501, code: code, requested: .short), .hidden)
        }
        XCTAssertEqual(SessionSummaryState.afterFailure(status: 501, code: "summary_unqualified", requested: .long), .hidden)
        XCTAssertEqual(SessionSummaryState.afterFailure(status: 501, code: "summary_unqualified", requested: .short), .hidden)
        XCTAssertEqual(SessionSummaryState.afterFailure(status: 404, code: "", requested: .long), .hidden)
        XCTAssertEqual(SessionSummaryState.afterFailure(status: 503, code: "backend_unavailable", requested: .short), .unavailable)
        XCTAssertEqual(SessionSummaryState.afterFailure(status: 500, code: "", requested: .short), .unavailable)
        XCTAssertEqual(SessionSummaryState.afterFailure(status: 400, code: "", requested: .long), .unavailable)
        XCTAssertEqual(SessionSummaryState.afterFailure(status: nil, code: "", requested: .long), .unavailable)
        XCTAssertTrue(SessionSummaryState.retriesAutomatically(status: 503))
        XCTAssertFalse(SessionSummaryState.retriesAutomatically(status: 500))
    }

    func testRequestPathCarriesWorkspaceSourceAndKindOnly() throws {
        XCTAssertEqual(SessionSummaryRequest.pathTemplate, "/sessions/{id}/summary")
        let path = try XCTUnwrap(SessionSummaryRequest.path(
            sessionID: "session/1 a", workspaceID: workspace, source: "claude", kind: .long))
        let url = try XCTUnwrap(URL(string: "https://agentrelay.com/cloud/api/v1" + path))
        let components = try XCTUnwrap(URLComponents(url: url, resolvingAgainstBaseURL: false))
        XCTAssertEqual(components.percentEncodedPath, "/cloud/api/v1/sessions/session%2F1%20a/summary")
        XCTAssertEqual(Dictionary(uniqueKeysWithValues: (components.queryItems ?? []).map { ($0.name, $0.value ?? "") }), [
            "workspace": workspace, "source": "claude", "kind": "long",
        ])
        XCTAssertTrue(try XCTUnwrap(SessionSummaryRequest.path(
            sessionID: "s", workspaceID: workspace, source: "codex", kind: .short)).hasSuffix("source=codex&kind=short"))
        XCTAssertTrue(try XCTUnwrap(SessionSummaryRequest.path(
            sessionID: "s", workspaceID: workspace, source: "gemini", kind: .long)).contains("source=gemini&"))
        let encoded = try XCTUnwrap(SessionSummaryRequest.path(
            sessionID: "s", workspaceID: workspace, source: "a+b c", kind: .short))
        XCTAssertTrue(encoded.contains("source=a%2Bb%20c&"))
        XCTAssertNil(SessionSummaryRequest.path(sessionID: "s", workspaceID: "not-a-workspace", source: "codex", kind: .short))
        XCTAssertNil(SessionSummaryRequest.path(sessionID: "s", workspaceID: workspace, source: "  ", kind: .short))
        XCTAssertNil(SessionSummaryRequest.path(sessionID: "", workspaceID: workspace, source: "codex", kind: .short))
    }

    func testTheMissingSourceSentinelMeansNoSource() throws {
        XCTAssertEqual(SessionSummaryRequest.missingSourceSentinel, "\u{2014}")
        XCTAssertNil(SessionSummaryRequest.usableSource("\u{2014}"))
        XCTAssertNil(SessionSummaryRequest.usableSource(" \u{2014} "))
        XCTAssertNil(SessionSummaryRequest.usableSource(""))
        XCTAssertNil(SessionSummaryRequest.usableSource(nil))
        XCTAssertEqual(SessionSummaryRequest.usableSource("codex"), "codex")
        XCTAssertNil(SessionSummaryRequest.path(sessionID: "s", workspaceID: workspace, source: "\u{2014}", kind: .long))

        let row = try XCTUnwrap(SummarySearchResult([
            "sessionId": "s", "source": "\u{2014}", "summary": "Title",
            "sessionURL": "https://agentrelay.com/sessions/s",
        ]))
        XCTAssertNil(SessionSummaryRequest.usableSource(row.source))
    }

    func testCacheRefetchesOnANewRevisionAfterFiveMinutesOrAfterAFailure() {
        let fetched = Date(timeIntervalSince1970: 1_000_000)
        let entry = SessionSummaryCacheEntry(state: .empty, revision: "r1", fetchedAt: fetched)
        XCTAssertTrue(entry.isFresh(revision: "r1", now: fetched.addingTimeInterval(60)))
        XCTAssertTrue(entry.isFresh(revision: nil, now: fetched.addingTimeInterval(60)))
        XCTAssertFalse(entry.isFresh(revision: "r2", now: fetched.addingTimeInterval(60)))
        XCTAssertFalse(entry.isFresh(revision: "r1", now: fetched.addingTimeInterval(SessionSummaryCacheEntry.maximumAge)))
        let failed = SessionSummaryCacheEntry(state: .unavailable, revision: "r1", fetchedAt: fetched)
        XCTAssertFalse(failed.isFresh(revision: "r1", now: fetched))
    }
}
