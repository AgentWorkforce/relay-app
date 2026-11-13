import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

final class LocalTranscriptTests: XCTestCase {
    @MainActor
    func testCursorlessFollowIsDistinctFromOpenAndRefresh() async {
        let core = FakeCore()
        defer { core.stop() }
        core.on("sessions.transcript") { _ in
            ["entries": [], "next_cursor": NSNull()] as [String: Any]
        }
        let session = LocalSession(live: true,
            registration: SessionRegistration(addedBySession: false, address: "c1@desk-desktop", name: "c1",
                spawn: SessionSpawn(canTakeOver: false, harness: "codex", takenOver: false)),
            suggestedName: "c1", target: Target(id: "thread-9", kind: .codex), title: "Work")
        let model = LocalTranscriptModel(row: LocalSessionRow.merge(uploads: [], sessions: [session])[0], client: CoreClient(socketPath: core.path))
        await model.load()
        XCTAssertEqual(core.lastParams("sessions.transcript")?["following"] as? Bool, false)
        for _ in 0..<3 {
            await model.follow(session)
            XCTAssertEqual(core.lastParams("sessions.transcript")?["following"] as? Bool, true)
            XCTAssertNil(core.lastParams("sessions.transcript")?["cursor"])
        }
        await model.load()
        XCTAssertEqual(core.lastParams("sessions.transcript")?["following"] as? Bool, false)
        XCTAssertEqual(core.callCount("sessions.transcript"), 5)
    }

    @MainActor
    func testOpeningLocalTranscriptPaginatesAndRefreshesWithoutUploading() async throws {
        let core = FakeCore()
        defer { core.stop() }
        core.on("sessions.transcript") { params in
            let more = params["cursor"] is [String: Any]
            return ["entries": [["id": more ? "second" : "first", "role": more ? "assistant" : "user",
                "kind": "text", "text": more ? "## Answer\n```swift\nlet x = 1\n```" : "Fix this locally",
                "timestamp_ms": 1, "truncated": false]],
                "next_cursor": more ? NSNull() : ["id": 1, "ts_ms": 1]] as [String: Any]
        }
        let upload = UploadSession(included: false, isAgent: false, sessionId: "local", source: "codex", status: .notShared, title: "Local work")
        let row = LocalSessionRow.merge(uploads: [upload], sessions: [])[0]
        let model = LocalTranscriptModel(row: row, client: CoreClient(socketPath: core.path))
        await model.load()
        XCTAssertEqual(model.entries.map(\.id), ["first", "second"])
        XCTAssertNil(model.cursor)
        XCTAssertEqual(core.callCount("sessions.transcript"), 2)
        XCTAssertEqual(core.lastParams("sessions.transcript")?["source"] as? String, "codex")
        XCTAssertEqual(core.lastParams("sessions.transcript")?["session_id"] as? String, "local")
        await model.load()
        XCTAssertEqual(model.entries.map(\.id), ["first", "second"])
        XCTAssertEqual(core.callCount("sessions.transcript"), 4)
        XCTAssertFalse(model.loading)
        XCTAssertNil(model.error)
        XCTAssertEqual(core.callCount("uploader.sessions.include"), 0)
        XCTAssertEqual(core.callCount("agents.register"), 0)
    }

    @MainActor
    func testConversationTableBudgetFollowsPagesAndResetsOnRefresh() async {
        let core = FakeCore()
        defer { core.stop() }
        let table = (["|" + Array(repeating: "h", count: 32).joined(separator: "|") + "|",
                      "|" + Array(repeating: "---", count: 32).joined(separator: "|") + "|"]
                     + Array(repeating: "|", count: 16)).joined(separator: "\n")
        let manyTables = Array(repeating: table, count: 4).joined(separator: "\n\n")
        let smallTable = "| a | b |\n| --- | --- |\n| c | d |"
        let upload = UploadSession(included: false, isAgent: false, sessionId: "local", source: "codex", status: .notShared, title: "Tables")
        let model = LocalTranscriptModel(row: LocalSessionRow.merge(uploads: [upload], sessions: [])[0], client: CoreClient(socketPath: core.path))
        for iteration in 0..<2 {
            core.on("sessions.transcript") { params in
                let more = params["cursor"] is [String: Any]
                return ["entries": more
                    ? [["id": "later-\(iteration)", "role": "assistant", "kind": "text", "text": table, "timestamp_ms": 3, "truncated": false]]
                    : [["id": "budget-\(iteration)", "role": "assistant", "kind": "text", "text": manyTables, "timestamp_ms": 1, "truncated": false],
                       ["id": "ordinary-\(iteration)", "role": "assistant", "kind": "text", "text": smallTable, "timestamp_ms": 2, "truncated": false]],
                    "next_cursor": more ? NSNull() : ["id": 2, "ts_ms": 2]] as [String: Any]
            }
            await model.load()
            XCTAssertNil(model.error)
            XCTAssertEqual(model.tableBudget.remaining, 412)
            let cells = model.entries.map { entry in
                ChatMarkdownParser.parse(entry.text, tableCellBudget: model.tableBudget.limit(for: entry.id)).reduce(0) { total, block in
                    if case let .table(headers, rows) = block { return total + headers.count * (rows.count + 1) }
                    return total
                }
            }
            XCTAssertEqual(cells, [1632, 4, 0], "Matches the GTK reader across entries, pages and refresh")
            XCTAssertEqual(ChatMarkdownParser.parse(table, tableCellBudget: model.tableBudget.limit(for: "later-\(iteration)")), [.paragraph(table)])
        }
    }

