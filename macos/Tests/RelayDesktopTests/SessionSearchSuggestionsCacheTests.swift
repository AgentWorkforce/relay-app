import XCTest
@testable import RelayDesktop

final class SessionSearchSuggestionsCacheTests: XCTestCase {
    func testLargeCatalogAndParsedQueryAreReusedAcrossEveryRenderedRowAndHighlight() {
        let cache = SessionSearchSuggestions()
        let projects = (0..<2_000).map { "project-\($0)" }
        let now = Date(timeIntervalSince1970: 1_792_000_000)
        let list = cache.list(text: "harness:codex ", harnesses: ["codex"], projects: projects, now: now)
        let navigation = SessionSearchNavigation()
        for (index, id) in list.ids.enumerated() {
            navigation.preserve(id: id, query: "harness:codex ")
            XCTAssertEqual(navigation.index(in: list.ids), index)
            let repeated = cache.list(text: "harness:codex ", harnesses: ["codex"], projects: projects, now: now)
            XCTAssertEqual(repeated.selectedIDs.contains(id), id == "harness:codex")
        }
        XCTAssertEqual(cache.catalogBuildCount, 1, "Rendering/highlighting must not reconstruct the option catalog")
        XCTAssertEqual(cache.queryBuildCount, 1, "Rows share one parsed query and one filtered list")
        for index in 0..<100 {
            _ = cache.list(text: "project-\(index)", harnesses: ["codex"], projects: projects, now: now)
        }
        XCTAssertEqual(cache.catalogBuildCount, 1, "Typing reuses static options and formatted dates")
        XCTAssertEqual(cache.queryBuildCount, 101)
    }

    func testCatalogInvalidatesForSourcesAndUTCDayWithoutChangingQuery() {
        let cache = SessionSearchSuggestions()
        let today = Date(timeIntervalSince1970: 86400 * 20_000 + 86399)
        let before = cache.list(text: "", harnesses: ["codex"], projects: ["cloud"], now: today)
        let after = cache.list(text: "", harnesses: ["claude"], projects: ["desktop"], now: today)
        XCTAssertFalse(after.ids.contains("harness:codex"))
        XCTAssertFalse(after.ids.contains("project:cloud"))
        XCTAssertTrue(after.ids.contains("harness:claude"))
        XCTAssertTrue(after.ids.contains("project:desktop"))
        let tomorrow = cache.list(text: "", harnesses: ["claude"], projects: ["desktop"], now: today.addingTimeInterval(1))
        XCTAssertNotEqual(before.items.last?.value, tomorrow.items.last?.value)
        XCTAssertEqual(cache.catalogBuildCount, 3)
        XCTAssertEqual(cache.queryBuildCount, 3)
    }

    func testRosterAgentsFormAnAgentsSectionFilteredByThePrefixAfterAt() {
        let cache = SessionSearchSuggestions()
        let roster = ["reviewer", "Codex-API", "lead", "bad name"]
        let all = cache.list(text: "", harnesses: ["codex"], projects: ["cloud"], agents: roster)
        let agents = all.items.filter { $0.section == "Agents" }
        XCTAssertEqual(agents.map(\.title), ["@Codex-API", "@lead", "@reviewer"])
        XCTAssertEqual(agents.map(\.field), ["agent", "agent", "agent"])
        // After Projects and before Upload.
        let sections = all.items.map(\.section)
        XCTAssertLessThan(sections.lastIndex(of: "Projects")!, sections.firstIndex(of: "Agents")!)
        XCTAssertLessThan(sections.lastIndex(of: "Agents")!, sections.firstIndex(of: "Upload")!)
        XCTAssertEqual(cache.list(text: "fix @co", harnesses: ["codex"], projects: ["cloud"], agents: roster).ids, ["agent:Codex-API"])
        XCTAssertEqual(cache.list(text: "@", harnesses: ["codex"], projects: ["cloud"], agents: roster).ids,
                       ["agent:Codex-API", "agent:lead", "agent:reviewer"])
        // Other words never offer agents, even when a name contains them.
        XCTAssertTrue(cache.list(text: "rev", harnesses: ["codex"], projects: ["cloud"], agents: roster).items.allSatisfy { $0.field != "agent" })
        let chosen = cache.list(text: "@lead ", harnesses: ["codex"], projects: ["cloud"], agents: roster)
        XCTAssertTrue(chosen.selectedIDs.contains("agent:lead"))
    }

    func testFilteringSelectionAndKeyboardIdentitySurviveExpansion() {
        let cache = SessionSearchSuggestions()
        let navigation = SessionSearchNavigation()
        let narrowed = cache.list(text: "clo", harnesses: ["codex"], projects: ["cloud", "my project"])
        XCTAssertEqual(narrowed.ids, ["project:cloud"])
        navigation.move(1, ids: narrowed.ids, query: "clo")
        let expanded = cache.list(text: "project:cloud ", harnesses: ["codex"], projects: ["cloud", "my project"])
        XCTAssertEqual(expanded.items[navigation.index(in: expanded.ids)].id, "project:cloud")
        XCTAssertEqual(expanded.selectedIDs, ["project:cloud"])
        let quoted = cache.list(text: "project:\"my project\"", harnesses: ["codex"], projects: ["cloud", "my project"])
        XCTAssertEqual(quoted.selectedIDs, ["project:my project"])
        let completed = cache.list(text: "harness:codex", harnesses: ["codex"], projects: ["cloud"])
        XCTAssertGreaterThan(completed.items.count, 1, "A complete token leaves the catalog available for another selection")
    }
}
