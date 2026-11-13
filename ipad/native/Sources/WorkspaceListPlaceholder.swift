/// What an empty agent or conversation list says. Before the first roster
/// answers, an empty list means "not loaded yet", not "nothing here".
enum WorkspaceListPlaceholder {
    static let loading = "Loading…"
    static let empty = "No agents in this view"

    static func text(rosterLoaded: Bool) -> String {
        rosterLoaded ? empty : loading
    }
}
