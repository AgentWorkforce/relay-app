import AppKit
import SwiftUI
import RelayDesktopCore

/// The same search control lives in the global header across session views.
struct SessionHeaderSearch: View {
    @Environment(AppModel.self) private var model
    var relay: RelayModel
    private var showsFilters: Bool {
        get { model.showsSessionSearch }
        nonmutating set { model.showsSessionSearch = newValue }
    }
    @State private var searchShortcut: Any?
    private var query: LocalSessionQuery { LocalSessionQuery(model.sessionSearch) }
    private func clearSearch() { model.sessionSearch = "" }

    var body: some View {
        @Bindable var model = model
        let query = self.query
        HStack(spacing: 10) {
            Button { showsFilters = true } label: {
                Image(systemName: "magnifyingglass").zoomFrame(width: 20, height: 26)
            }.buttonStyle(.plain).accessibilityLabel("Search and filter sessions")
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    ForEach(Array(query.terms.enumerated()).filter { $0.element.field != nil }, id: \.offset) { index, term in
                        HStack(spacing: 6) {
                            Text(term.displayTitle).lineLimit(1)
                            Button { model.sessionSearch = query.removingTerm(at: index) } label: {
                                Image(systemName: "xmark").zoomFont(size: 9, weight: .semibold)
                                    .zoomFrame(width: 18, height: 20).contentShape(Rectangle())
                            }
                            .buttonStyle(HoverPlainButtonStyle(shape: RoundedRectangle(cornerRadius: 3)))
                            .help("Remove " + term.displayTitle).accessibilityLabel("Remove " + term.displayTitle)
                            .background(GeometryReader { geometry in
                                Color.clear.preference(key: SearchBarRemoveBounds.self,
                                    value: [geometry.frame(in: .named("sessionSearchBar"))])
                            })
                        }.zoomFont(size: 11).zoomPadding(.leading, 8).zoomPadding(.trailing, 4).zoomPadding(.vertical, 3)
                            .background(CloudTheme.muted, in: RoundedRectangle(cornerRadius: 5))
                            .overlay(RoundedRectangle(cornerRadius: 5).strokeBorder(CloudTheme.border))
                    }
                    Button { showsFilters = true } label: {
                        let words = query.terms.filter { $0.field == nil }.map(\.raw).joined(separator: " ")
                        Text(words.isEmpty ? (query.terms.isEmpty ? "Search or filter sessions…" : "Add filters…") : words)
                            .lineLimit(1).zoomPadding(.horizontal, 4).zoomFrame(minHeight: 26)
                    }.buttonStyle(.plain).keyboardShortcut("f", modifiers: .command)
                }
            }.frame(maxWidth: .infinity).zoomFrame(height: 28)
            // Later `@` agents stay as chips but only the first one filters.
            if let note = query.agentNote {
                Text(note).zoomFont(size: 11).lineLimit(1).fixedSize()
                    .accessibilityIdentifier("session-agent-note")
            }
            if !model.sessionSearch.isEmpty {
                Button(action: clearSearch) {
                    Image(systemName: "xmark.circle.fill").zoomFont(size: 15).zoomFrame(width: 24, height: 26)
                }.buttonStyle(HoverPlainButtonStyle(shape: RoundedRectangle(cornerRadius: 5)))
                    .help("clear search").accessibilityLabel("Clear search and filters")
                    .background(GeometryReader { geometry in
                        Color.clear.preference(key: SearchBarRemoveBounds.self,
                            value: [geometry.frame(in: .named("sessionSearchBar"))])
                    })
            }
            Button { showsFilters = true } label: {
                Text("/").zoomFont(size: 11, design: .monospaced)
                    .zoomPadding(.horizontal, 5).zoomPadding(.vertical, 2)
                    .overlay(RoundedRectangle(cornerRadius: 4).strokeBorder(CloudTheme.border))
            }.buttonStyle(.plain).help("Search and filter sessions (/ or ⌘F)")
        }
        .foregroundStyle(CloudTheme.secondary)
        .zoomPadding(.horizontal, 14).zoomPadding(.vertical, 9)
        .zoomFrame(minWidth: 250, maxWidth: .infinity)
        .background(CloudTheme.background, in: RoundedRectangle(cornerRadius: 7))
        .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(showsFilters ? CloudTheme.input : CloudTheme.border))
        .coordinateSpace(name: "sessionSearchBar")
        .backgroundPreferenceValue(SearchBarRemoveBounds.self) { exclusions in
            SearchBarClickRouter(exclusions: exclusions) { showsFilters = true }
        }
        .actionCursor()
        .background {
            AnchoredSearchPanel(presented: $model.showsSessionSearch) {
            let source = model.sessionSearchSource.rows(model: model, relay: relay)
            return SessionSearchPanel(text: $model.sessionSearch,
                harnesses: source.harnesses,
                projects: source.projects,
                agents: relay.roster.filter { $0.kind == "agent" }.map(\.name),
                close: { showsFilters = false })
            }
        }
        .onAppear {
            searchShortcut = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
                guard event.characters == "/", event.modifierFlags.intersection([.command, .control, .option]).isEmpty,
                      !showsFilters, !(NSApp.keyWindow?.firstResponder is NSTextView),
                      !(NSApp.keyWindow?.firstResponder is NSTextField) else { return event }
                showsFilters = true
                return nil
            }
        }
        .onDisappear {
            model.showsSessionSearch = false
            if let searchShortcut { NSEvent.removeMonitor(searchShortcut) }
            searchShortcut = nil
        }
    }
}
