import Foundation

/// How many times the request layer sends one request on its own. Only a GET
/// is repeated automatically: a POST that failed may still have been applied
/// by the server (a lost response, a 5xx after the write), so sending it again
/// would deliver it twice. A failed send is retried only when the person taps.
enum RelayRequestRetry {
    static func automaticAttempts(method: String) -> Int {
        method == "GET" ? 3 : 1
    }
}

/// One request to Relaycast or Cloud, as the request layer sends it. The
/// layer's one credential-refresh resend changes only the token, so it is
/// the same request to the server: same body, same Idempotency-Key.
struct RelayRequestSpec {
    let url: URL
    var token: String?
    var method: String = "GET"
    var body: JSONObject?
    var idempotencyKey: String?

    /// This request again with another credential.
    func resent(with token: String) -> RelayRequestSpec {
        var request = self
        request.token = token
        return request
    }

    func urlRequest() throws -> URLRequest {
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = 25
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if let token { request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization") }
        if let idempotencyKey { request.setValue(idempotencyKey, forHTTPHeaderField: "Idempotency-Key") }
        if let body {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONSerialization.data(withJSONObject: body)
        }
        return request
    }
}

/// How `RelayClient.send` turns one POST into an outcome: it returns as soon
/// as Relaycast answers, and what follows an acceptance (refresh, history,
/// delivery checks) runs afterwards, so a slow reload cannot hold the
/// composer's bubble in "Sending…".
enum RelaySendFlow {
    @MainActor
    static func run(post: @MainActor () async throws -> Any,
                    onFailure: @MainActor (Error) -> Void,
                    afterAcceptance: @escaping @MainActor () async -> Void) async -> RelaySendOutcome {
        let response: Any
        do {
            response = try await post()
        } catch is CancellationError {
            return .cancelled
        } catch {
            onFailure(error)
            return .failed
        }
        Task { @MainActor in await afterAcceptance() }
        return .accepted(messageID: OutgoingMessages.messageID(fromSendResponse: response))
    }
}

/// What one POST of a composer send came back with.
enum RelaySendOutcome: Equatable {
    /// Relaycast accepted the message; its id when the response carried one.
    case accepted(messageID: String?)
    /// The POST failed or could not be confirmed. It may still have been applied.
    case failed
    /// The session ended before the send finished.
    case cancelled
}

/// A message the composer sent, shown from the tap until its history row
/// appears. Its `id` is also the request's Idempotency-Key, so a deliberate
/// retry of the same send is the same request to Relaycast.
struct OutgoingMessage: Identifiable, Equatable {
    enum State: Equatable {
        /// The POST has not answered yet.
        case sending
        /// Relaycast accepted it; the bubble stays until history shows the row.
        case sent
        /// The POST failed or did not answer in time. Only a tap retries it.
        case failed
    }

    let id: String
    let agentID: String
    let text: String
    let attachments: [RelayAttachment]
    let createdAt: Date
    /// History rows already shown when the send began; none of them is this
    /// send. Nil when the conversation's history had not loaded yet: then
    /// nothing is known to predate the send, so it is matched only by the id
    /// Relaycast returned, never by its content.
    let earlierRowIDs: Set<String>?
    var state: State = .sending
    /// The message id Relaycast returned when it accepted the send.
    var messageID: String?
    /// When Relaycast accepted it.
    var acceptedAt: Date?
    /// Counts the POSTs of this send, so a late answer to an older POST
    /// cannot fail a newer one.
    var attempt = 1
}

extension OutgoingMessage {
    /// Accepted, and `sentRetention` has passed since.
    func sentRetentionPassed(now: Date) -> Bool {
        guard state == .sent, let acceptedAt else { return false }
        return now.timeIntervalSince(acceptedAt) >= OutgoingMessages.sentRetention
    }
}

/// A history row as the composer reconciles its outgoing messages with it.
struct OutgoingHistoryRow: Equatable {
    let id: String
    let text: String
    let attachmentIDs: [String]
    let mine: Bool
    let createdAt: Date?
}

/// What a tap on a failed bubble does.
enum OutgoingRetry: Equatable {
    /// POST again, with the same Idempotency-Key.
    case post(OutgoingMessage)
    /// History already shows the message, so the first POST was applied and
    /// nothing is sent again.
    case delivered
    /// The bubble is not failed (already retrying, sent or gone); nothing is sent.
    case ignored
}

/// The composer's outgoing messages. Every send is POSTed once per tap: a
/// failure is shown, never re-sent on its own; a send that did not answer
/// within `confirmationTimeout` is shown as failed; a bubble leaves as soon as
/// history shows its row, matched by the returned id or, for a send whose
/// answer was lost, by its content.
struct OutgoingMessages {
    /// How long a bubble may say "Sending…" before it is shown as failed.
    static let confirmationTimeout: Duration = .seconds(60)
    /// How far before the tap the server's clock may date the row.
    static let clockSkew: TimeInterval = 120
    /// How long an accepted bubble may say "Sent" when history never shows
    /// its row (it fell outside the loaded window, or history stopped loading).
    static let sentRetention: TimeInterval = 60