    func testConversationTableBudgetIgnoresContextToolsAndDuplicateEntries() {
        let table = "| h |\n| --- |\n| value |"
        var budget = TranscriptTableBudget()
        for (role, kind) in [("system", "text"), ("user", "context"), ("assistant", "tool_use"), ("tool_result", "text"), ("assistant", "thinking")] {
            budget.observe([LocalTranscriptEntry(id: role + kind, kind: kind, role: role, text: table, timestampMs: 0, truncated: false)])
        }
        XCTAssertEqual(budget.remaining, 2048)
        let message = LocalTranscriptEntry(id: "answer", kind: "text", role: "assistant", text: table, timestampMs: 1, truncated: false)
        budget.observe([message, message])
        XCTAssertEqual(budget.remaining, 2046)
        budget.observe([message])
        XCTAssertEqual(budget.remaining, 2046)
        var otherConversation = TranscriptTableBudget()
        otherConversation.observe([message])
        XCTAssertEqual(otherConversation.limit(for: "answer"), 2048)
    }

    @MainActor
    func testResetRefusalExplainsPreservedSourceTranscript() async {
        let core = FakeCore()
        defer { core.stop() }
        let reason = "This session was removed from Relay by Reset all sessions. Its original transcript is unchanged; open it in the source app to read it."
        core.on("sessions.transcript") { _ in throw FakeCore.Failure(kind: "unavailable", message: reason) }
        let session = LocalSession(live: false, suggestedName: "local", target: Target(id: "local", kind: .codex), title: "Local work")
        let row = LocalSessionRow.merge(uploads: [], sessions: [session])[0]
        let model = LocalTranscriptModel(row: row, client: CoreClient(socketPath: core.path))
        await model.load()
        XCTAssertEqual(model.error, reason)
        XCTAssertTrue(model.entries.isEmpty)
        XCTAssertFalse(model.loading)
        XCTAssertEqual(core.callCount("uploader.sessions.include"), 0)
    }

    @MainActor
    func testFailedReadCanRetryAndEmptyTranscriptIsNotAnError() async {
        let core = FakeCore()
        defer { core.stop() }
        core.on("sessions.transcript") { _ in throw FakeCore.Failure(kind: "unavailable", message: "Transcript missing") }
        let session = LocalSession(live: false, suggestedName: "local", target: Target(id: "local", kind: .codex), title: "Local work")
        let row = LocalSessionRow.merge(uploads: [], sessions: [session])[0]
        let model = LocalTranscriptModel(row: row, client: CoreClient(socketPath: core.path))
        await model.load()
        XCTAssertNotNil(model.error)
        XCTAssertFalse(model.loading)
        core.on("sessions.transcript") { _ in ["entries": [], "next_cursor": NSNull()] as [String: Any] }
        await model.load()
        XCTAssertNil(model.error)
        XCTAssertTrue(model.entries.isEmpty)
    }

