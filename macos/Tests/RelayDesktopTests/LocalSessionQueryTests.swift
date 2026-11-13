import XCTest
@testable import RelayDesktopCore

final class LocalSessionQueryTests: XCTestCase {
    private func row(at: String = "2026-10-11T23:59:59Z", live: Bool = true) -> LocalSessionRow {
        let ms = Int64(ISO8601DateFormatter().date(from: at)!.timeIntervalSince1970 * 1000)
        let upload = UploadSession(cwd: "/work/cloud", included: false, isAgent: false, lastActivityMs: ms,
                                   sessionId: "one", source: "codex", status: .notShared, title: "Fix login")
        let local = LocalSession(live: live, suggestedName: "session", target: Target(id: "one", kind: .codex), title: "Local work")
        return LocalSessionRow.merge(uploads: [upload], sessions: [local])[0]
    }

    /// The registered row the shared `agent_matches` vectors run against.
    private func agentRow() -> LocalSessionRow {
        let registration = SessionRegistration(addedBySession: false, address: "example-agent@desk", alias: "example-reviewer", name: "example-agent")
        return LocalSessionRow.merge(uploads: [], sessions: [
            LocalSession(live: true, registration: registration, suggestedName: "session", target: Target(id: "a1", kind: .codex), title: "Agent work"),
        ])[0]
    }

    func testSharedSearchContract() throws {
        struct Refusal: Decodable { let status: Int; let code: String; let reason: String?; let message: String }
        struct Case: Decodable {
            let query: String; let matches: Bool?; let invalid: Bool?
            let agents: [String]?; let free_text: String?; let agent_note: String?; let too_short: Bool?
            let agent_matches: Bool?; let refusal: Refusal?; let refusal_notice: String?; let refusal_free_text: String?
        }
        let file = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("shared/session-search-cases.json")
        let cases = try JSONDecoder().decode([Case].self, from: Data(contentsOf: file))
        for test in cases {
            let query = LocalSessionQuery(test.query)
            if let agents = test.agents {
                XCTAssertEqual(query.agents, agents, test.query)
                XCTAssertEqual(query.agentFilter?.value, agents.first, test.query)
                XCTAssertEqual(query.extraAgents, Array(agents.dropFirst()), test.query)
            }
            if let free = test.free_text { XCTAssertEqual(query.freeText, free, test.query) }
            XCTAssertEqual(query.agentNote, test.agent_note, test.query)
            XCTAssertEqual(query.agentFilter?.tooShort == true, test.too_short == true, test.query)
            if let expected = test.agent_matches { XCTAssertEqual(query.matches(agentRow()), expected, test.query) }
            if let expected = test.matches { XCTAssertEqual(query.matches(row()), expected, test.query) }
            XCTAssertEqual(query.error != nil, test.invalid == true, test.query)
            if let refusal = test.refusal {
                let answer = query.agentRefusal(status: refusal.status, code: refusal.code, reason: refusal.reason, message: refusal.message)
                XCTAssertEqual(answer?.notice, test.refusal_notice, test.query)
                XCTAssertEqual(answer?.freeText, test.refusal_free_text, test.query)
            }
        }
    }

    private struct ActivityCase: Decodable {
        let name: String; let session: LocalSession; let active: Bool; let nextChangeMs: Int64?
        enum CodingKeys: String, CodingKey { case name, session, active, nextChangeMs = "next_change_ms" }
    }
    private func activityFixture() throws -> (nowMs: Int64, cases: [ActivityCase]) {
        struct Fixture: Decodable { let nowMs: Int64; let cases: [ActivityCase]
            enum CodingKeys: String, CodingKey { case nowMs = "now_ms", cases } }
        let file = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("shared/session-active-cases.json")
        let fixture = try JSONDecoder().decode(Fixture.self, from: Data(contentsOf: file))
        return (fixture.nowMs, fixture.cases)
    }

    /// The Status column's, `is:active`'s and the menu counts' rule, shared
    /// with desktop-core's `session_activity` (GTK) through one fixture.
    func testSharedActivityContract() throws {
        let (now, cases) = try activityFixture()
        for test in cases {
            let session = test.session
            XCTAssertEqual(session.isActive(nowMs: now), test.active, test.name)
            XCTAssertEqual(session.activityChange(afterMs: now), test.nextChangeMs, test.name)
            if let change = test.nextChangeMs {
                XCTAssertNotEqual(session.isActive(nowMs: change), test.active, "flips: \(test.name)")
                XCTAssertEqual(session.isActive(nowMs: change - 1), test.active, "not before: \(test.name)")
            }
            let row = LocalSessionRow.merge(uploads: [], sessions: [session])[0]
            for (text, expected) in [("is:active", test.active), ("is:inactive", !test.active)] {
                var query = LocalSessionQuery(text)
                query.nowMs = now
                XCTAssertEqual(query.matches(row), expected, "\(text): \(test.name)")
            }
        }
    }

