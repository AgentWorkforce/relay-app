import XCTest
@testable import RelayDesktopCore

final class MenuSummaryTests: XCTestCase {
    private func rows() -> [LocalSessionRow] {
        var uploads: [UploadSession] = []
        for index in 0..<6 {
            let uploaded = index < 3
            let activity: Int64 = 1_700_000_000_000 + Int64(index)
            let status: UploadShareStatus = uploaded ? .uploaded : .notShared
            uploads.append(UploadSession(cwd: "/work/relay", included: uploaded, isAgent: false, lastActivityMs: activity,
                                         sessionId: "upload-\(index)", source: index % 2 == 0 ? "codex" : "claude",
                                         status: status, title: "Session \(index)"))
        }
        var sessions: [LocalSession] = []
        for (id, live, onRelay) in [("a", true, true), ("b", true, false), ("c", false, true)] {
            let registration = onRelay ? SessionRegistration(addedBySession: false, address: "\(id)@mac", name: id) : nil
            sessions.append(LocalSession(live: live, registration: registration, suggestedName: id,
                                         target: Target(id: id, kind: .codex), title: id))
        }
        return LocalSessionRow.merge(uploads: uploads, sessions: sessions)
    }

    func testOnePassCountsMatchEachFilter() {
        let rows = rows()
        let state = { (row: LocalSessionRow) in row.upload.map { SessionUploadState(session: $0, status: nil) } }
        let counts = MenuSummaryFilter.counts(rows, uploadState: state)
        XCTAssertEqual(Set(counts.keys), Set(MenuSummaryFilter.allCases))
        for filter in MenuSummaryFilter.allCases {
            XCTAssertEqual(counts[filter], filter.count(rows, uploadState: state), filter.title)
        }
        XCTAssertEqual(counts[.receiving], 1)
        XCTAssertEqual(counts[.localOnly], 1)
        XCTAssertEqual(counts[.uploaded], 3)
        XCTAssertEqual(counts[.notUploaded], rows.count - 3)
    }

    func testOnePassReadsEachRowsUploadStateOnce() {
        let rows = rows()
        var reads = 0
        _ = MenuSummaryFilter.counts(rows) { row in
            reads += 1
            return row.upload.map { SessionUploadState(session: $0, status: nil) }
        }
        XCTAssertEqual(reads, rows.count)
    }
}