    @MainActor
    func testAutomaticPagingPreservesPartialReadAndRetriesFailedPage() async {
        let core = FakeCore()
        defer { core.stop() }
        core.on("sessions.transcript") { params in
            if params["cursor"] is [String: Any] {
                throw FakeCore.Failure(kind: "unavailable", message: "Read interrupted")
            }
            return ["entries": [["id": "first", "role": "user", "kind": "text", "text": "Hello", "timestamp_ms": 1, "truncated": false]],
                    "next_cursor": ["id": 1, "ts_ms": 1]] as [String: Any]
        }
        let session = LocalSession(live: false, suggestedName: "local", target: Target(id: "local", kind: .codex), title: "Local work")
        let model = LocalTranscriptModel(row: LocalSessionRow.merge(uploads: [], sessions: [session])[0], client: CoreClient(socketPath: core.path))
        await model.load()
        XCTAssertEqual(core.callCount("sessions.transcript"), 2)
        XCTAssertEqual(model.entries.map(\.id), ["first"])
        XCTAssertNotNil(model.error)
        XCTAssertNotNil(model.cursor)
        core.on("sessions.transcript") { params in
            XCTAssertNotNil(params["cursor"] as? [String: Any])
            return ["entries": [["id": "second", "role": "assistant", "kind": "text", "text": "Hi", "timestamp_ms": 2, "truncated": false]], "next_cursor": NSNull()] as [String: Any]
        }
        await model.load(more: true)
        XCTAssertEqual(model.entries.map(\.id), ["first", "second"])
        XCTAssertNil(model.error)
        XCTAssertNil(model.cursor)
    }

    @MainActor
    func testRepeatedCursorStopsAutomaticPaging() async {
        let core = FakeCore()
        defer { core.stop() }
        core.on("sessions.transcript") { _ in
            ["entries": [], "next_cursor": ["id": 1, "ts_ms": 1]] as [String: Any]
        }
        let session = LocalSession(live: false, suggestedName: "local", target: Target(id: "local", kind: .codex), title: "Local work")
        let model = LocalTranscriptModel(row: LocalSessionRow.merge(uploads: [], sessions: [session])[0], client: CoreClient(socketPath: core.path))
        await model.load()
        XCTAssertEqual(core.callCount("sessions.transcript"), 2)
        XCTAssertEqual(model.error, "The transcript stopped advancing. Try refreshing this session.")
        XCTAssertFalse(model.loading)
    }

    @MainActor
    func testImageCopyDecodesWithTheTranscriptAndLoadsWithoutTheOriginal() throws {
        let bytes = Data(base64Encoded: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")!
        let copy = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString + ".png")
        try bytes.write(to: copy)
        defer { try? FileManager.default.removeItem(at: copy) }
        let wire: [String: Any] = ["id": "image", "role": "user", "kind": "text", "text": "", "image_path": copy.path, "timestamp_ms": 1, "truncated": false]
        let entry = try JSONDecoder().decode(LocalTranscriptEntry.self, from: JSONSerialization.data(withJSONObject: wire))
        XCTAssertEqual(entry.imagePath, copy.path)
        XCTAssertEqual(NSImage(contentsOfFile: entry.imagePath!)?.size, NSSize(width: 1, height: 1))
    }

    func testToolRunsKeepResultsAndStopAtConversationBoundaries() throws {
        func entry(_ id: String, _ kind: String, role: String = "assistant") throws -> LocalTranscriptEntry {
            try JSONDecoder().decode(LocalTranscriptEntry.self, from: JSONSerialization.data(withJSONObject:
                ["id": id, "kind": kind, "role": role, "text": id, "timestamp_ms": 1, "truncated": false]))
        }
        let firstPage = try [entry("a", "tool_use"), entry("b", "tool_result"), entry("c", "tool_use")]
        let secondPage = try [entry("d", "text", role: "tool_result"), entry("e", "text"),
                              entry("f", "tool_use"), entry("g", "thinking"), entry("h", "tool_use")]
        XCTAssertEqual(transcriptGroups(firstPage).map { $0.map(\.id) }, [["a", "b", "c"]])
        XCTAssertEqual(transcriptGroups(firstPage + secondPage).map { $0.map(\.id) },
                       [["a", "b", "c", "d"], ["e"], ["f", "h"]])
        XCTAssertTrue(transcriptGroups([]).isEmpty)
    }

    func testContextClassificationAndRepeatedCaptureIDsPreserveDistinctTurns() throws {
        func entry(_ id: String, _ kind: String, role: String) throws -> LocalTranscriptEntry {
            try JSONDecoder().decode(LocalTranscriptEntry.self, from: JSONSerialization.data(withJSONObject:
                ["id": id, "kind": kind, "role": role, "text": "Same text", "timestamp_ms": 1, "truncated": false]))
        }
        let first = try entry("user", "text", role: "user")
        let repeated = try entry("another-user-message", "text", role: "user")
        let context = try entry("context", "context", role: "user")
        let system = try entry("system", "text", role: "system")
        XCTAssertEqual(transcriptGroups([context, system, first, first, repeated]).flatMap { $0.map(\.id) }, ["context", "system", "user", "another-user-message"])
        XCTAssertTrue(isTranscriptContext(context))
        XCTAssertTrue(isTranscriptContext(system))
        XCTAssertFalse(isTranscriptContext(first))
    }

