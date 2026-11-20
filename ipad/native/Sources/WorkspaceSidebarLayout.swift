import Foundation

/// Decides the regular-width workspace layout. Below 700 points the phone
/// layout is used and the stored collapsed choice is ignored.
struct WorkspaceSidebarLayout: Equatable {
    static let collapsedKey = "relay.workspaceSidebarCollapsed"

    /// The phone layout: one navigation stack, no rail or sidebar.
    let isCompact: Bool
    /// The rail, the sidebar and their divider lines are on screen.
    let showsLeftSide: Bool
    let sidebarWidth: CGFloat
    /// The "Toggle sidebar" button is in the detail header.
    let showsToggle: Bool

    init(width: CGFloat, collapsed: Bool) {
        isCompact = width < 700
        showsLeftSide = !isCompact && !collapsed
        sidebarWidth = width < 850 ? 245 : 285
        showsToggle = !isCompact
    }

    static func storedCollapsed(in defaults: UserDefaults = .standard) -> Bool {
        defaults.bool(forKey: collapsedKey)
    }

    /// Flips the stored choice and returns the new value.
    @discardableResult
    static func toggleStored(in defaults: UserDefaults = .standard) -> Bool {
        let collapsed = !storedCollapsed(in: defaults)
        defaults.set(collapsed, forKey: collapsedKey)
        return collapsed
    }
}
