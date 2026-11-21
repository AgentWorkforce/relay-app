import Foundation

/// An agent's escalated question, read from the DM the relay core sends the
/// person (`question/text.rs`). The text is untrusted: anything that does not
/// read exactly as a question from the agent this chat is with stays the
/// plain bubble it was.
struct RelayQuestion: Equatable {
    enum Selection: Equatable {
        /// Free text only; no numbered options.
        case none
        case single
        case multiple
    }

    struct Option: Equatable {
        let number: Int
        let label: String
        let description: String?
    }

    struct Item: Equatable {
        /// The question line without its "(i/n) " prefix.
        let text: String
        /// The part before the first ": " when it reads as a short header.
        let header: String?
        /// The text after the header, or all of it.
        let body: String
        let options: [Option]
        let selection: Selection
        let allowsFreeText: Bool
    }

    /// As written, any case; the answer repeats it as written.
    let ref: String
    let agent: String
    let host: String
    let workingOn: String?
    let items: [Item]
    /// "Expires in {minutes} min" on the reply line.
    let expiresInMinutes: Int

    func expiresAt(sentAt: Date?) -> Date? {
        sentAt.map { $0.addingTimeInterval(TimeInterval(expiresInMinutes) * 60) }
    }
}

enum RelayQuestionParser {
    static let maximumQuestions = 10
    static let maximumOptions = 50
    /// Longer text is not read at all.
    static let maximumCharacters = 40_000
    static let maximumLines = 1_000

    /// The hint line that ends each question, and what it allows.
    static let hints: [String: (selection: RelayQuestion.Selection, freeText: Bool)] = [
        "free text": (.none, true),
        "Choose one or more (comma-separated numbers), or answer in free text.": (.multiple, true),
        "Choose one or more (comma-separated numbers).": (.multiple, false),
        "Choose a number, or answer in free text.": (.single, true),
        "Choose a number.": (.single, false),
    ]

    /// The question `text` carries, or nil. It is a question only when it is
    /// not the person's own message, its sender is `chatAgent` (the agent this
    /// DM is with), line 1 names that same agent, the ref is "q" and 8 hex
    /// digits, every question ends with a known hint line, and the reply line
    /// repeats the ref.
    static func parse(_ text: String, sender: String, mine: Bool, chatAgent: String) -> RelayQuestion? {
        guard !mine, !chatAgent.isEmpty, sender == chatAgent, text.count <= maximumCharacters else { return nil }
        var lines = text.split(separator: "\n", omittingEmptySubsequences: false)
            .map { $0.trimmingCharacters(in: .whitespaces) }
        guard lines.count <= maximumLines else { return nil }
        while lines.last?.isEmpty == true { lines.removeLast() }
        guard lines.count >= 4, let head = header(lines[0]), head.agent == chatAgent,
              let reply = replyLine(lines[lines.count - 1], ref: head.ref) else { return nil }

        var workingOn: String?
        var items: [RelayQuestion.Item] = []
        var textLines: [String] = []
        var options: [RelayQuestion.Option] = []
        // The core writes "Working on: …" as line 2 and "session …" right
        // after it, each once; only those two positions are metadata, so a
        // question that starts with either word keeps its line.
        var body = Array(lines[1..<(lines.count - 1)])
        if body.first?.hasPrefix("Working on: ") == true {
            workingOn = String(body.removeFirst().dropFirst("Working on: ".count))
            if body.first?.hasPrefix("session ") == true { body.removeFirst() }
        }
        for line in body {
            if let hint = hints[line] {
                guard !textLines.isEmpty, items.count < maximumQuestions,
                      (hint.selection == .none) == options.isEmpty else { return nil }
                items.append(item(textLines.joined(separator: "\n"), options: options, hint: hint))
                textLines = []
                options = []
                continue
            }
            if !textLines.isEmpty, let option = option(line, expected: options.count + 1) {
                guard options.count < maximumOptions else { return nil }
                options.append(option)
                continue
            }
            // Text after a question's options and before its hint is not the format.
            guard options.isEmpty else { return nil }
            if line.isEmpty && textLines.isEmpty { continue }
            textLines.append(line)
        }
        guard textLines.isEmpty, options.isEmpty, !items.isEmpty, reply.several == (items.count > 1) else { return nil }
        // Several questions carry "(i/n) " prefixes in order.
        var cleaned: [RelayQuestion.Item] = []
        for (index, entry) in items.enumerated() {
            let (stripped, position) = numbered(entry.text)
            if items.count > 1 {
                guard let position, position.0 == index + 1, position.1 == items.count else { return nil }
            } else if let position, position.0 != 1 || position.1 != 1 {
                return nil
            }
            cleaned.append(item(stripped, options: entry.options,
                                hint: (entry.selection, entry.allowsFreeText)))
        }
        return RelayQuestion(ref: head.ref, agent: head.agent, host: head.host, workingOn: workingOn,
                             items: cleaned, expiresInMinutes: reply.minutes)
    }