    private(set) var all: [OutgoingMessage] = []

    func messages(for agentID: String) -> [OutgoingMessage] {
        all.filter { $0.agentID == agentID }
    }

    /// Starts a send. `history` is the conversation's loaded history, or nil
    /// when it has not loaded yet.
    @discardableResult
    mutating func begin(agentID: String, text: String, attachments: [RelayAttachment],
                        history: [OutgoingHistoryRow]?, now: Date = Date(),
                        id: String = UUID().uuidString) -> OutgoingMessage {
        let message = OutgoingMessage(id: id, agentID: agentID, text: text, attachments: attachments,
                                      createdAt: now, earlierRowIDs: history.map { Set($0.map(\.id)) })
        all.append(message)
        return message
    }

    /// Records the answer to one POST of a send. An acceptance always counts,
    /// even from an older attempt; a failure counts only for the current
    /// attempt of a send that is still sending.
    mutating func finish(_ id: String, attempt: Int, outcome: RelaySendOutcome, now: Date = Date()) {
        guard let index = all.firstIndex(where: { $0.id == id }) else { return }
        switch outcome {
        case .accepted(let messageID):
            all[index].state = .sent
            if all[index].acceptedAt == nil { all[index].acceptedAt = now }
            if let messageID, !messageID.isEmpty { all[index].messageID = messageID }
        case .failed, .cancelled:
            guard all[index].state == .sending, all[index].attempt == attempt else { return }
            all[index].state = .failed
        }
    }

    /// Shows a send that is still unanswered after `confirmationTimeout` as failed.
    mutating func expire(_ id: String, attempt: Int) {
        finish(id, attempt: attempt, outcome: .failed)
    }

    /// Removes an accepted bubble once `sentRetention` has passed since
    /// Relaycast accepted it, whether or not history showed its row.
    mutating func settle(_ id: String, now: Date = Date()) {
        all.removeAll { $0.id == id && $0.sentRetentionPassed(now: now) }
    }

    /// A tap on a failed bubble. History is checked first: a send whose POST
    /// failed after Relaycast applied it is already there and is not sent again.
    mutating func retry(_ id: String, history: [OutgoingHistoryRow], now: Date = Date()) -> OutgoingRetry {
        guard let message = all.first(where: { $0.id == id }), message.state == .failed else { return .ignored }
        reconcile(agentID: message.agentID, with: history, now: now)
        guard let index = all.firstIndex(where: { $0.id == id }) else { return .delivered }
        all[index].state = .sending
        all[index].attempt += 1
        return .post(all[index])
    }

    mutating func discard(_ id: String) {
        all.removeAll { $0.id == id }
    }

    mutating func removeAll() {
        all = []
    }