    /// Cached results are judged again at the first change, never later than
    /// the recheck, and the revision they are keyed by moves exactly then.
    func testActivityIsCheckedAgainAtTheFirstChange() throws {
        let (now, cases) = try activityFixture()
        let sessions = cases.map(\.session)
        let first = cases.compactMap(\.nextChangeMs).min()!
        XCTAssertEqual(sessions.nextActivityCheck(afterMs: now), min(first, now + LocalSession.activityRecheckMs))
        XCTAssertEqual([LocalSession]().nextActivityCheck(afterMs: now), now + LocalSession.activityRecheckMs)
        XCTAssertEqual(sessions.activityRevision(atMs: now), sessions.activityRevision(atMs: first - 1))
        XCTAssertNotEqual(sessions.activityRevision(atMs: now), sessions.activityRevision(atMs: first))
    }

    /// The menu's kept searches judge activity at the time they count at,
    /// not when they were parsed.
    func testMenuCountsJudgeActivityWhenCounted() throws {
        let (now, cases) = try activityFixture()
        let recent = cases.first { $0.name.hasPrefix("a Devin Cloud session active half a day ago") }!
        let row = LocalSessionRow.merge(uploads: [], sessions: [recent.session])[0]
        let counts = { (at: Int64) in MenuSummaryFilter.counts([row], nowMs: at) { _ in nil }[.localOnly] }
        XCTAssertEqual(counts(now), 1)
        XCTAssertEqual(counts(recent.nextChangeMs!), 0)
        XCTAssertEqual(MenuSummaryFilter.localOnly.count([row], nowMs: recent.nextChangeMs!) { _ in nil }, 0)
    }

    func testAgentTokensFollowTheSharedGrammar() {
        for word in ["@codex", "@Codex-API", "@a.b_c-d", "@x@y", "@layered-*", "@a@b*", "@a**"] {
            XCTAssertNotNil(LocalSessionQuery.agentToken(word), word)
        }
        for word in ["someone@example.com", "@", "@*", "@foo!", "@a@b@c", "@a*b", "\"@a\"", "@a@", "@@a", "-@a", "@**", "@é"] {
            XCTAssertNil(LocalSessionQuery.agentToken(word), word)
        }
        XCTAssertEqual(LocalSessionQuery.agentToken("@Layered-Control@No*"), "layered-control@no*")
        // The whole trailing run of `*` and whitespace is one prefix star.
        XCTAssertEqual(LocalSessionQuery.agentToken("@a**"), "a*")
        XCTAssertEqual(LocalSessionQuery.agentToken("@Abc*\t "), "abc*")
        XCTAssertEqual(LocalSessionQuery.agentValue("abc* "), "abc*")
        XCTAssertEqual(LocalSessionQuery.agentValue("abc* *"), "abc*")
        XCTAssertEqual(LocalSessionQuery.agentValue("abc "), "abc")
        XCTAssertEqual(AgentFilter("abc** "), AgentFilter("abc*"))
        XCTAssertFalse(AgentFilter("abc ").prefix)
        let query = LocalSessionQuery("@B fix @a  @b \"@c\" agent:@A")
        XCTAssertEqual(query.agents, ["b", "a"])
        XCTAssertEqual(query.extraAgents, ["a"])
        XCTAssertEqual(query.agentNote, "One agent at a time")
        XCTAssertEqual(query.freeText, "fix \"@c\"")
        XCTAssertFalse(query.agentOnly)
        XCTAssertTrue(LocalSessionQuery("@codex @b").agentOnly)
        // Further agents stay in the query text.
        XCTAssertEqual(query.removingTerm(at: 0), "fix @a @b \"@c\" agent:@A")
    }