    /// "[Question {ref} from @{agent} on {host}]"
    static func header(_ line: String) -> (ref: String, agent: String, host: String)? {
        let prefix = "[Question "
        guard line.hasPrefix(prefix), line.hasSuffix("]") else { return nil }
        let inner = line.dropFirst(prefix.count).dropLast()
        let parts = inner.components(separatedBy: " from @")
        guard parts.count == 2, isRef(parts[0]) else { return nil }
        let rest = parts[1].components(separatedBy: " on ")
        guard rest.count >= 2, !rest[0].isEmpty, !rest[0].contains(" ") else { return nil }
        let host = rest.dropFirst().joined(separator: " on ")
        guard !host.isEmpty else { return nil }
        return (parts[0], rest[0], host)
    }

    /// "q" and 8 hex digits, any case.
    static func isRef(_ text: String) -> Bool {
        guard text.count == 9, text.first == "q" || text.first == "Q" else { return false }
        return text.dropFirst().allSatisfy(\.isHexDigit)
    }

    private static func replyLine(_ line: String, ref: String) -> (minutes: Int, several: Bool)? {
        let single = "Reply: answer \(ref) <n|text>. Expires in "
        let several = "Reply: answer \(ref) <n|text>; <n|text> (one per question, in order). Expires in "
        let suffix = " min; the agent continues without it."
        let isSeveral: Bool
        let rest: Substring
        if line.hasPrefix(single) {
            isSeveral = false
            rest = line.dropFirst(single.count)
        } else if line.hasPrefix(several) {
            isSeveral = true
            rest = line.dropFirst(several.count)
        } else {
            return nil
        }
        guard rest.hasSuffix(suffix) else { return nil }
        let number = rest.dropLast(suffix.count)
        guard !number.isEmpty, number.count <= 6, number.allSatisfy(\.isASCIIDigit), let minutes = Int(number) else { return nil }
        return (minutes, isSeveral)
    }

    /// "{n}. {label}" or "{n}. {label} — {description}", numbered from 1 in order.
    private static func option(_ line: String, expected: Int) -> RelayQuestion.Option? {
        let prefix = "\(expected). "
        guard line.hasPrefix(prefix) else { return nil }
        let rest = String(line.dropFirst(prefix.count))
        guard !rest.trimmingCharacters(in: .whitespaces).isEmpty else { return nil }
        if let dash = rest.range(of: " — ") {
            let label = String(rest[..<dash.lowerBound])
            let description = String(rest[dash.upperBound...])
            guard !label.isEmpty else { return nil }
            return .init(number: expected, label: label, description: description.isEmpty ? nil : description)
        }
        return .init(number: expected, label: rest, description: nil)
    }

    /// The text without a leading "(i/n) ", and i and n.
    private static func numbered(_ text: String) -> (String, (Int, Int)?) {
        guard text.hasPrefix("("), let close = text.firstIndex(of: ")"),
              text[text.index(after: close)...].hasPrefix(" ") else { return (text, nil) }
        let parts = text[text.index(after: text.startIndex)..<close].split(separator: "/")
        guard parts.count == 2, parts.allSatisfy({ !$0.isEmpty && $0.count <= 3 && $0.allSatisfy(\.isASCIIDigit) }),
              let index = Int(parts[0]), let count = Int(parts[1]) else { return (text, nil) }
        return (String(text[text.index(close, offsetBy: 2)...]), (index, count))
    }

    private static func item(_ text: String, options: [RelayQuestion.Option],
                             hint: (selection: RelayQuestion.Selection, freeText: Bool)) -> RelayQuestion.Item {
        var header: String?
        var body = text
        if let colon = text.range(of: ": "), text[..<colon.lowerBound].count <= 40,
           !text[..<colon.lowerBound].contains("\n") {
            header = String(text[..<colon.lowerBound])
            body = String(text[colon.upperBound...])
        }
        return .init(text: text, header: header, body: body, options: options,
                     selection: hint.selection, allowsFreeText: hint.freeText)
    }
}

private extension Character {
    var isASCIIDigit: Bool { isASCII && isNumber }
}

/// The answer line the core accepts: "answer {ref} {rest}", one part per
/// question joined by ";" in order, numbers comma-separated for a
/// multi-select, one line, at most 4096 characters.
enum RelayQuestionAnswer {
    static let maximumLength = 4096

