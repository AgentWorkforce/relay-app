import SwiftUI
import RelayDesktopCore

@MainActor
@Observable
final class LocalTranscriptModel {
    private(set) var entries: [LocalTranscriptEntry] = [] {
        didSet {
            groups = transcriptGroups(entries)
            tableBudget.observe(entries)
        }
    }
    /// The entries as the transcript shows them: a run of tool calls is one group.
    private(set) var groups: [[LocalTranscriptEntry]] = []
    private(set) var tableBudget = TranscriptTableBudget()
    private(set) var cursor: LocalTranscriptCursor?
    private(set) var loading = false
    private(set) var error: String?
    @ObservationIgnored private let client: CoreClient
    let row: LocalSessionRow
    /// The row's session as last read: a spawned agent's changes as it is
    /// taken over.
    private(set) var session: LocalSession?
    /// Where following continues once the whole transcript is read, and the
    /// spawned agent read from (nil: the session's transcript).
    @ObservationIgnored private var followFrom: LocalTranscriptCursor?
    @ObservationIgnored private var followReady = false
    @ObservationIgnored private var readAgent: String?
    /// Bumped by every read: a follow that a read overtook drops its page.
    @ObservationIgnored private var reads = 0
    /// Keeps `session` as the relay lists it while the transcript shows.
    @ObservationIgnored private var sessionFeed: ModelReaction<LocalSession?>?
    /// Reads a spawned agent's new output every 3 s while it shows: the
    /// core announces no transcript growth.
    @ObservationIgnored private var followTask: Task<Void, Never>?
    @ObservationIgnored private var shown = false

    init(row: LocalSessionRow, client: CoreClient, relay: RelayModel? = nil) {
        self.row = row
        self.client = client
        session = row.relay
        if let relay, let id = row.relay?.id {
            sessionFeed = ModelReaction(read: { relay.sessions.first { $0.id == id } }, apply: { [weak self] listed in
                guard let self, let listed, listed != self.session else { return }
                self.session = listed
                self.followSpawnedAgent()
            })
        }
    }

    /// The transcript came on screen or left it, its window included.
    func setShown(_ shown: Bool) {
        guard self.shown != shown else { return }
        self.shown = shown
        if shown { sessionFeed?.start() } else { sessionFeed?.stop() }
        followSpawnedAgent()
    }

    private func followSpawnedAgent() {
        guard shown, SpawnHandover.of(session) != nil else {
            followTask?.cancel()
            followTask = nil
            return
        }
        guard followTask == nil else { return }
        followTask = Task { [weak self] in
            while !Task.isCancelled {
                do { try await Task.sleep(nanoseconds: 3_000_000_000) } catch { return }
                guard let self else { return }
                await self.follow(self.session)
            }
        }
    }

    /// One page: a spawned agent's own output while Agent Relay runs it
    /// headless, else the session's transcript.
    private func page(_ cursor: LocalTranscriptCursor?, sessionID: String, agent: String?, following: Bool = false) async throws -> LocalTranscriptPage {
        if let agent { return try await client.agentsTranscript(AgentTranscriptParams(cursor: cursor, name: agent)) }
        return try await client.sessionsTranscript(LocalTranscriptParams(cursor: cursor, following: following, sessionId: sessionID, source: row.source))
    }

    func load(more: Bool = false, following: Bool = false) async {
        guard !loading else { return }
        guard let sessionID = row.upload?.sessionId ?? row.relay?.target.id else { return }
        loading = true
        reads += 1
        if !more { cursor = nil }
        error = nil
        followReady = false
        let agent = SpawnHandover.transcriptAgent(session)
        readAgent = agent
        defer { loading = false }
        do {
            var next = more ? cursor : nil
            var seen = Set<LocalTranscriptCursor>()
            var replace = !more
            repeat {
                try Task.checkCancellation()
                let requested = next
                let page = try await page(next, sessionID: sessionID, agent: agent, following: following)
                try Task.checkCancellation()
                if replace {
                    tableBudget = TranscriptTableBudget()
                    entries = page.entries
                    replace = false
                } else {
                    let existing = Set(entries.map(\.id))
                    entries += page.entries.filter { !existing.contains($0.id) }
                }
                cursor = page.nextCursor
                if let cursor, cursor == next || !seen.insert(cursor).inserted {
                    error = "The transcript stopped advancing. Try refreshing this session."
                    return
                }
                if page.nextCursor == nil {
                    followFrom = SpawnHandover.followCursor(requested: requested, page: page)
                    followReady = true
                }
                next = cursor
                await Task.yield()
            } while next != nil
        } catch {
            guard !Task.isCancelled else { return }
            self.error = error.localizedDescription
        }
    }

