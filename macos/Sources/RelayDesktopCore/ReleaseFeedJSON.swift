import Foundation

/// A value of a feed envelope, as the strict reader decodes it. Members keep
/// the order they were received in; the reader has refused any duplicate.
///
/// **Proposed contract, not confirmed.** Like the rest of the feed reader,
/// this follows the Cloud team's draft (relay-control `docs/desktop-feed.md`)
/// and mirrors Linux and Windows (gtk/src/updates/feed); the encodings may
/// change.
public indirect enum FeedJSON: Equatable, Sendable {
    case null
    case bool(Bool)
    /// A safe integer: the feed carries no other number.
    case integer(Int64)
    case string(String)
    case array([FeedJSON])
    case object([FeedJSONMember])

    /// The member named `name`, compared by its UTF-8 bytes: Swift's `==`
    /// would take a canonically equivalent name for it.
    public subscript(name: String) -> FeedJSON? {
        guard case let .object(members) = self else { return nil }
        return members.first { $0.name.utf8.elementsEqual(name.utf8) }?.value
    }

    /// The text of a string value.
    public var text: String? {
        if case let .string(text) = self { return text }
        return nil
    }
}

public struct FeedJSONMember: Equatable, Sendable {
    public let name: String
    public let value: FeedJSON

    public init(_ name: String, _ value: FeedJSON) {
        self.name = name
        self.value = value
    }
}

/// Why a text is not strict I-JSON.
public enum StrictJSONError: Error, Equatable, Sendable {
    /// The bytes are not UTF-8.
    case invalidUTF8
    /// The text starts with a byte order mark.
    case byteOrderMark
    /// An object names a member twice.
    case duplicateName
    /// A `\u` escape is an unpaired surrogate.
    case loneSurrogate
    /// A number is not a safe integer.
    case number
    /// Something follows the top-level value.
    case trailingCharacters
    /// Values nest deeper than the feed ever needs.
    case tooDeep
    /// Anything else outside the RFC 8259 grammar.
    case syntax
}

/// A strict I-JSON (RFC 7493) reader for a feed envelope's raw bytes, as on
/// Linux and Windows (gtk/src/updates/feed/strict_json.rs).
///
/// RFC 8785 requires its input to be I-JSON, and a reader that kept either
/// of two members with one name would let a signature over one reading
/// verify a text other readers read differently. This refuses, at any
/// depth: a duplicate member name (compared after escape decoding), a lone
/// surrogate, a leading byte order mark, invalid UTF-8, characters after
/// the top-level value, and anything outside the RFC 8259 grammar. The
/// manifest carries only safe integers, so a fraction, an exponent, `-0` or
/// an integer beyond ±(2^53−1) is refused too.
public enum StrictJSON {
    /// The largest integer the feed accepts: 2^53 − 1.
    public static let maxSafeInteger: UInt64 = (1 << 53) - 1
    /// How deeply values may nest. The envelope nests four levels; this
    /// bounds the recursion of a hostile text.
    static let maxDepth = 32

    /// `raw` read as one strict I-JSON value, or why it is not one.
    public static func parse(_ raw: [UInt8]) -> Result<FeedJSON, StrictJSONError> {
        guard validUTF8(raw) else { return .failure(.invalidUTF8) }
        if raw.starts(with: [0xEF, 0xBB, 0xBF]) { return .failure(.byteOrderMark) }
        var parser = Parser(bytes: raw)
        do {
            parser.whitespace()
            let value = try parser.value(depth: 0)
            parser.whitespace()
            guard parser.at == raw.count else { return .failure(.trailingCharacters) }
            return .success(value)
        } catch let error as StrictJSONError {
            return .failure(error)
        } catch {
            return .failure(.syntax)
        }
    }

    /// Why `raw` is not strict I-JSON; nil when it is.
    public static func check(_ raw: [UInt8]) -> StrictJSONError? {
        if case let .failure(error) = parse(raw) { return error }
        return nil
    }

    /// RFC 3629 UTF-8: no overlong form, no surrogate, nothing past U+10FFFF.
    static func validUTF8(_ bytes: [UInt8]) -> Bool {
        var index = 0
        while index < bytes.count {
            let lead = bytes[index]
            if lead < 0x80 {
                index += 1
                continue
            }
            let count: Int
            var low: UInt8 = 0x80
            var high: UInt8 = 0xBF
            switch lead {
            case 0xC2...0xDF: count = 1
            case 0xE0: count = 2; low = 0xA0
            case 0xE1...0xEC, 0xEE...0xEF: count = 2
            case 0xED: count = 2; high = 0x9F
            case 0xF0: count = 3; low = 0x90
            case 0xF1...0xF3: count = 3
            case 0xF4: count = 3; high = 0x8F
            default: return false
            }
            guard index + count < bytes.count else { return false }
            for offset in 1...count {
                let byte = bytes[index + offset]
                let range: ClosedRange<UInt8> = offset == 1 ? low...high : 0x80...0xBF
                guard range.contains(byte) else { return false }
            }
            index += count + 1
        }
        return true
    }
}

private struct Parser {
    let bytes: [UInt8]
    var at = 0

