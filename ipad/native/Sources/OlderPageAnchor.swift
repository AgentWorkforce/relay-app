import Foundation

/// Where an open chat stays while an older session page loads: on the row
/// that was first before the page, so the rows that land above it do not
/// push the reader's place down the screen.
///
/// It is recorded when the load starts, from the store as it is then (the
/// oldest event), and used once the page's rows are in the chat: when the
/// store's oldest event changes. A load that adds nothing, another chat, or
/// a page whose rows no longer hold anything from before drops it, so a
/// later poll or scroll never jumps to it.
struct OlderPageAnchor: Equatable {
    let sessionID: String
    /// The oldest event when the load started.
    let oldestID: String
    let oldestAt: Date?

    /// Nil when the store holds no events: there is no row to keep.
    init?(sessionID: String, oldest: SessionEvent?) {
        guard let oldest else { return nil }
        self.sessionID = sessionID
        oldestID = oldest.id
        oldestAt = oldest.ts
    }

    enum Step: Equatable {
        /// The page has not landed: keep the anchor.
        case wait
        /// Scroll this row to the top, and drop the anchor.
        case scroll(String)
        /// Drop the anchor without scrolling.
        case drop
    }

    /// What to do now that the chat of `sessionID` has `oldestID` as its
    /// oldest event and shows `items`.
    func step(sessionID: String, oldestID: String?, items: [ChatTimelineItem]) -> Step {
        guard sessionID == self.sessionID, let oldestID else { return .drop }
        guard oldestID != self.oldestID else { return .wait }
        guard let kept = items.first(where: existedBefore) else { return .drop }
        return .scroll(kept.id)
    }

    /// Whether the chat showed `item` before the page: a relay message (a
    /// page brings only events), an event no older than the old oldest
    /// one, or a group holding one.
    private func existedBefore(_ item: ChatTimelineItem) -> Bool {
        switch item {
        case .message: true
        case .event(let row): existedBefore(row.event)
        case .activity(let group): group.rows.contains { existedBefore($0.event) }
        }
    }

    private func existedBefore(_ event: SessionEvent) -> Bool {
        if event.id == oldestID { return true }
        guard let ts = event.ts, let oldestAt else { return false }
        return ts > oldestAt
    }
}
