import SwiftUI

// The chat's session rows. Each keeps its own state (expanded, on screen),
// so expanding a row or scrolling one in or out re-renders that row only,
// never the whole workspace view.

/// One session event: a prompt or reply as a bubble, a tool call or other
/// activity as a compact row. Long content is cut with "Show more".
struct SessionEventBubble: View {
    let row: SessionEventRow
    let agentName: String
    let accent: Color
    @State private var expanded = false

    var body: some View {
        let preview = SessionTimeline.preview(row.text)
        let shown = expanded ? row.text : preview.text
        switch row.event.role {
        case .person, .agent:
            message(shown: shown, preview: preview)
        case .tool, .activity:
            compact(shown: shown, preview: preview)
        }
    }

    private func message(shown: String, preview: (text: String, isCut: Bool)) -> some View {
        let person = row.event.role == .person
        return HStack(alignment: .bottom) {
            if person { Spacer(minLength: 45) }
            VStack(alignment: .leading, spacing: 5) {
                Text(ChatLinks.render(shown)).font(.system(size: 14))
                    .textSelection(.enabled).tint(accent)
                if preview.isCut || row.event.contentTruncated {
                    showMore(canExpand: preview.isCut)
                }
                Text(person ? "In session" : "\(agentName) · in session")
                    .font(.system(size: 10)).foregroundStyle(.secondary).lineLimit(1)
            }
            .padding(12)
            .background(person ? accent.opacity(0.06) : Color(.secondarySystemGroupedBackground),
                        in: RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color.secondary.opacity(0.15)))
            if !person { Spacer(minLength: 45) }
        }
        .accessibilityIdentifier(person ? "Session prompt" : "Session reply")
    }

    private func compact(shown: String, preview: (text: String, isCut: Bool)) -> some View {
        let tool = row.event.role == .tool
        let collapsedLimit = 240
        let canExpand = preview.isCut || preview.text.count > collapsedLimit
            || preview.text.split(separator: "\n", omittingEmptySubsequences: false).count > 3
        let title = tool ? (row.event.toolName.isEmpty ? "Tool" : row.event.toolName)
            : (row.event.kind.isEmpty ? "Activity" : row.event.kind.replacingOccurrences(of: "_", with: " "))
        return VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Image(systemName: tool ? "wrench.and.screwdriver" : "circle.dotted")
                    .font(.system(size: 11)).foregroundStyle(.secondary)
                Text(title).font(.system(size: 12, weight: .semibold, design: tool ? .monospaced : .default))
                if !row.event.toolStatus.isEmpty {
                    Text(row.event.toolStatus).font(.system(size: 11)).foregroundStyle(.secondary)
                }
                Spacer(minLength: 0)
            }
            if !row.event.filesTouched.isEmpty {
                Text(SessionTimeline.filesSummary(row.event.filesTouched))
                    .font(.system(size: 11, design: .monospaced)).foregroundStyle(.secondary).lineLimit(1)
            }
            if !shown.isEmpty {
                // Collapsed rows are cut here rather than by a line limit, so
                // the text's size does not depend on its wrapped height.
                Text(expanded ? shown : SessionTimeline.collapsed(shown, characters: collapsedLimit))
                    .font(.system(size: 11, design: tool ? .monospaced : .default))
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if canExpand || row.event.contentTruncated {
                showMore(canExpand: canExpand)
            }
        }
        .padding(.horizontal, 10).padding(.vertical, 7)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(.tertiarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 8))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier(tool ? "Session tool row" : "Session activity row")
    }

    /// "Show more" for content cut here, and a note when the session store
    /// itself shortened it (shown once nothing more can be expanded).
    private func showMore(canExpand: Bool) -> some View {
        HStack(spacing: 8) {
            if canExpand {
                Button(expanded ? "Show less" : "Show more") { expanded.toggle() }
                    .font(.system(size: 11, weight: .medium)).buttonStyle(.borderless)
            }
            if row.event.contentTruncated && (expanded || !canExpand) {
                Text("Shortened by the session store").font(.system(size: 10)).foregroundStyle(.secondary)
            }
        }
    }
}

/// The row above the oldest loaded session event. It reads the next older
/// page once it has stayed on screen for half a second (the person scrolled
/// up, or the chat is shorter than the screen; in a longer chat the first
/// scroll to the newest row moves it away before then), again when a page
/// moved the cursor while it stays there, or when tapped. It stops loading
/// on its own when `autoLoads` is false (the store stopped it after an
/// empty page, a failure or repeated restarts); a tap still loads.
struct OlderSessionRow: View {
    /// Changes when a page moves the older cursor.
    let cursorKey: String
    let autoLoads: Bool
    let load: @MainActor () async -> Void
    @State private var visible = false
    @State private var loading = false

    var body: some View {
        Button {
            Task { await run() }
        } label: {
            Label("Load earlier activity", systemImage: "arrow.up.circle")
                .font(.caption).foregroundStyle(.secondary)
        }
        .buttonStyle(.plain).frame(maxWidth: .infinity)
        .accessibilityIdentifier("Load earlier activity")
        .onAppear { if !visible { visible = true } }
        .onDisappear { if visible { visible = false } }
        .task(id: "\(cursorKey)|\(visible)|\(autoLoads)") {
            guard visible, autoLoads else { return }
            try? await Task.sleep(for: .milliseconds(500))
            guard !Task.isCancelled, visible, autoLoads else { return }
            await run()
        }
    }

    /// One load at a time from this row; the client also allows only one
    /// read per session.
    private func run() async {
        guard !loading else { return }
        loading = true
        await load()
        loading = false
    }
}

/// "Working: <tool>" at the end of the chat while the agent looks busy. It
/// only exists while the newest event is recent, and then re-reads the rule
/// every five seconds so it clears by age.
struct SessionWorkingRow: View {
    let events: [SessionEvent]
    /// A fixed start, so a re-render does not restart the schedule.
    private static let start = Date(timeIntervalSinceReferenceDate: 0)

    var body: some View {
        if let newest = events.last?.ts, Date().timeIntervalSince(newest) < SessionWorking.window {
            TimelineView(.periodic(from: Self.start, by: 5)) { context in
                if let state = SessionWorking.state(events: events, now: context.date) {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text(state.label).font(.system(size: 12)).foregroundStyle(.secondary)
                        Spacer()
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityIdentifier("Agent working")
                }
            }
        }
    }
}

/// Whether the end of the open chat is on screen. A reference, so updating
/// it as rows scroll in and out re-renders nothing; it is only read when
/// new session events arrive.
final class ChatScrollPosition {
    var atBottom = true
}