    /// Reads what a spawned agent wrote since the last read; reads it all
    /// again when what it is read from changed (it was taken over).
    func follow(_ session: LocalSession?) async {
        self.session = session ?? self.session
        guard !loading, error == nil, followReady,
              let sessionID = row.upload?.sessionId ?? row.relay?.target.id else { return }
        let agent = SpawnHandover.transcriptAgent(self.session)
        if agent != readAgent { return await load(following: true) }
        let read = reads
        var next = followFrom
        while let page = try? await page(next, sessionID: sessionID, agent: agent, following: true) {
            // A refresh or take-over read replaced the transcript meanwhile.
            guard read == reads, !loading else { return }
            let existing = Set(entries.map(\.id))
            entries += page.entries.filter { !existing.contains($0.id) }
            guard let more = page.nextCursor, more != next else {
                followFrom = SpawnHandover.followCursor(requested: next, page: page)
                return
            }
            next = more
        }
    }
}

struct LocalTranscriptView: View {
    @Environment(AppModel.self) private var model
    @State private var transcript: LocalTranscriptModel
    var relay: RelayModel
    @Environment(\.windowIsVisible) private var windowIsVisible
    @State private var onScreen = false

    init(row: LocalSessionRow, client: CoreClient, relay: RelayModel) {
        _transcript = State(initialValue: LocalTranscriptModel(row: row, client: client, relay: relay))
        self.relay = relay
    }

    /// The row's session as last read.
    private var session: LocalSession? { transcript.session }

    var body: some View {
        HStack(spacing: 0) {
            VStack(spacing: 0) {
                TranscriptScroll(transcript: transcript)
                if SpawnHandover.joins(session), let registration = session?.registration {
                    Divider().overlay(CloudTheme.border)
                    Text(SpawnHandover.following).zoomFont(.caption).foregroundStyle(CloudTheme.secondary)
                        .zoomFrame(maxWidth: 760, alignment: .leading).zoomPadding(.horizontal, 32).zoomPadding(.top, 8)
                        .frame(maxWidth: .infinity)
                    SpawnComposer(relay: relay, name: registration.name, address: registration.address)
                }
            }
            if model.showsSessionDetails {
                Divider().overlay(CloudTheme.border)
                LocalSessionSidebar(row: transcript.row, relay: relay, loading: transcript.loading,
                                    refresh: { Task { await transcript.load() } })
                    .zoomFrame(width: 264)
            }
        }
        .background(CloudTheme.background)
        .task { await transcript.load() }
        // A spawned agent's transcript is followed live while it is shown.
        .onAppear { onScreen = true; transcript.setShown(windowIsVisible) }
        .onDisappear { onScreen = false; transcript.setShown(false) }
        .onChange(of: windowIsVisible) { transcript.setShown(onScreen && $1) }
    }
}

/// The transcript itself: it re-renders only for the transcript's own changes.
struct TranscriptScroll: View {
    var transcript: LocalTranscriptModel
    @State private var position = TranscriptScrollState()
    @State private var userScrolling = false

