import Foundation

/// Relay messages inside a session's tool work: an agent receives them as the
/// output of reading its inbox and sends them by running a command or calling
/// a tool. A chat can show both as messages, so a conversation shows what went
/// over the relay rather than the commands that carried it. The desktop's
/// `chat_relay_incoming` and `chat_relay_outgoing`.
///
/// Who a message names comes from session text, which is the agent's own and
/// could name anyone: a caller decides whether to show it.
public enum RelayLines {
    /// The frames a relayed message arrives in.
    static let frames = ["[via Agent Relay", "[Agent Relay native input"]

    /// The relay message in `text`, a tool's output, when it carries one: the
    /// first frame, which has to open a line (not a mention of it inside other
    /// text), read as `RelayEnvelope` reads it. Nil when the message is empty.
    public static func incoming(_ text: String) -> (sender: String?, messageID: String?, body: String)? {
        guard let start = frames.compactMap({ text.range(of: $0)?.lowerBound }).min() else { return nil }
        if start > text.startIndex, text[text.index(before: start)].isNewline == false { return nil }
        guard let envelope = RelayEnvelope.parse(String(text[start...])),
              !envelope.body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
        return (envelope.sender, envelope.messageID, envelope.body)
    }

    /// The relay message `text`, a tool call, sends, when it sends one: the
    /// `agent-relay-probe send` command (`--to`, `--address` or `--channel`;
    /// `--body` or a heredoc into a variable it passes), or the
    /// `send_relay_message` / `post_relay_channel` tool. A leading `@` is
    /// dropped from the recipient; a channel is named with its `#`.
    public static func outgoing(_ text: String) -> (recipient: String?, body: String)? {
        if text.contains("send_relay_message") || text.contains("post_relay_channel") {
            return toolCall(text)
        }
        guard let command = text.range(of: "agent-relay-probe") else { return nil }
        let rest = text[command.lowerBound...]
        guard let send = rest.range(of: " send ") else { return nil }
        let args = rest[send.lowerBound...]
        let person = (flag(args, "--to") ?? flag(args, "--address")).map { String($0.drop(while: { $0 == "@" })) }
        let recipient = person ?? flag(args, "--channel").map(channelName)
        let flagBody = flag(args, "--body").flatMap { $0.hasPrefix("$") ? nil : $0 }
        guard let raw = flagBody ?? heredoc(text) else { return nil }
        let body = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !body.isEmpty else { return nil }
        return (recipient, body)
    }

    /// A channel keeps its "#", and gets one when the send leaves it out.
    private static func channelName(_ channel: String) -> String {
        channel.hasPrefix("#") ? channel : "#" + channel
    }

    /// The `to` (or `channel`) and `text` of a relay tool call's JSON input:
    /// the first object in `text`.
    private static func toolCall(_ text: String) -> (recipient: String?, body: String)? {
        guard let json = firstObject(in: text),
              let value = try? JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any],
              let raw = value["text"] as? String else { return nil }
        let body = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !body.isEmpty else { return nil }
        if let to = value["to"] as? String {
            return (String(to.drop(while: { $0 == "@" })), body)
        }
        return ((value["channel"] as? String).map(channelName), body)
    }

    /// The JSON object that starts at the first `{` of `text`, through its
    /// matching `}` (braces inside strings do not count).
    private static func firstObject(in text: String) -> Substring? {
        guard let open = text.firstIndex(of: "{") else { return nil }
        var depth = 0, inString = false, escaped = false
        var index = open
        while index < text.endIndex {
            let character = text[index]
            if inString {
                if escaped { escaped = false } else if character == "\\" { escaped = true } else if character == "\"" { inString = false }
            } else if character == "\"" {
                inString = true
            } else if character == "{" {
                depth += 1
            } else if character == "}" {
                depth -= 1
                if depth == 0 { return text[open...index] }
            }
            index = text.index(after: index)
        }
        return nil
    }

    /// The value after `name` in a shell command: quoted, or a single word.
    private static func flag(_ args: Substring, _ name: String) -> String? {
        guard let at = args.range(of: name + " ") ?? args.range(of: name + "=") else { return nil }
        let value = args[at.upperBound...].drop(while: { $0.isWhitespace })
        guard let quote = value.first else { return nil }
        if quote == "'" || quote == "\"" {
            let inner = value.dropFirst()
            guard let end = inner.firstIndex(of: quote) else { return nil }
            return String(inner[..<end])
        }
        return value.split(whereSeparator: \.isWhitespace).first.map(String.init)
    }

    /// The text of the first heredoc in `text` (`<<'EOF'` … `EOF`).
    private static func heredoc(_ text: String) -> String? {
        guard let at = text.range(of: "<<") else { return nil }
        let rest = text[at.upperBound...]
        guard let lineEnd = rest.firstIndex(where: \.isNewline) else { return nil }
        let quotes: Set<Character> = ["'", "\""]
        var opener = Substring(rest[..<lineEnd].trimmingCharacters(in: .whitespaces))
        opener = opener.drop(while: { $0 == "-" })
        opener = opener.drop(while: { quotes.contains($0) })
        while let last = opener.last, quotes.contains(last) { opener = opener.dropLast() }
        guard let marker = opener.split(omittingEmptySubsequences: false, whereSeparator: { $0 == ")" || $0 == " " }).first,
              !marker.isEmpty else { return nil }
        let body = rest[rest.index(after: lineEnd)...]
        let end = body.range(of: "\n" + marker)?.lowerBound ?? body.endIndex
        return String(body[..<end])
    }
}
