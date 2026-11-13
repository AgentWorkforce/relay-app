import SwiftUI
@testable import RelayDesktop
import RelayDesktopCore

/// One Sessions table row driven by live models, as the table drives it.
struct SessionRowHost: View {
    var model: AppModel
    var relay: RelayModel
    let row: LocalSessionRow
    @Binding var selected: Bool
    var open: () -> Void = {}
    var toggleRelay: () -> Void = {}

    var body: some View {
        LocalSessionTableRow(state: SessionRowIndex(model: model, relay: relay).state(for: row),
            columns: model.sessionColumns, selected: selected,
            actions: SessionRowActions(open: open, toggleSelected: { selected.toggle() }, toggleRelay: toggleRelay,
                setHidden: { _ in }, openInCloud: { _ in }))
            .equatable()
            .environment(model)
    }
}
