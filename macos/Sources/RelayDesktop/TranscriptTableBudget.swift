import RelayDesktopCore

/// Stable allowances assigned as entries arrive, never while SwiftUI renders.
/// GTK shares the same 2048-cell conversation budget across paginated reads.
struct TranscriptTableBudget {
    private var limits: [String: Int] = [:]
    private(set) var remaining = 2048

    func limit(for entryID: String) -> Int { limits[entryID] ?? 0 }

    func allowances(for entries: [LocalTranscriptEntry]) -> [String: Int] {
        var result: [String: Int] = [:]
        for entry in entries { result[entry.id] = limit(for: entry.id) }
        return result
    }

    mutating func observe(_ entries: [LocalTranscriptEntry]) {
        for entry in entries where limits[entry.id] == nil {
            limits[entry.id] = 0
            guard entry.kind == "text", entry.role != "tool_result", !isTranscriptContext(entry) else { continue }
            limits[entry.id] = remaining
            // Most messages have no table. Parse potential tables only once,
            // discard the blocks, and let visible SwiftUI rows render lazily.
            guard entry.text.contains("|") else { continue }
            for case let .table(headers, rows) in ChatMarkdownParser.parse(entry.text, tableCellBudget: remaining) {
                remaining -= headers.count * (rows.count + 1)
            }
        }
    }
}
