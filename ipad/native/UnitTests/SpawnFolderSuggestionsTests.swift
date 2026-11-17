import XCTest
@testable import AgentRelayiPad

final class SpawnFolderSuggestionsTests: XCTestCase {
    private func node(_ name: String, folders: [String]) -> RelaySpawnNode {
        RelaySpawnNode(name: name, harnesses: ["claude"], recentFolders: folders)
    }

    private func capability(_ name: String, folders: [String]?) -> JSONObject {
        var configured: JSONObject = ["harness": String(name.dropFirst("spawn:".count)), "machine": "node-a"]
        if let folders { configured["recent_folders"] = folders }
        return ["name": name, "kind": "capacity", "metadata": ["configured": configured, "live": JSONObject()]]
    }

    func testANodeReportsTheRecentFoldersOfItsSpawnCapabilities() {
        let capabilities: [JSONObject] = [
            capability("spawn:claude", folders: ["/Users/sam/relay", "/Users/sam/cloud"]),
            capability("spawn:codex", folders: ["/Users/sam/relay", "/Users/sam/site"]),
            ["name": "release", "kind": "capacity", "metadata": ["recent_folders": ["/Users/sam/ignored"]]],
            ["name": "relay:live-agents:v1", "metadata": ["names": ["planner"]]],
        ]
        XCTAssertEqual(SpawnFolderSuggestions.advertisedFolders(in: capabilities),
                       ["/Users/sam/relay", "/Users/sam/cloud", "/Users/sam/site"])
    }

    func testANodeThatPublishesNoFoldersHasNone() {
        XCTAssertEqual(SpawnFolderSuggestions.advertisedFolders(in: [capability("spawn:claude", folders: nil)]), [])
        XCTAssertEqual(SpawnFolderSuggestions.advertisedFolders(in: []), [])
    }

    func testSuggestionsKeepTheNodesNewestFirstOrderAndNameTheLastFolder() {
        let suggestions = SpawnFolderSuggestions.suggestions(
            for: "node-a", in: [node("node-a", folders: ["/Users/sam/relay-app", "/srv/work/cloud"])])
        XCTAssertEqual(suggestions, [
            SpawnFolderSuggestion(name: "relay-app", path: "/Users/sam/relay-app"),
            SpawnFolderSuggestion(name: "cloud", path: "/srv/work/cloud"),
        ])
    }

    func testDuplicatesAreSuggestedOnceAtTheirNewestPlace() {
        let suggestions = SpawnFolderSuggestions.suggestions(
            for: "node-a",
            in: [node("node-a", folders: ["/Users/sam/relay", "/Users/sam/cloud", "/Users/sam/relay/", " /Users/sam/cloud"])])
        XCTAssertEqual(suggestions.map(\.path), ["/Users/sam/relay", "/Users/sam/cloud"])
    }

    func testFoldersAlreadySavedOnTheNodeAreNotSuggestedAgain() {
        let suggestions = SpawnFolderSuggestions.suggestions(
            for: "node-a", in: [node("node-a", folders: ["/Users/sam/relay", "/Users/sam/cloud"])],
            saved: ["/Users/sam/relay/"])
        XCTAssertEqual(suggestions.map(\.path), ["/Users/sam/cloud"])
    }

    func testSuggestionsStopAtTheLimit() {
        let folders = (1...12).map { "/work/project-\($0)" }
        let suggestions = SpawnFolderSuggestions.suggestions(for: "node-a", in: [node("node-a", folders: folders)])
        XCTAssertEqual(suggestions.count, SpawnFolderSuggestions.limit)
        XCTAssertEqual(suggestions.map(\.path), Array(folders.prefix(SpawnFolderSuggestions.limit)))
    }

    func testOnlyTheChosenNodesFoldersAreSuggested() {
        let nodes = [node("studio-mac", folders: ["/Users/ana/site"]), node("node-a", folders: ["/Users/sam/relay"])]
        XCTAssertEqual(SpawnFolderSuggestions.suggestions(for: "node-a", in: nodes).map(\.path), ["/Users/sam/relay"])
        XCTAssertEqual(SpawnFolderSuggestions.suggestions(for: "missing-node", in: nodes), [])
        XCTAssertEqual(SpawnFolderSuggestions.suggestions(for: "", in: nodes), [])
    }

    func testEmptyAndUnusablePathsAreNotSuggested() {
        XCTAssertEqual(SpawnFolderSuggestions.suggestions(for: "node-a", in: []), [])
        XCTAssertEqual(SpawnFolderSuggestions.suggestions(for: "node-a", in: [node("node-a", folders: [])]), [])
        let unusable = ["", "   ", "/", "~/relay", "relative/path", "/two\nlines"]
        XCTAssertEqual(SpawnFolderSuggestions.suggestions(for: "node-a",
                                                          in: [node("node-a", folders: unusable)]), [])
    }
}
