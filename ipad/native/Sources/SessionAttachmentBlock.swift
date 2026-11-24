import Foundation

/// The posted pictures named by the `Attachments:` block a desktop core
/// appends to a message it hands a session as the person's own turn (owner
/// native input, which carries no relay envelope):
///
///     Attachments:
///     - shot.png (image/png, 153.1 KB) saved to <folder>/attachments/<file id>/shot.png
///
/// That text is written by another machine and anyone can type it, so only
/// this exact shape is read, only the file id is taken from it (the name of
/// the path's parent folder, under a folder named `attachments`, and only a
/// plain id), and the path itself is never used. Anything that does not
/// match stays text.
enum SessionAttachmentBlock {
    struct Picture: Equatable {
        let fileID: String
        let filename: String
        let contentType: String
        /// From the size the line states, rounded up.
        let sizeBytes: Int
        /// The line as written, shown if the picture cannot be.
        let line: String

        var attachment: RelayAttachment {
            RelayAttachment(fileID: fileID, filename: filename, contentType: contentType, sizeBytes: sizeBytes)
        }
    }

    struct Parsed: Equatable {
        /// The text without the picture lines; the block's heading and its
        /// other lines stay when any line of it is not a picture.
        let text: String
        /// Never empty.
        let pictures: [Picture]
    }

    static let heading = "Attachments:"
    static let maximumIDLength = 128
    /// The core lists at most this many files.
    static let maximumLines = 20

    /// The pictures in the block that ends `text`, or nil when there is no
    /// such block or none of its lines is a picture.
    static func parse(_ text: String) -> Parsed? {
        guard text.contains(heading) else { return nil }
        var lines = text.split(omittingEmptySubsequences: false, whereSeparator: \.isNewline).map(String.init)
        while lines.last?.isEmpty == true { lines.removeLast() }
        // The block is the heading and the "- " lines after it, at the end.
        guard let headingIndex = lines.lastIndex(of: heading) else { return nil }
        let entries = Array(lines[(headingIndex + 1)...])
        guard !entries.isEmpty, entries.count <= maximumLines,
              entries.allSatisfy({ $0.hasPrefix("- ") }) else { return nil }
        var pictures: [Picture] = []
        var kept: [String] = []
        for line in entries {
            if let picture = picture(line) { pictures.append(picture) } else { kept.append(line) }
        }
        guard !pictures.isEmpty else { return nil }
        var shown = Array(lines[..<headingIndex])
        if !kept.isEmpty {
            shown.append(heading)
            shown.append(contentsOf: kept)
        }
        while let last = shown.last, last.trimmingCharacters(in: .whitespaces).isEmpty { shown.removeLast() }
        return Parsed(text: shown.joined(separator: "\n"), pictures: pictures)
    }

    // `- <name> (image/<type>, <size>) saved to <path>`. The name is
    // greedy, so a name holding the same words still ends at the last match.
    private static let lineShape = try? NSRegularExpression(
        pattern: "^- (.+) \\((image/[A-Za-z0-9.+-]{1,64}), ([0-9]{1,4} B|[0-9]{1,4}\\.[0-9] KB|[0-9]{1,6}\\.[0-9] MB)\\) saved to (.+)$")

    /// One line of the block as a picture, or nil.
    static func picture(_ line: String) -> Picture? {
        guard let lineShape, line.count <= 4_096 else { return nil }
        let range = NSRange(line.startIndex..<line.endIndex, in: line)
        guard let match = lineShape.firstMatch(in: line, range: range), match.numberOfRanges == 5,
              let nameRange = Range(match.range(at: 1), in: line),
              let typeRange = Range(match.range(at: 2), in: line),
              let sizeRange = Range(match.range(at: 3), in: line),
              let pathRange = Range(match.range(at: 4), in: line),
              let sizeBytes = bytes(String(line[sizeRange])),
              let fileID = fileID(inPath: String(line[pathRange])) else { return nil }
        let filename = String(line[nameRange])
        guard !filename.trimmingCharacters(in: .whitespaces).isEmpty else { return nil }
        return Picture(fileID: fileID, filename: filename, contentType: String(line[typeRange]).lowercased(),
                       sizeBytes: sizeBytes, line: line)
    }

    /// The core's 1024-based size (`512 B`, `153.1 KB`, `2.4 MB`) in bytes,
    /// rounded up to the next tenth of a unit so a cap is never undercut.
    static func bytes(_ size: String) -> Int? {
        let parts = size.split(separator: " ")
        guard parts.count == 2, let value = Double(parts[0]), value >= 0 else { return nil }
        let unit: Double
        switch parts[1] {
        case "B": unit = 1
        case "KB": unit = 1_024
        case "MB": unit = 1_024 * 1_024
        default: return nil
        }
        let rounded = parts[1] == "B" ? value : value + 0.05
        return Int((rounded * unit).rounded(.up))
    }

    /// The file id of a saved file's path: the name of its parent folder,
    /// which must sit in a folder named `attachments`, and must be a plain
    /// id. Either separator, so a Windows path reads too.
    static func fileID(inPath path: String) -> String? {
        let parts = path.split(omittingEmptySubsequences: false, whereSeparator: { $0 == "/" || $0 == "\\" })
        guard parts.count >= 3, parts[parts.count - 3] == "attachments", !parts[parts.count - 1].isEmpty else { return nil }
        let id = String(parts[parts.count - 2])
        return isPlainID(id) ? id : nil
    }

    /// ASCII letters, digits, `-` and `_`, 1 to `maximumIDLength` long.
    static func isPlainID(_ id: String) -> Bool {
        guard !id.isEmpty, id.utf8.count <= maximumIDLength else { return false }
        return id.utf8.allSatisfy { byte in
            (byte >= 0x30 && byte <= 0x39) || (byte >= 0x41 && byte <= 0x5A) || (byte >= 0x61 && byte <= 0x7A)
                || byte == 0x2D || byte == 0x5F
        }
    }
}