    var body: some View {
        ScrollViewReader { proxy in
        VStack(spacing: 0) {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 20) {
                if transcript.groups.isEmpty && transcript.cursor == nil && !transcript.loading && transcript.error == nil {
                    VStack(spacing: 10) {
                        Image(systemName: "text.bubble").zoomFont(size: 28).foregroundStyle(CloudTheme.secondary.opacity(0.7))
                        Text("No local transcript available").zoomFont(.headline)
                        Text("This session’s contents are not available on this machine.").foregroundStyle(CloudTheme.secondary)
                    }.frame(maxWidth: .infinity).zoomPadding(.vertical, 60)
                }
                ForEach(transcript.groups, id: \.first!.id) { entries in
                    TranscriptGroupView(entries: entries, source: transcript.row.source, tableCellBudgets: transcript.tableBudget.allowances(for: entries))
                }
                if let error = transcript.error {
                    VStack(alignment: .leading, spacing: 10) {
                        Label("Couldn’t read this session", systemImage: "exclamationmark.triangle").zoomFont(.headline)
                        Text(error).zoomFont(.caption).foregroundStyle(CloudTheme.secondary).textSelection(.enabled)
                        Button("Try again") { Task { await transcript.load(more: transcript.cursor != nil) } }
                    }.zoomPadding(16).frame(maxWidth: .infinity, alignment: .leading)
                        .background(Color.orange.opacity(0.07), in: RoundedRectangle(cornerRadius: 10))
                }
                if transcript.loading {
                    HStack(spacing: 10) { ProgressView().controlSize(.small); Text("Reading local transcript…").foregroundStyle(CloudTheme.secondary) }
                        .frame(maxWidth: .infinity).zoomPadding(20)
                }
                Color.clear.zoomFrame(height: 24).id("conversation-end")
            }
            .zoomFrame(maxWidth: 760, alignment: .leading)
            .zoomPadding(.horizontal, 32).zoomPadding(.vertical, 30)
            .frame(maxWidth: .infinity)
        }
        // LazyVStack's changing height cannot also drive native automatic
        // anchors: SwiftUI can keep re-aligning and re-measuring forever.
        // Only the deferred request below moves the scroll position.
        .onScrollPhaseChange { _, phase, context in
            if userScrolling || (phase.isScrolling && phase != .animating) {
                updateUserPosition(atLatest: context.geometry.contentSize.height - context.geometry.visibleRect.maxY < 100)
            }
            userScrolling = phase.isScrolling && phase != .animating
        }
        .task(id: TranscriptScrollRequest(lastEntryID: transcript.entries.last?.id,
                                          loading: transcript.loading, followsLatest: position.followsLatest,
                                          userScrolling: userScrolling)) {
            // Coalesce paginated reads and leave the current view transaction
            // before asking LazyVStack to resolve the end anchor. A newer read
            // or a user scrolling back cancels this request.
            guard !transcript.loading, position.followsLatest, !userScrolling else { return }
            await Task.yield()
            guard !Task.isCancelled, position.followsLatest, !userScrolling else { return }
            proxy.scrollTo("conversation-end", anchor: .bottom)
        }
        // Its own row under the conversation, so it never covers message
        // text. It shows once you scroll back, not while new replies follow.
        if position.showsLatestButton {
            Divider().overlay(CloudTheme.border)
            Button {
                position.jumpToLatest()
                proxy.scrollTo("conversation-end", anchor: .bottom)
            } label: { Label("Latest message", systemImage: "arrow.down") }
                .buttonStyle(CloudControlStyle(height: 28))
                .zoomPadding(.vertical, 8)
                .frame(maxWidth: .infinity)
        }
        }
        }
    }

    private func updateUserPosition(atLatest: Bool) {
        guard position.atLatest != atLatest || position.followsLatest != atLatest else { return }
        position.update(atLatest: atLatest, userScrolling: true)
    }
}

private struct TranscriptScrollRequest: Equatable {
    let lastEntryID: String?
    let loading: Bool
    let followsLatest: Bool
    let userScrolling: Bool
}

struct TranscriptScrollState {
    private(set) var followsLatest = true
    private(set) var atLatest = true

    mutating func update(atLatest: Bool, userScrolling: Bool) {
        self.atLatest = atLatest
        if userScrolling { followsLatest = atLatest }
    }

    /// Latest message shows only while the reader has scrolled back, never
    /// while a reply's layout briefly leaves the follower short of the end.
    var showsLatestButton: Bool { !followsLatest }

    mutating func jumpToLatest() { followsLatest = true; atLatest = true }
}