    /// Removes every bubble of `agentID` whose message `history` now shows.
    /// A send with a returned id matches only that row. One without (still
    /// sending, failed, or accepted without an id) matches a new, dated row of
    /// mine with the same text and attachments, each row claimed by one send,
    /// and only when history had loaded when the send began. An accepted
    /// bubble also leaves once `sentRetention` has passed or history shows a
    /// row dated clearly after the send.
    mutating func reconcile(agentID: String, with history: [OutgoingHistoryRow], now: Date = Date()) {
        let historyIDs = Set(history.map(\.id))
        var claimed = Set(all.compactMap(\.messageID))
        var kept: [OutgoingMessage] = []
        for message in all {
            guard message.agentID == agentID else { kept.append(message); continue }
            if message.state == .sent {
                let after = message.createdAt.addingTimeInterval(Self.clockSkew)
                if message.sentRetentionPassed(now: now) ||
                    history.contains(where: { newer in newer.createdAt.map { $0 > after } ?? false }) { continue }
            }
            if let messageID = message.messageID {
                if !historyIDs.contains(messageID) { kept.append(message) }
                continue
            }
            guard let earlierRowIDs = message.earlierRowIDs else { kept.append(message); continue }
            let attachmentIDs = message.attachments.map(\.id)
            let earliest = message.createdAt.addingTimeInterval(-Self.clockSkew)
            let row = history.first { row in
                row.mine && row.text == message.text && row.attachmentIDs == attachmentIDs &&
                    !earlierRowIDs.contains(row.id) && !claimed.contains(row.id) &&
                    (row.createdAt.map { $0 >= earliest } ?? false)
            }
            if let row { claimed.insert(row.id) } else { kept.append(message) }
        }
        all = kept
    }

    /// The message id in Relaycast's answer to a send: `message.id` for a DM,
    /// `id` for a channel post.
    static func messageID(fromSendResponse response: Any) -> String? {
        guard let data = response as? [String: Any] else { return nil }
        if let id = (data["message"] as? [String: Any])?["id"] as? String, !id.isEmpty { return id }
        if let id = data["id"] as? String, !id.isEmpty { return id }
        return nil
    }
}

/// The timers of one POST attempt, as the composer runs them. `sleep` is
/// injected so tests can drive them.
struct OutgoingTimers {
    var sleep: @MainActor (Duration) async -> Void = { duration in _ = try? await Task.sleep(for: duration) }
    var now: @MainActor () -> Date = { Date() }

    /// After `confirmationTimeout`, shows the attempt as failed if it is
    /// still unanswered. `edit` applies a change to the composer's state.
    @MainActor
    func expireWhenUnanswered(_ id: String, attempt: Int,
                              edit: @MainActor ((inout OutgoingMessages) -> Void) -> Void) async {
        await sleep(OutgoingMessages.confirmationTimeout)
        edit { $0.expire(id, attempt: attempt) }
    }

    /// After `sentRetention`, removes the accepted bubble even when history
    /// never showed its row.
    @MainActor
    func settleWhenAccepted(_ id: String,
                            edit: @MainActor ((inout OutgoingMessages) -> Void) -> Void) async {
        await sleep(.seconds(OutgoingMessages.sentRetention))
        let time = now()
        edit { $0.settle(id, now: time) }
    }
}

/// A tap on a failed bubble: history is reloaded first, and only then is it
/// decided whether to POST again, so a message Relaycast applied before its
/// answer was lost is not sent a second time.
enum OutgoingRetryTap {
    /// `loadHistory` reloads and returns the conversation's rows, or nil when
    /// the conversation is no longer open; `decide` is `OutgoingMessages.retry`
    /// on the composer's state; `post` sends one attempt.
    @MainActor
    @discardableResult
    static func run(loadHistory: @MainActor () async -> [OutgoingHistoryRow]?,
                    decide: @MainActor ([OutgoingHistoryRow]) -> OutgoingRetry,
                    post: @MainActor (OutgoingMessage) -> Void) async -> OutgoingRetry {
        guard let history = await loadHistory() else { return .ignored }
        let decision = decide(history)
        if case .post(let message) = decision { post(message) }
        return decision
    }
}