    func testLatestScrollFollowsRepliesAndLayoutButNeverPullsHistoryReadersAway() {
        var state = TranscriptScrollState()
        state.update(atLatest: false, userScrolling: false)
        XCTAssertTrue(state.followsLatest, "A new reply changing layout must keep following")
        XCTAssertFalse(state.showsLatestButton, "Latest message must not flash in while following")
        state.update(atLatest: true, userScrolling: false)
        state.update(atLatest: false, userScrolling: true)
        XCTAssertFalse(state.atLatest)
        XCTAssertFalse(state.followsLatest)
        state.update(atLatest: false, userScrolling: false)
        XCTAssertFalse(state.atLatest)
        XCTAssertFalse(state.followsLatest, "New replies must leave scrollback in place")
        XCTAssertTrue(state.showsLatestButton)
        state.jumpToLatest()
        XCTAssertFalse(state.showsLatestButton)
        XCTAssertTrue(state.followsLatest)
        XCTAssertTrue(state.atLatest)
    }

    func testMarkdownKeepsCodeAndConversationStructure() {
        XCTAssertEqual(ChatMarkdownParser.parse("# Heading\n\n**Answer**\n\n```swift\nlet x = 1\n```\n\n> Note\n- Next"), [
            .heading(1, "Heading"), .paragraph("**Answer**"), .code("let x = 1"), .quote("Note"), .unorderedItem("Next")
        ])
        XCTAssertEqual(ChatMarkdownParser.parse("| Relayfile | Messaging |\n| --- | :---: |\n| Shared files | Interruptions |\n| `a\\|b` | **Ready** |"), [
            .table(["Relayfile", "Messaging"], [["Shared files", "Interruptions"], ["`a|b`", "**Ready**"]])
        ])
    }
    func testOversizedMarkdownTablesRemainTextWithoutAllocatingGridCells() {
        for (columns, rows) in [(33, 1), (2, 201), (999, 29000), (32, 64)] {
            let header = "|" + Array(repeating: "h", count: columns).joined(separator: "|") + "|"
            let separator = "|" + Array(repeating: "---", count: columns).joined(separator: "|") + "|"
            let markdown = ([header, separator] + Array(repeating: "|", count: rows)).joined(separator: "\n")
            XCTAssertEqual(ChatMarkdownParser.parse(markdown), [.paragraph(markdown)])
        }
        let header = "|" + Array(repeating: "h", count: 32).joined(separator: "|") + "|"
        let separator = "|" + Array(repeating: "---", count: 32).joined(separator: "|") + "|"
        let markdown = ([header, separator] + Array(repeating: "|", count: 63)).joined(separator: "\n")
        guard case let .table(headers, rows)? = ChatMarkdownParser.parse(markdown).first else {
            return XCTFail("Tables within the limit keep their columns")
        }
        XCTAssertEqual(headers.count, 32)
        XCTAssertEqual(rows.count, 63)
    }

    func testAssistantAndToolSetupQuotationsKeepTheirRoles() {
        let setup = "# Environment\nYou have been invoked in the following environment:"
        for (role, kind, expected) in [
            ("user", "text", true), ("assistant", "text", false),
            ("user", "tool_result", false), ("tool_result", "text", false),
            ("system", "text", true), ("developer", "text", true)
        ] {
            let entry = LocalTranscriptEntry(id: "setup", kind: kind, role: role, text: setup, timestampMs: 0, truncated: false)
            XCTAssertEqual(isTranscriptContext(entry), expected, "\(role) \(kind)")
        }
    }

    func testManySmallTablesShareOneMessageCellBudget() {
        let header = "|" + Array(repeating: "h", count: 32).joined(separator: "|") + "|"
        let separator = "|" + Array(repeating: "---", count: 32).joined(separator: "|") + "|"
        let table = ([header, separator] + Array(repeating: "|", count: 16)).joined(separator: "\n")
        let blocks = ChatMarkdownParser.parse(Array(repeating: table, count: 4).joined(separator: "\n\n"))
        var cells = 0
        var tables = 0
        for block in blocks {
            if case let .table(headers, rows) = block {
                cells += headers.count * (rows.count + 1)
                tables += 1
            }
        }
        XCTAssertEqual(tables, 3)
        XCTAssertEqual(cells, 1632)
        XCTAssertLessThanOrEqual(cells, 2048)
        XCTAssertEqual(blocks.last, .paragraph(table), "The remaining table stays readable as text")
    }

}