    init(bytes: [UInt8]) {
        self.bytes = bytes
    }

    func peek(_ offset: Int = 0) -> UInt8? {
        at + offset < bytes.count ? bytes[at + offset] : nil
    }

    mutating func next() throws -> UInt8 {
        guard let byte = peek() else { throw StrictJSONError.syntax }
        at += 1
        return byte
    }

    mutating func consume(_ wanted: UInt8) throws {
        guard try next() == wanted else { throw StrictJSONError.syntax }
    }

    mutating func whitespace() {
        while let byte = peek(), byte == 0x20 || byte == 0x09 || byte == 0x0A || byte == 0x0D {
            at += 1
        }
    }

    mutating func value(depth: Int) throws -> FeedJSON {
        guard depth <= StrictJSON.maxDepth else { throw StrictJSONError.tooDeep }
        guard let byte = peek() else { throw StrictJSONError.syntax }
        switch byte {
        case UInt8(ascii: "{"): return try object(depth: depth)
        case UInt8(ascii: "["): return try array(depth: depth)
        case UInt8(ascii: "\""): return .string(try string())
        case UInt8(ascii: "t"): try literal("true"); return .bool(true)
        case UInt8(ascii: "f"): try literal("false"); return .bool(false)
        case UInt8(ascii: "n"): try literal("null"); return .null
        case UInt8(ascii: "-"), UInt8(ascii: "0")...UInt8(ascii: "9"): return .integer(try number())
        default: throw StrictJSONError.syntax
        }
    }

    mutating func literal(_ word: String) throws {
        for byte in word.utf8 { try consume(byte) }
    }

    mutating func object(depth: Int) throws -> FeedJSON {
        try consume(UInt8(ascii: "{"))
        whitespace()
        if peek() == UInt8(ascii: "}") {
            at += 1
            return .object([])
        }
        var names = Set<[UInt8]>()
        var members: [FeedJSONMember] = []
        while true {
            guard peek() == UInt8(ascii: "\"") else { throw StrictJSONError.syntax }
            let name = try string()
            guard names.insert(Array(name.utf8)).inserted else { throw StrictJSONError.duplicateName }
            whitespace()
            try consume(UInt8(ascii: ":"))
            whitespace()
            members.append(FeedJSONMember(name, try value(depth: depth + 1)))
            whitespace()
            switch try next() {
            case UInt8(ascii: ","): whitespace()
            case UInt8(ascii: "}"): return .object(members)
            default: throw StrictJSONError.syntax
            }
        }
    }

    mutating func array(depth: Int) throws -> FeedJSON {
        try consume(UInt8(ascii: "["))
        whitespace()
        if peek() == UInt8(ascii: "]") {
            at += 1
            return .array([])
        }
        var items: [FeedJSON] = []
        while true {
            items.append(try value(depth: depth + 1))
            whitespace()
            switch try next() {
            case UInt8(ascii: ","): whitespace()
            case UInt8(ascii: "]"): return .array(items)
            default: throw StrictJSONError.syntax
            }
        }
    }

    /// An integer: `-?(0|[1-9][0-9]*)`, at most 2^53 − 1 in magnitude, and
    /// not `-0`. A fraction or exponent is refused as a non-integer.
    mutating func number() throws -> Int64 {
        let negative = peek() == UInt8(ascii: "-")
        if negative { at += 1 }
        let start = at
        while let byte = peek(), (UInt8(ascii: "0")...UInt8(ascii: "9")).contains(byte) {
            at += 1
        }
        let digits = bytes[start..<at]
        guard let first = digits.first else { throw StrictJSONError.syntax }
        if first == UInt8(ascii: "0") {
            if digits.count > 1 { throw StrictJSONError.syntax }
            if negative { throw StrictJSONError.number }
        }
        if let byte = peek(), byte == UInt8(ascii: ".") || byte == UInt8(ascii: "e") || byte == UInt8(ascii: "E") {
            throw StrictJSONError.number
        }
        var magnitude: UInt64 = 0
        for digit in digits {
            magnitude = magnitude * 10 + UInt64(digit - UInt8(ascii: "0"))
            guard magnitude <= StrictJSON.maxSafeInteger else { throw StrictJSONError.number }
        }
        return negative ? -Int64(magnitude) : Int64(magnitude)
    }

    /// A string, decoded so names compare after escape decoding.
    mutating func string() throws -> String {
        try consume(UInt8(ascii: "\""))
        var decoded: [UInt8] = []
        while true {
            while let byte = peek(), byte != UInt8(ascii: "\""), byte != UInt8(ascii: "\\"), byte >= 0x20 {
                decoded.append(byte)
                at += 1
            }
            switch try next() {
            case UInt8(ascii: "\""): return String(decoding: decoded, as: UTF8.self)
            case UInt8(ascii: "\\"): decoded.append(contentsOf: String(Character(try escape())).utf8)
            default: throw StrictJSONError.syntax
            }
        }
    }

