import RelayDesktopCore

extension AppModel {
    func openSessionFromMenu(_ row: LocalSessionRow) {
        guard !hiddenSessionIDs.contains(row.id) else { return }
        showsSessionSearch = false
        page = .sessions
        selectedLocalSession = row
        showMainWindow()
    }

    /// The session list, searched for `search`.
    func showSessionsFromMenu(search: String) {
        isComposingMessage = false
        showLocalSessions(search: search)
        showMainWindow()
    }

    /// Diagnostics, where the problem behind an orange dot is explained in
    /// full: the failed request when there is one.
    func showDiagnosticsFromMenu() {
        isComposingMessage = false
        if requestFailure != nil {
            openFailedRequest()
        } else {
            selectedRequest = nil
            page = .settings(.diagnostics)
        }
        showMainWindow()
    }

    /// The sessions the session list shows, for counting in the menu.
    func menuSessionRows(relay: RelayModel) -> [LocalSessionRow] {
        LocalSessionRow.merge(uploads: sessions, sessions: relay.sessions)
            .filter { showHiddenSessions || !hiddenSessionIDs.contains($0.id) }
    }
}

/// What the menu panel's counts are made from.
struct MenuSummaryKey: Equatable {
    var rows: UInt64
    var metadata: UInt64
    var relayRows: UInt64
    var relayMetadata: UInt64
    var hidden: Set<String>
    var showHidden: Bool
}

struct MenuSummary: Equatable {
    var counts: [MenuSummaryFilter: Int]

    subscript(_ filter: MenuSummaryFilter) -> Int { counts[filter] ?? 0 }
}
