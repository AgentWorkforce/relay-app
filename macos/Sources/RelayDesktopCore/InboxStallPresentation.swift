import Foundation

extension InboxStall {
    public var waitingSummary: String {
        let messages = undrainedCount == 1 ? "message" : "messages"
        return "Stalled · \(undrainedCount) \(messages) waiting · oldest \(max(0, oldestUndrainedAgeMs) / 60_000) min"
    }
}