    mutating func escape() throws -> Unicode.Scalar {
        switch try next() {
        case UInt8(ascii: "\""): return "\""
        case UInt8(ascii: "\\"): return "\\"
        case UInt8(ascii: "/"): return "/"
        case UInt8(ascii: "b"): return "\u{8}"
        case UInt8(ascii: "f"): return "\u{c}"
        case UInt8(ascii: "n"): return "\n"
        case UInt8(ascii: "r"): return "\r"
        case UInt8(ascii: "t"): return "\t"
        case UInt8(ascii: "u"): return try unicodeEscape()
        default: throw StrictJSONError.syntax
        }
    }

    /// The character of a `\uXXXX` escape, joining a high surrogate with the
    /// `\uXXXX` low surrogate that must follow it.
    mutating func unicodeEscape() throws -> Unicode.Scalar {
        let unit = try hex4()
        let code: UInt32
        switch unit {
        case 0xD800...0xDBFF:
            guard peek() == UInt8(ascii: "\\"), peek(1) == UInt8(ascii: "u") else {
                throw StrictJSONError.loneSurrogate
            }
            at += 2
            let low = try hex4()
            guard (0xDC00...0xDFFF).contains(low) else { throw StrictJSONError.loneSurrogate }
            code = 0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00)
        case 0xDC00...0xDFFF:
            throw StrictJSONError.loneSurrogate
        default:
            code = unit
        }
        guard let scalar = Unicode.Scalar(code) else { throw StrictJSONError.loneSurrogate }
        return scalar
    }

    mutating func hex4() throws -> UInt32 {
        var total: UInt32 = 0
        for _ in 0..<4 {
            let byte = try next()
            let digit: UInt8
            switch byte {
            case UInt8(ascii: "0")...UInt8(ascii: "9"): digit = byte - UInt8(ascii: "0")
            case UInt8(ascii: "a")...UInt8(ascii: "f"): digit = byte - UInt8(ascii: "a") + 10
            case UInt8(ascii: "A")...UInt8(ascii: "F"): digit = byte - UInt8(ascii: "A") + 10
            default: throw StrictJSONError.syntax
            }
            total = total << 4 | UInt32(digit)
        }
        return total
    }
}

/// Why a value has no canonical form here.
public enum JCSError: Error, Equatable, Sendable {
    /// A number is not an integer within ±(2^53 − 1).
    case number
}

/// The JSON Canonicalization Scheme (RFC 8785) for a feed manifest, as on
/// Linux and Windows (gtk/src/updates/feed/jcs.rs).
///
/// The signature covers these bytes of the `manifest` member as received,
/// so a manifest re-serialised with other whitespace or member order still
/// verifies. Members are sorted by the UTF-16 code units of their names,
/// nothing is padded, strings are escaped as section 3.2.2.2 requires, and
/// numbers are written as section 3.2.2.3 does for an integer.
public enum JCS {
    /// The canonical UTF-8 bytes of `value`.
    public static func canonicalize(_ value: FeedJSON) throws -> [UInt8] {
        var out: [UInt8] = []
        try write(value, to: &out)
        return out
    }

    private static func write(_ value: FeedJSON, to out: inout [UInt8]) throws {
        switch value {
        case .null: out.append(contentsOf: "null".utf8)
        case .bool(true): out.append(contentsOf: "true".utf8)
        case .bool(false): out.append(contentsOf: "false".utf8)
        case let .integer(number):
            guard number.magnitude <= StrictJSON.maxSafeInteger else { throw JCSError.number }
            out.append(contentsOf: String(number).utf8)
        case let .string(text):
            write(text, to: &out)
        case let .array(items):
            out.append(UInt8(ascii: "["))
            for (index, item) in items.enumerated() {
                if index > 0 { out.append(UInt8(ascii: ",")) }
                try write(item, to: &out)
            }
            out.append(UInt8(ascii: "]"))
        case let .object(members):
            let sorted = members.sorted { $0.name.utf16.lexicographicallyPrecedes($1.name.utf16) }
            out.append(UInt8(ascii: "{"))
            for (index, member) in sorted.enumerated() {
                if index > 0 { out.append(UInt8(ascii: ",")) }
                write(member.name, to: &out)
                out.append(UInt8(ascii: ":"))
                try write(member.value, to: &out)
            }
            out.append(UInt8(ascii: "}"))
        }
    }

    private static func write(_ text: String, to out: inout [UInt8]) {
        out.append(UInt8(ascii: "\""))
        for byte in text.utf8 {
            switch byte {
            case UInt8(ascii: "\""): out.append(contentsOf: #"\""#.utf8)
            case UInt8(ascii: "\\"): out.append(contentsOf: #"\\"#.utf8)
            case 0x08: out.append(contentsOf: #"\b"#.utf8)
            case 0x09: out.append(contentsOf: #"\t"#.utf8)
            case 0x0A: out.append(contentsOf: #"\n"#.utf8)
            case 0x0C: out.append(contentsOf: #"\f"#.utf8)
            case 0x0D: out.append(contentsOf: #"\r"#.utf8)
            case ..<0x20: out.append(contentsOf: String(format: "\\u%04x", Int(byte)).utf8)
            default: out.append(byte)
            }
        }
        out.append(UInt8(ascii: "\""))
    }
}
