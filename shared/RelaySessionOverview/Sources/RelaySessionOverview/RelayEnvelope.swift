import Foundation

/// The Agent Relay envelope at the start of a session event's text: the line
/// a relayed message or a spawned agent's task arrives under, read after any
/// harness preamble blocks `withoutPreamble` skips. A delivered relay message's
/// header carries `message <id>`, so an app showing both the relay message and
/// the session's copy of it can show it once.
public struct RelayEnvelope: Equatable, Sendable {
    /// The relay message id the header names, or nil (a spawn task has none).
    public let messageID: String?
    /// The sender handle without `@`, or nil when the header names none.
    public let sender: String?
    /// The text after the envelope and any preamble, as `withoutPreamble` reads it.
    public let body: String

    public init(messageID: String?, sender: String?, body: String) {
        self.messageID = messageID
        self.sender = sender
        self.body = body
    }

    /// The envelope `text` starts with, after any leading preamble blocks,
    /// or nil when it starts with none.
    public static func parse(_ text: String) -> RelayEnvelope? {
        var rest = Substring(text).drop(while: { $0.isWhitespace })
        while true {
            if let after = withoutRelayEnvelope(rest) {
                let header = rest[..<(rest.firstIndex(of: "\n") ?? rest.endIndex)]
                let fields = headerFields(header)
                return RelayEnvelope(messageID: value(after: "message ", in: fields),
                                     sender: value(after: "from ", in: fields).map { $0.hasPrefix("@") ? String($0.dropFirst()) : $0 },
                                     body: withoutPreamble(String(after)))
            }
            guard rest.first == "<", let close = rest.firstIndex(of: ">") else { return nil }
            let tag = rest[rest.index(after: rest.startIndex)..<close]
            guard preambleTags.contains(tag), let end = rest.range(of: "</\(tag)>") else { return nil }
            rest = rest[end.upperBound...].drop(while: { $0.isWhitespace })
        }
    }

    /// The header's fields: `[via Agent Relay — from @a to @b · message m_1 · ref …]`
    /// splits into `from @a to @b`, `message m_1` and `ref …`. Some bridges
    /// separate fields with `; ` instead of ` · `.
    private static func headerFields(_ header: Substring) -> [Substring] {
        var inner = header.trimmingCharacters(in: .whitespaces)
        if inner.hasSuffix("]") { inner.removeLast() }
        if let dash = inner.range(of: " — ") { inner = String(inner[dash.upperBound...]) }
        return inner.components(separatedBy: " · ")
            .flatMap { $0.components(separatedBy: "; ") }
            .map { Substring($0.trimmingCharacters(in: .whitespaces)) }
    }

    /// The first word after `prefix` in the first field that starts with it.
    private static func value(after prefix: String, in fields: [Substring]) -> String? {
        for field in fields where field.hasPrefix(prefix) {
            let word = field.dropFirst(prefix.count).prefix { !$0.isWhitespace && $0 != ";" && $0 != "]" }
            if !word.isEmpty, !(prefix == "message " && word == "…") { return String(word) }
        }
        return nil
    }
}