    func testAgentValuesAreCappedAt200Characters() {
        let max = LocalSessionQuery.agentMaxLength
        XCTAssertEqual(max, 200)
        let longest = String(repeating: "a", count: max)
        let over = String(repeating: "a", count: max + 1)
        XCTAssertEqual(LocalSessionQuery.agentToken("@" + longest), longest)
        XCTAssertEqual(LocalSessionQuery.agentToken("@" + String(repeating: "a", count: max - 1) + "*")?.count, max)
        XCTAssertNil(LocalSessionQuery.agentToken("@" + longest + "*"))
        XCTAssertNil(LocalSessionQuery.agentToken("@" + over))
        var query = LocalSessionQuery("@" + over)
        XCTAssertNil(query.agentFilter)
        XCTAssertEqual(query.freeText, "@" + over)
        query = LocalSessionQuery("agent:" + over)
        XCTAssertNil(query.agentFilter)
        XCTAssertNil(query.terms[0].field)
        // A 200-character agent is still a chip.
        XCTAssertEqual(LocalSessionQuery("@" + longest).terms.first?.field, "agent")
    }

    func testReadAPIRefusalsShowANoticeAndNeverFallBackForAnAgentOnlyQuery() {
        let only = LocalSessionQuery("@example*")
        let withText = LocalSessionQuery("@example* fix")
        for reason in ["param_agentMatch", "agent_match_not_migrated", "agent_prefix_not_supported"] {
            let refusal = only.agentRefusal(status: 501, code: "unsupported_by_backend", reason: reason, message: "")
            XCTAssertEqual(refusal, AgentRefusal(notice: LocalSessionQuery.agentPrefixUnavailable, freeText: nil), reason)
            XCTAssertEqual(refusal?.notice, "Prefix search is not available yet", reason)
            XCTAssertEqual(withText.agentRefusal(status: 501, code: "unsupported_by_backend", reason: reason, message: "")?.freeText, "fix", reason)
        }
        for reason in ["agent_with_text", "agent_filter_not_ready", "agent_filter_not_on_search"] {
            let refusal = only.agentRefusal(status: 501, code: "unsupported_by_backend", reason: reason, message: "")
            XCTAssertEqual(refusal, AgentRefusal(notice: LocalSessionQuery.agentFilterUnavailable, freeText: nil), reason)
        }
        XCTAssertEqual(only.agentRefusal(status: 400, code: "invalid_request", reason: nil, message: "agent is too long"),
                       AgentRefusal(notice: "agent is too long", freeText: nil))
        XCTAssertEqual(only.agentRefusal(status: 400, code: "agent_prefix_too_short", reason: nil, message: "")?.notice,
                       LocalSessionQuery.agentPrefixTooShort)
        XCTAssertEqual(only.agentRefusal(status: 400, code: "agent_prefix_too_broad", reason: nil, message: "")?.notice,
                       LocalSessionQuery.agentPrefixTooBroad)
        XCTAssertNil(only.agentRefusal(status: 500, code: "internal", reason: nil, message: "boom"))
        XCTAssertNil(LocalSessionQuery("fix").agentRefusal(status: 501, code: "unsupported_by_backend", reason: "agent_with_text", message: ""))
    }

    func testAgentFiltersAreExactOrALongEnoughPrefix() throws {
        func filter(_ query: String) throws -> AgentFilter { try XCTUnwrap(LocalSessionQuery(query).agentFilter) }
        XCTAssertEqual(try filter("@Codex-API@Desk"), AgentFilter("codex-api@desk"))
        XCTAssertFalse(try filter("@Codex-API@Desk").prefix)
        let prefix = try filter("@layered-*")
        XCTAssertTrue(prefix.prefix && !prefix.tooShort)
        XCTAssertEqual(prefix.stem, "layered-")
        XCTAssertTrue(prefix.matches(["Layered-Control"]))
        XCTAssertFalse(prefix.matches(["control-layered"]))
        XCTAssertTrue(try filter("@co*").tooShort)
        XCTAssertTrue(try filter("@a.-b*").tooShort)
        XCTAssertFalse(try filter("@abc*").tooShort)
        XCTAssertFalse(try filter("@co*").matches(["codex"]))
        XCTAssertFalse(try filter("@codex").matches(["codex-api"]))
        XCTAssertTrue(try filter("@codex-api").matches(["reviewer", "CODEX-API"]))
        // The `@node` suffix is part of the value.
        XCTAssertFalse(try filter("@codex-api@desk").matches(["codex-api"]))
        XCTAssertTrue(try filter("@x").matches(["x"]))
        XCTAssertNil(LocalSessionQuery("fix").agentFilter)
    }