    enum Response: Equatable {
        case choices([Int])
        case text(String)
    }

    enum Problem: Error, Equatable {
        /// One response per question is needed.
        case wrongCount
        /// A question has no answer yet.
        case unanswered(question: Int)
        /// A number that is not one of the question's options, or several
        /// numbers for a single-choice question.
        case invalidChoice(question: Int)
        /// Free text for a question that only takes numbers.
        case freeTextNotAllowed(question: Int)
        /// Free text that would read as option numbers.
        case looksLikeChoice(question: Int)
        case tooLong
    }

    /// Free text as the answer carries it: one line, spaces collapsed. With
    /// several questions a ";" would split the parts, so it becomes ",".
    static func freeText(_ text: String, severalQuestions: Bool) -> String {
        var line = text.components(separatedBy: .whitespacesAndNewlines).filter { !$0.isEmpty }.joined(separator: " ")
        if severalQuestions { line = line.replacingOccurrences(of: ";", with: ",") }
        return line
    }

    static func line(for question: RelayQuestion, responses: [Response]) -> Result<String, Problem> {
        guard responses.count == question.items.count else { return .failure(.wrongCount) }
        let several = question.items.count > 1
        var parts: [String] = []
        for (index, (item, response)) in zip(question.items, responses).enumerated() {
            let number = index + 1
            switch response {
            case .choices(let picked):
                let unique = Array(Set(picked)).sorted()
                guard !unique.isEmpty else { return .failure(.unanswered(question: number)) }
                guard item.selection != .none,
                      unique.allSatisfy({ (1...item.options.count).contains($0) }),
                      item.selection == .multiple || unique.count == 1 else {
                    return .failure(.invalidChoice(question: number))
                }
                parts.append(unique.map(String.init).joined(separator: ","))
            case .text(let raw):
                guard item.allowsFreeText else { return .failure(.freeTextNotAllowed(question: number)) }
                let text = freeText(raw, severalQuestions: several)
                guard !text.isEmpty else { return .failure(.unanswered(question: number)) }
                // "2" or "1, 3" typed as text would be taken as option numbers.
                if !item.options.isEmpty, text.allSatisfy({ $0.isNumber || $0 == "," || $0 == " " }) {
                    return .failure(.looksLikeChoice(question: number))
                }
                parts.append(text)
            }
        }
        let line = "answer \(question.ref) " + parts.joined(separator: several ? "; " : "")
        guard line.count <= maximumLength else { return .failure(.tooLong) }
        return .success(line)
    }
}

/// What the core said about a question afterwards. Wording can drift; text
/// that is not one of these is not a follow-up.
enum RelayQuestionFollowUp: Equatable {
    case delivered
    case queued
    case notDelivered(reason: String)
    case expired
    case alreadyAnswered
    case answeredInSession
    case stillOpen(reason: String)
    case notOpen

    /// The ref and what it says, or nil.
    static func parse(_ text: String) -> (ref: String, followUp: RelayQuestionFollowUp)? {
        let line = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !line.contains("\n") else { return nil }
        func after(_ prefix: String) -> (ref: String, rest: String)? {
            guard line.hasPrefix(prefix) else { return nil }
            let tail = line.dropFirst(prefix.count)
            let ref = String(tail.prefix(9))
            guard RelayQuestionParser.isRef(ref) else { return nil }
            return (ref, String(tail.dropFirst(9)))
        }
        if let (ref, rest) = after("Answer to question ") {
            if rest.hasPrefix(" delivered to @") { return (ref, .delivered) }
            if rest.hasPrefix(" is queued for @") { return (ref, .queued) }
            return nil
        }
        if let (ref, rest) = after("The answer to question ") {
            guard rest.hasPrefix(" was not delivered") else { return nil }
            let reason = rest.dropFirst(" was not delivered".count)
            return (ref, .notDelivered(reason: reason.hasPrefix(": ") ? String(reason.dropFirst(2)) : ""))
        }
        if let (ref, rest) = after("Question ") {
            if rest.hasPrefix(" expired") { return (ref, .expired) }
            if rest.hasPrefix(" was already answered in the session") { return (ref, .answeredInSession) }
            if rest.hasPrefix(" was already answered") { return (ref, .alreadyAnswered) }
            if rest.hasPrefix(" is still open") {
                var reason = rest.dropFirst(" is still open".count)
                if reason.hasPrefix(": ") { reason = reason.dropFirst(2) }
                if reason.hasSuffix(".") { reason = reason.dropLast() }
                return (ref, .stillOpen(reason: String(reason)))
            }
            if rest.hasPrefix(" is not open") { return (ref, .notOpen) }
        }
        return nil
    }
}

