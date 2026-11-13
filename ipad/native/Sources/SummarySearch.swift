import SwiftUI

/// Session-summary search stays development-only until its production rollout
/// and customer-facing behavior are approved.
enum SummarySearchFeature {
    static let infoKey = "RelaySummarySearchEnabled"
    static var isEnabled: Bool { enabled(in: Bundle.main.infoDictionary) }

    static func enabled(in info: [String: Any]?) -> Bool {
        switch info?[infoKey] {
        case let value as Bool: return value
        case let value as NSNumber: return value.boolValue
        case let value as String: return ["yes", "true", "1"].contains(value.lowercased())
        default: return false
        }
    }
}

struct SummarySearchResult: Identifiable, Equatable {
    let sessionID: String
    let source: String
    let title: String
    let agentName: String
    let summarySnippet: String
    let createdAt: Date?
    let lastTimestamp: Date?
    let sessionURL: URL
    let matchedBy: [String]

    var id: String { sessionID }
    var displayTimestamp: Date? { lastTimestamp ?? createdAt }

    init?(_ data: JSONObject) {
        func date(_ value: Any?) -> Date? {
            guard let timestamp = value as? String else { return nil }
            return (try? Date.ISO8601FormatStyle(includingFractionalSeconds: true).parse(timestamp))
                ?? (try? Date.ISO8601FormatStyle().parse(timestamp))
        }
        sessionID = data["sessionId"] as? String ?? ""
        source = data["source"] as? String ?? ""
        let reportedTitle = (data["summary"] as? String ?? "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let reportedName = (data["agentName"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        agentName = reportedName.isEmpty ? (source.isEmpty ? "Agent" : source.capitalized) : reportedName
        let search = data["search"] as? JSONObject ?? [:]
        let reportedSnippet = (search["snippet"] as? String ?? "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        title = reportedTitle.isEmpty
            ? (reportedSnippet.isEmpty ? "Session summary unavailable" : reportedSnippet)
            : reportedTitle
        summarySnippet = reportedSnippet.isEmpty ? title : reportedSnippet
        createdAt = date(data["createdAt"])
        lastTimestamp = date(data["lastTs"])
        let rawURL = data["sessionURL"] as? String ?? ""
        sessionURL = URL(string: rawURL) ?? URL(fileURLWithPath: "/")
        matchedBy = search["matchedBy"] as? [String] ?? []
        guard !sessionID.isEmpty, !source.isEmpty,
              sessionURL.scheme == "https", sessionURL.host != nil else { return nil }
    }

    var agent: RelayAgent {
        RelayAgent(relay: [
            "name": agentName,
            "status": "idle",
            "metadata": ["session_id": sessionID, "cli": source],
        ])
    }
}

struct SummarySearchPage: Equatable {
    let results: [SummarySearchResult]
    let nextCursor: String?

    init(_ response: JSONObject) {
        results = (response["items"] as? [JSONObject] ?? []).compactMap(SummarySearchResult.init)
        let cursor = (response["nextCursor"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        nextCursor = cursor.isEmpty ? nil : cursor
    }
}

enum SummarySearchQuery {
    static let minimumLength = 2
    static let debounceMilliseconds = 350

    static func normalize(_ value: String) -> String {
        value.trimmingCharacters(in: .whitespacesAndNewlines)
            .split(whereSeparator: \.isWhitespace).joined(separator: " ")
    }
}

enum SummarySearchRequest {
    static func path(workspaceID: String, query: String, cursor: String?, limit: Int = 25) -> String? {
        let normalized = SummarySearchQuery.normalize(query)
        guard UUID(uuidString: workspaceID) != nil,
              normalized.count >= SummarySearchQuery.minimumLength,
              normalized.count <= 500, (1...25).contains(limit) else { return nil }
        var queryItems = [
            URLQueryItem(name: "q", value: normalized),
            URLQueryItem(name: "ranking", value: "hybrid"),
            URLQueryItem(name: "limit", value: String(limit)),
        ]
        if let cursor, !cursor.isEmpty { queryItems.append(URLQueryItem(name: "cursor", value: cursor)) }
        var components = URLComponents()
        components.queryItems = queryItems
        // URLQueryItem leaves literal plus signs untouched, while form-style
        // query parsers decode them as spaces. Preserve C++ queries and opaque
        // base64-like cursors explicitly.
        guard let rawQuery = components.percentEncodedQuery else { return nil }
        let encoded = rawQuery.replacingOccurrences(of: "+", with: "%2B")
        return "/workspaces/\(workspaceID)/sessions?\(encoded)"
    }
}

enum SummarySearchPhase: Equatable {
    case prompt
    case loading
    case results
    case empty
    case offline
    case failure(String)
}

struct SessionSummarySearchView<Avatar: View>: View {
    @ObservedObject var client: RelayClient
    let avatar: (RelayAgent) -> Avatar
    let onSelect: (SummarySearchResult) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var query = ""
    @State private var results: [SummarySearchResult] = []
    @State private var phase: SummarySearchPhase = .prompt
    @State private var retryID = 0
    @State private var nextCursor: String?
    @State private var loadingMore = false
    @State private var paginationError: String?
    @State private var resultsQuery = ""
    @State private var paginationTask: Task<Void, Never>?

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                searchField
                Divider()
                content
            }
            .background(Color(.systemGroupedBackground))
            .navigationTitle("Search summaries")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close") { dismiss() }
                }
            }
        }
        .task(id: "\(query)|\(retryID)") { await search() }
        .onChange(of: query) { _, _ in invalidatePagination() }
        .onDisappear { paginationTask?.cancel() }
    }

    private var searchField: some View {
        HStack(spacing: 9) {
            Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
            TextField("Search agent summaries", text: $query)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .submitLabel(.search)
                .accessibilityIdentifier("Summary search field")
            if !query.isEmpty {
                Button { query = "" } label: {
                    Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Clear summary search")
            }
        }
        .padding(11)
        .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 10))
        .padding(.horizontal, 16).padding(.vertical, 12)
        .keyboardDismissToolbar()
    }

    @ViewBuilder private var content: some View {
        switch phase {
        case .results:
            List {
                ForEach(results) { result in
                    Button {
                        onSelect(result)
                        dismiss()
                    } label: {
                        resultRow(result)
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("Summary result \(result.sessionID)")
                    .listRowBackground(Color(.secondarySystemGroupedBackground))
                }
                paginationFooter
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .scrollDismissesKeyboard(.interactively)
        case .loading:
            stateView(icon: nil, title: "Searching summaries…", detail: nil, progress: true)
        case .empty:
            stateView(icon: "magnifyingglass", title: "No matching summaries",
                      detail: "Try a broader task, decision, or outcome.")
        case .offline:
            stateView(icon: "wifi.slash", title: "You're offline",
                      detail: "Reconnect to search agent summaries.", retry: true)
        case .failure(let message):
            stateView(icon: "exclamationmark.triangle", title: "Search unavailable",
                      detail: message, retry: true)
        case .prompt:
            stateView(icon: "sparkles", title: "Search every agent summary",
                      detail: "Describe a task, decision, or outcome. Semantic search also finds related wording.")
        }
    }

    private func resultRow(_ result: SummarySearchResult) -> some View {
        HStack(alignment: .top, spacing: 12) {
            avatar(result.agent)
            VStack(alignment: .leading, spacing: 5) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(result.title).font(.headline).lineLimit(1)
                    Spacer(minLength: 4)
                    if let timestamp = result.displayTimestamp {
                        Text(timestamp, style: .relative)
                            .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    } else {
                        Text("Time unavailable")
                            .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }
                }
                HStack(spacing: 8) {
                    Text(result.agentName).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                    if result.matchedBy.contains("semantic") {
                        Label("Semantic", systemImage: "sparkles")
                            .font(.caption2).foregroundStyle(.secondary)
                    }
                }
                Text(result.summarySnippet)
                    .font(.subheadline).foregroundStyle(.secondary)
                    .multilineTextAlignment(.leading).lineLimit(3)
            }
            Image(systemName: "chevron.right")
                .font(.caption).foregroundStyle(.tertiary).padding(.top, 4)
        }
        .padding(.vertical, 7)
        .contentShape(Rectangle())
    }

    @ViewBuilder private var paginationFooter: some View {
        if loadingMore {
            HStack { Spacer(); ProgressView(); Spacer() }
                .listRowBackground(Color.clear)
        } else if let paginationError {
            VStack(spacing: 6) {
                Text(paginationError).font(.caption).foregroundStyle(.red)
                Button("Try loading more") { beginLoadingMore() }
            }
            .frame(maxWidth: .infinity).listRowBackground(Color.clear)
        } else if nextCursor != nil {
            Button("Load more") { beginLoadingMore() }
                .frame(maxWidth: .infinity).listRowBackground(Color.clear)
                .accessibilityIdentifier("Load more summary results")
        }
    }

    private func stateView(icon: String?, title: String, detail: String?,
                           progress: Bool = false, retry: Bool = false) -> some View {
        VStack(spacing: 12) {
            if progress {
                ProgressView().controlSize(.large).accessibilityLabel(title)
            } else if let icon {
                Image(systemName: icon).font(.system(size: 34)).foregroundStyle(.secondary)
            }
            Text(title).font(.headline)
            if let detail {
                Text(detail).font(.subheadline).foregroundStyle(.secondary)
                    .multilineTextAlignment(.center).frame(maxWidth: 360)
            }
            if retry {
                Button("Try again") {
                    retryID += 1
                }
                .buttonStyle(.bordered)
            }
        }
        .padding(28)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func search() async {
        let normalized = SummarySearchQuery.normalize(query)
        guard normalized.count >= SummarySearchQuery.minimumLength else {
            results = []
            nextCursor = nil
            paginationError = nil
            resultsQuery = ""
            phase = .prompt
            return
        }
        do {
            try await Task.sleep(for: .milliseconds(SummarySearchQuery.debounceMilliseconds))
            guard !Task.isCancelled else { return }
            phase = .loading
            let page = try await client.searchSummaries(normalized)
            guard !Task.isCancelled else { return }
            results = page.results
            nextCursor = page.nextCursor
            paginationError = nil
            resultsQuery = normalized
            phase = page.results.isEmpty ? .empty : .results
        } catch {
            guard !Task.isCancelled else { return }
            results = []
            nextCursor = nil
            resultsQuery = ""
            if let failure = error as? URLError,
               failure.code == .notConnectedToInternet || failure.code == .networkConnectionLost {
                phase = .offline
            } else if error is CancellationError || (error as? URLError)?.code == .cancelled {
                phase = .failure("Search was interrupted. Try again.")
            } else {
                phase = .failure(error.localizedDescription)
            }
        }
    }

    private func invalidatePagination() {
        paginationTask?.cancel()
        paginationTask = nil
        loadingMore = false
        paginationError = nil
        nextCursor = nil
        resultsQuery = ""
        results = []
        phase = SummarySearchQuery.normalize(query).count >= SummarySearchQuery.minimumLength ? .loading : .prompt
    }

    private func beginLoadingMore() {
        guard !loadingMore, let cursor = nextCursor,
              resultsQuery == SummarySearchQuery.normalize(query) else { return }
        loadingMore = true
        paginationError = nil
        paginationTask?.cancel()
        paginationTask = Task { await loadMore(query: resultsQuery, cursor: cursor) }
    }

    private func loadMore(query requestQuery: String, cursor: String) async {
        do {
            let page = try await client.searchSummaries(requestQuery, cursor: cursor)
            guard !Task.isCancelled, requestQuery == resultsQuery,
                  requestQuery == SummarySearchQuery.normalize(query), nextCursor == cursor else { return }
            var seen = Set(results.map(\.id))
            results.append(contentsOf: page.results.filter { seen.insert($0.id).inserted })
            nextCursor = page.nextCursor
            loadingMore = false
        } catch {
            guard !Task.isCancelled, requestQuery == resultsQuery,
                  requestQuery == SummarySearchQuery.normalize(query), nextCursor == cursor else { return }
            loadingMore = false
            if error is CancellationError || (error as? URLError)?.code == .cancelled {
                paginationError = "Loading was interrupted. Try again."
            } else {
                paginationError = error.localizedDescription
            }
        }
    }
}