func transcriptGroups(_ entries: [LocalTranscriptEntry]) -> [[LocalTranscriptEntry]] {
    var groups: [[LocalTranscriptEntry]] = []
    var seen = Set<String>()
    for entry in entries where entry.kind != "thinking" && seen.insert(entry.id).inserted {
        if isTranscriptTool(entry), let last = groups.last, let first = last.first, isTranscriptTool(first) {
            groups[groups.count - 1].append(entry)
        } else {
            groups.append([entry])
        }
    }
    return groups
}

func isTranscriptContext(_ entry: LocalTranscriptEntry) -> Bool {
    entry.kind == "context" || ["system", "developer"].contains(entry.role)
        || (entry.role == "user" && entry.kind == "text" && entry.text.hasPrefix("# Environment\nYou have been invoked in the following environment:"))
}

private func isTranscriptTool(_ entry: LocalTranscriptEntry) -> Bool {
    entry.kind == "tool_use" || entry.kind == "tool_result" || entry.role == "tool_result"
}

private struct TranscriptGroupView: View {
    let entries: [LocalTranscriptEntry]
    let source: String
    let tableCellBudgets: [String: Int]
    @State private var expanded = false

    var body: some View {
        if entries.first.map(isTranscriptTool) == true {
            VStack(alignment: .leading, spacing: 8) {
                Button { expanded.toggle() } label: {
                    HStack(spacing: 8) {
                        Image(systemName: expanded ? "chevron.down" : "chevron.right")
                            .zoomFont(size: 9, weight: .semibold)
                        Text("Activity")
                    }.zoomFont(size: 12).zoomPadding(.vertical, 8).contentShape(Rectangle())
                }.buttonStyle(.plain).foregroundStyle(CloudTheme.secondary)
                    .accessibilityValue(expanded ? "Expanded" : "Collapsed")
                if expanded {
                    ForEach(entries, id: \.id) { entry in
                        TranscriptEntryView(entry: entry, source: source, tableCellBudget: tableCellBudgets[entry.id] ?? 0)
                    }
                }
            }
        } else {
            ForEach(entries, id: \.id) { entry in
                TranscriptEntryView(entry: entry, source: source, tableCellBudget: tableCellBudgets[entry.id] ?? 0)
            }
        }
    }
}

private struct TranscriptEntryView: View {
    @Environment(AppModel.self) private var model
    let entry: LocalTranscriptEntry
    let source: String
    let tableCellBudget: Int
    @State private var expanded = false
    @State private var hovering = false
    @FocusState private var copyFocused: Bool
    private var context: Bool { isTranscriptContext(entry) }
    private var activity: Bool { context || entry.kind != "text" || entry.role == "tool_result" }
    private var user: Bool { entry.role == "user" && !activity }
    private var title: String {
        if context { return "System prompt" }
        switch entry.kind {
        case "thinking": return "Thinking"
        case "tool_use": return "Tool call"
        case "tool_result": return "Tool result"
        case "context": return "Session context"
        default: return entry.role == "tool_result" ? "Tool result" : "Session activity"
        }
    }
    private var glyph: String {
        switch entry.kind {
        case "thinking": return "sparkle"
        case "context": return "doc.text"
        default: return "terminal"
        }
    }