/// Where a question card stands, read from the chat's messages.
enum RelayQuestionState: Equatable {
    case open
    /// The person's answer is on its way (a pending bubble).
    case sending
    /// The send could not be confirmed; its bubble offers the retry.
    case sendFailed
    /// The person's answer line is in the chat.
    case sent
    case delivered
    case queued
    case notDelivered(reason: String)
    case expired
    case alreadyAnswered
    case answeredInSession
    case stillOpen(reason: String)
    case notOpen

    /// Whether the card offers a send.
    var answerable: Bool {
        switch self {
        case .open, .notDelivered, .stillOpen: true
        default: false
        }
    }

    /// One message in the chat, as the state rule reads it.
    struct Message {
        let text: String
        let mine: Bool
        let sender: String
        var createdAt: Date? = nil
    }

    /// A composer send not yet in history.
    struct PendingAnswer {
        let text: String
        let failed: Bool
        /// Relaycast accepted it.
        var accepted: Bool = false
    }

    /// How far the server's clock may differ from the device's, as
    /// `OutgoingMessages.clockSkew` allows for the same timestamps.
    static let clockSkew: TimeInterval = 120

    /// The state of `question`, whose message is `messages[index]`:
    /// 1. the messages after it: the person's own answer line, then the
    ///    agent's follow-ups for the same ref; a later message wins and an
    ///    unknown follow-up changes nothing;
    /// 2. an answer this device had accepted (`acceptedAt`, kept after its
    ///    bubble left): sent until history shows that answer row; from then
    ///    on the messages' order decides, so a "not delivered" reply to it
    ///    reopens the card;
    /// 3. the composer's pending sends;
    /// 4. the clock, with `clockSkew` of margin, since only the device can
    ///    time the question out (the core tells the inbox nothing).
    static func state(for question: RelayQuestion, at index: Int, in messages: [Message],
                      chatAgent: String, pending: [PendingAnswer] = [], acceptedAt: Date? = nil,
                      sentAt: Date?, now: Date) -> RelayQuestionState {
        var state = RelayQuestionState.open
        var latestOwnAnswerAt: Date?
        for message in messages.dropFirst(index + 1) {
            if message.mine {
                if isAnswer(message.text, to: question.ref) {
                    state = .sent
                    if let at = message.createdAt { latestOwnAnswerAt = max(latestOwnAnswerAt ?? at, at) }
                }
                continue
            }
            guard message.sender == chatAgent,
                  let (ref, followUp) = RelayQuestionFollowUp.parse(message.text),
                  ref.lowercased() == question.ref.lowercased() else { continue }
            switch followUp {
            case .delivered: state = .delivered
            case .queued: state = .queued
            case .notDelivered(let reason): state = .notDelivered(reason: reason)
            case .expired: state = .expired
            case .alreadyAnswered: state = .alreadyAnswered
            case .answeredInSession: state = .answeredInSession
            case .stillOpen(let reason): state = .stillOpen(reason: reason)
            case .notOpen: state = .notOpen
            }
        }
        // An accepted answer counts even when history never shows it. Once a
        // row of mine for this ref dates from it (within the clock margin),
        // history has caught up and the loop above already decided.
        if state.answerable, let acceptedAt {
            let caughtUp = latestOwnAnswerAt.map { $0 >= acceptedAt.addingTimeInterval(-clockSkew) } ?? false
            if !caughtUp { state = .sent }
        }
        let answers = pending.filter { isAnswer($0.text, to: question.ref) }
        if state.answerable, let last = answers.last {
            state = last.failed ? .sendFailed : (last.accepted ? .sent : .sending)
        }
        if state.answerable, let expires = question.expiresAt(sentAt: sentAt),
           now >= expires.addingTimeInterval(clockSkew) {
            state = .expired
        }
        return state
    }

    static func isAnswer(_ text: String, to ref: String) -> Bool {
        let line = text.trimmingCharacters(in: .whitespacesAndNewlines)
        let prefix = "answer \(ref) "
        return line.count > prefix.count && line.prefix(prefix.count).lowercased() == prefix.lowercased()
    }

    /// The ref an "answer {ref} …" line answers, lowercased, or nil.
    static func answeredRef(_ text: String) -> String? {
        let parts = text.trimmingCharacters(in: .whitespacesAndNewlines).split(separator: " ", maxSplits: 2)
        guard parts.count == 3, parts[0].lowercased() == "answer" else { return nil }
        let ref = String(parts[1])
        return RelayQuestionParser.isRef(ref) ? ref.lowercased() : nil
    }
}