    func testAgentChipsCarryTheAgentField() {
        let query = LocalSessionQuery("fix @Reviewer agent:@codex-api @co*")
        let chips = query.terms.filter { $0.field != nil }.map { [$0.field!, $0.value, $0.displayTitle] }
        XCTAssertEqual(chips, [["agent", "reviewer", "@Reviewer"],
                               ["agent", "codex-api", "agent:@codex-api"],
                               ["agent", "co*", "@co* (too short)"]])
        XCTAssertEqual(query.removingTerm(at: 1), "fix agent:@codex-api @co*")
        XCTAssertNil(query.error)
        XCTAssertNotNil(LocalSessionQuery("agent:").error)
    }

    func testRosterAgentsAreSuggestedByThePrefixAfterAt() {
        let roster = ["reviewer", "Codex-API", "codex-api", "lead", "bad name"]
        XCTAssertEqual(LocalSessionQuery.agentSuggestions("fix @co", roster: roster), ["Codex-API"])
        XCTAssertEqual(LocalSessionQuery.agentSuggestions("@", roster: roster), ["Codex-API", "lead", "reviewer"])
        XCTAssertEqual(LocalSessionQuery.agentSuggestions("fix ", roster: roster), ["Codex-API", "lead", "reviewer"])
        XCTAssertEqual(LocalSessionQuery.agentSuggestions("fix co", roster: roster), [])
        XCTAssertEqual(LocalSessionQuery.agentSuggestions("@zz", roster: roster), [])
        // Choosing one writes `@name`, completing the typed fragment.
        XCTAssertEqual(LocalSessionQuery.suggesting("fix @co", field: "agent", value: "Codex-API"), "fix @codex-api ")
        XCTAssertEqual(LocalSessionQuery.suggesting("fix @", field: "agent", value: "lead"), "fix @lead ")
        XCTAssertEqual(LocalSessionQuery.suggesting("fix ", field: "agent", value: "lead"), "fix @lead ")
        XCTAssertEqual(LocalSessionQuery.suggesting("fix @lead ", field: "agent", value: "lead"), "fix ")
        XCTAssertEqual(LocalSessionQuery.completingSuggestion("fix @rev", field: "agent", value: "reviewer", title: "@reviewer"), "fix @reviewer ")
        XCTAssertEqual(LocalSessionQuery("fix").applyingFilter("agent", to: "@lead"), "fix @lead")
    }

    func testSharedFilterMenuActions() throws {
        struct Action: Decodable { let query: String; let field: String; let value: String; let expected: String }
        let file = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("shared/session-filter-actions.json")
        let actions = try JSONDecoder().decode([Action].self, from: Data(contentsOf: file))
        for action in actions {
            XCTAssertEqual(LocalSessionQuery(action.query).applyingFilter(action.field, to: action.value), action.expected)
        }
    }

    func testDateMidnightBoundariesAndMissingDates() {
        XCTAssertTrue(row(at: "2026-10-11T00:00:00Z").matches("updated:2026-10-11"))
        XCTAssertFalse(row(at: "2026-10-12T00:00:00Z").matches("updated:2026-10-11"))
        let undated = LocalSessionRow.merge(uploads: [], sessions: [LocalSession(live: true, suggestedName: "a", target: Target(id: "a", kind: .codex), title: "Undated")])[0]
        XCTAssertFalse(undated.matches("updated:>=2026-10-01"))
        XCTAssertTrue(undated.matches("is:active upload:none"))
        XCTAssertTrue(row(live: false).matches("is:inactive"))
    }

    func testMenuEditsReplaceOnlyTheirQualifierAndPreserveQuotes() {
        let original = "title: \"Fix login\" -harness:claude harness:codex updated:>=2026-10-01"
        let changed = LocalSessionQuery(original).setting("harness", to: "cursor")
        XCTAssertEqual(changed, "title:\"Fix login\" updated:>=2026-10-01 harness:cursor")
        XCTAssertEqual(LocalSessionQuery(changed).setting("harness", to: nil), "title:\"Fix login\" updated:>=2026-10-01")
        XCTAssertEqual(LocalSessionQuery("title: \"Fix: login\"").value(for: "title"), "Fix: login")
        XCTAssertEqual(LocalSessionQuery("title:\"Fix \\\"login\\\"\"").value(for: "title"), "Fix \"login\"")
    }

    func testLiveUploadStateIsUsedAndAllFiltersIntersect() {
        XCTAssertTrue(LocalSessionQuery("is:active upload:paused project:cloud").matches(row(), uploadState: .paused))
        XCTAssertFalse(LocalSessionQuery("is:active upload:uploaded project:cloud").matches(row(), uploadState: .paused))
        XCTAssertFalse(LocalSessionQuery("is:active harness:claude").matches(row()))
    }
}