    var body: some View {
        if activity {
            VStack(alignment: .leading, spacing: 0) {
                Button { expanded.toggle() } label: {
                    HStack(spacing: 9) {
                        Image(systemName: context ? (expanded ? "chevron.down" : "chevron.right") : glyph)
                        Text(title).fontWeight(.medium)
                        if !expanded && !context {
                            Text(entry.text.replacingOccurrences(of: "\n", with: " "))
                                .lineLimit(1).foregroundStyle(CloudTheme.secondary.opacity(0.7))
                        }
                        Spacer(minLength: 4)
                        if !context { Image(systemName: expanded ? "chevron.down" : "chevron.right").zoomFont(size: 9, weight: .semibold) }
                    }.zoomFont(size: 12).zoomPadding(context ? 4 : 12).contentShape(Rectangle())
                }.buttonStyle(NavButtonStyle(kind: .icon))
                    .help(expanded ? "Collapse \(title.lowercased())" : "Expand \(title.lowercased())")
                    .accessibilityValue(expanded ? "Expanded" : "Collapsed")
                if expanded {
                    Divider().hidden().overlay(CloudTheme.border).zoomPadding(.horizontal, 12)
                    HStack { Spacer(); copyButton }.zoomPadding(.horizontal, 8).zoomPadding(.top, 4)
                    if entry.kind == "thinking" {
                        ChatMarkdownView(entry.text, tableCellBudget: tableCellBudget).textSelection(.enabled).zoomPadding(14)
                    } else {
                        ScrollView(.horizontal) {
                            Text(entry.text.isEmpty ? "No text recorded." : entry.text)
                                .zoomFont(size: 11.5, design: .monospaced).textSelection(.enabled)
                                .fixedSize(horizontal: true, vertical: false)
                        }.zoomPadding(14)
                    }
                    truncation
                }
            }.background(context ? .clear : CloudTheme.card, in: RoundedRectangle(cornerRadius: 10))
                .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(context ? .clear : CloudTheme.border))
        } else if entry.imagePath != nil && entry.text.isEmpty {
            attachment
        } else {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 8) {
                    if user {
                        PersonAvatar(name: model.accountName ?? "You", url: model.accountAvatarURL.flatMap(URL.init(string:)), size: 24)
                    } else { AgentIconView(source: source, size: 16) }
                    Text(user ? model.accountName ?? "You" : source.lowercased() == "claude" ? "Claude Code" : AgentIconView.name(for: source)).zoomFont(size: 12, weight: .semibold)
                        .foregroundStyle(CloudTheme.secondary)
                    Text(Date(timeIntervalSince1970: Double(entry.timestampMs) / 1000), style: .time)
                        .zoomFont(size: 10).foregroundStyle(CloudTheme.secondary.opacity(0.7))
                    Spacer()
                    copyButton.opacity(hovering || copyFocused ? 1 : 0)
                }
                if entry.imagePath != nil { attachment }
                if !entry.text.isEmpty || entry.imagePath == nil {
                    ChatMarkdownView(entry.text.isEmpty ? "No text recorded." : entry.text, conversation: true, tableCellBudget: tableCellBudget).textSelection(.enabled)
                }
                truncation
            }
            .zoomPadding(.horizontal, user ? 20 : 0).zoomPadding(.vertical, user ? 12 : 0)
            .background(user ? CloudTheme.muted.opacity(0.6) : .clear, in: RoundedRectangle(cornerRadius: 16))
            .zoomPadding(.leading, user ? 48 : 0)
            .onHover { hovering = $0 }
        }
    }

    private var attachment: some View {
        VStack(alignment: .leading, spacing: 12) {
            Button { expanded.toggle() } label: {
                HStack(spacing: 8) {
                    Image(systemName: expanded ? "chevron.down" : "chevron.right").zoomFont(size: 9, weight: .semibold)
                    Text("Attached image")
                }.zoomFont(size: 12).zoomPadding(.vertical, 4).contentShape(Rectangle())
            }.buttonStyle(.plain).foregroundStyle(CloudTheme.secondary)
                .accessibilityValue(expanded ? "Expanded" : "Collapsed")
            if expanded {
                if let path = entry.imagePath, let image = NSImage(contentsOfFile: path) {
                    Image(nsImage: image).resizable().aspectRatio(contentMode: .fit)
                        .zoomFrame(maxWidth: 640, maxHeight: 480)
                        .clipShape(RoundedRectangle(cornerRadius: 8)).accessibilityLabel("Attached image")
                } else { Text("Image unavailable").foregroundStyle(CloudTheme.secondary) }
            }
        }
    }

    private var copyButton: some View {
        Button {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(entry.text, forType: .string)
        } label: {
            Image(systemName: "doc.on.doc").zoomFont(size: 10).zoomFrame(width: 26, height: 24)
        }.buttonStyle(NavButtonStyle(kind: .icon)).help("Copy message").accessibilityLabel("Copy message")
            .focused($copyFocused)
    }

    @ViewBuilder private var truncation: some View {
        if entry.truncated {
            Text("Long message shortened. Open the original session for the full output.")
                .zoomFont(.caption).foregroundStyle(CloudTheme.secondary).zoomPadding(8)
        }
    }
}
