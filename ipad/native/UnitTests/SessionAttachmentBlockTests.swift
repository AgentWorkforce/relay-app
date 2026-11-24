import XCTest
@testable import AgentRelayiPad

final class SessionAttachmentBlockTests: XCTestCase {
    private func line(name: String = "shot.png", type: String = "image/png", size: String = "153.1 KB",
                      path: String = "/data/app/attachments/file_1/shot.png") -> String {
        "- \(name) (\(type), \(size)) saved to \(path)"
    }

    func testReadsThePictureAndTakesTheIDFromTheParentFolder() throws {
        let entry = line()
        let parsed = try XCTUnwrap(SessionAttachmentBlock.parse("Look at this\n\nAttachments:\n\(entry)\n"))
        XCTAssertEqual(parsed.text, "Look at this")
        XCTAssertEqual(parsed.pictures, [SessionAttachmentBlock.Picture(
            fileID: "file_1", filename: "shot.png", contentType: "image/png",
            sizeBytes: 156_826, line: entry)])
        let attachment = parsed.pictures[0].attachment
        XCTAssertEqual(attachment.id, "file_1")
        XCTAssertEqual(attachment.filename, "shot.png")
        XCTAssertTrue(AttachmentPreview.showsPicture(attachment))
    }

    func testReadsAWindowsPath() {
        let parsed = SessionAttachmentBlock.parse(
            "Attachments:\n" + line(path: "C:\\Data\\App\\attachments\\Ab-9_z\\shot.png"))
        XCTAssertEqual(parsed?.pictures.first?.fileID, "Ab-9_z")
        XCTAssertEqual(parsed?.text, "")
    }

    func testKeepsOtherFilesAndTheHeading() {
        let other = "- notes.pdf (application/pdf, 2.4 MB) saved to /data/app/attachments/file_2/notes.pdf"
        let parsed = SessionAttachmentBlock.parse(
            "Two files\nAttachments:\n\(line())\n\(other)\n\(line(name: "b.JPG", type: "image/JPEG", size: "512 B", path: "/x/attachments/file_3/b.JPG"))")
        XCTAssertEqual(parsed?.text, "Two files\nAttachments:\n\(other)")
        XCTAssertEqual(parsed?.pictures.map(\.fileID), ["file_1", "file_3"])
        XCTAssertEqual(parsed?.pictures.last?.contentType, "image/jpeg")
        XCTAssertEqual(parsed?.pictures.last?.sizeBytes, 512)
    }

    func testNameWithBracketsAndTheSameWords() {
        let name = "shot (image/png, 1 B) saved to x.png"
        let parsed = SessionAttachmentBlock.parse("Attachments:\n" + line(name: name))
        XCTAssertEqual(parsed?.pictures.first?.filename, name)
        XCTAssertEqual(parsed?.pictures.first?.fileID, "file_1")
    }

    func testOnlyTheLastBlockAtTheEndIsRead() {
        let quoted = "Attachments:\n\(line(path: "/a/attachments/old_1/shot.png"))\nthen\nAttachments:\n\(line())"
        let parsed = SessionAttachmentBlock.parse(quoted)
        XCTAssertEqual(parsed?.pictures.map(\.fileID), ["file_1"])
        XCTAssertEqual(parsed?.text, "Attachments:\n\(line(path: "/a/attachments/old_1/shot.png"))\nthen")
    }

    func testNearMissesStayText() {
        let cases = [
            line(),
            "Attachment:\n" + line(),
            "attachments:\n" + line(),
            " Attachments:\n" + line(),
            "Attachments:\n" + line() + "\nmore text",
            "Attachments:",
            "Attachments:\n\n" + line(),
            "Attachments:\n" + line() + "\n  extra",
            "Attachments:\n" + line(type: "application/pdf"),
            "Attachments:\n" + line(type: "text/png"),
            "Attachments:\n" + line(size: "153 KB"),
            "Attachments:\n" + line(size: "153.12 KB"),
            "Attachments:\n" + line(size: "1.5 GB"),
            "Attachments:\n" + line(size: "-1 B"),
            "Attachments:\n" + line(name: ""),
            "Attachments:\n" + line(name: " "),
            "Attachments:\n" + line(path: "/data/app/files/file_1/shot.png"),
            "Attachments:\n" + line(path: "attachments/file_1/"),
            "Attachments:\n" + line(path: "file_1/shot.png"),
            "Attachments:\n" + line(path: "shot.png"),
            "Attachments:\n" + line().replacingOccurrences(of: " saved to ", with: " stored at "),
            "Attachments:\n" + line().replacingOccurrences(of: "- ", with: "* "),
            "Attachments:\n" + Array(repeating: line(), count: SessionAttachmentBlock.maximumLines + 1)
                .joined(separator: "\n"),
            "Attachments:\n" + line(path: "/a/attachments/file_1/" + String(repeating: "x", count: 4_096)),
        ]
        for text in cases {
            XCTAssertNil(SessionAttachmentBlock.parse(text), text.prefix(120).description)
        }
    }

    func testHostileIDsAreRefused() {
        let paths = [
            "/a/attachments/../shot.png",
            "/a/attachments/./shot.png",
            "/a/attachments//shot.png",
            "/a/attachments/file 1/shot.png",
            "/a/attachments/file.1/shot.png",
            "/a/attachments/file%2F1/shot.png",
            "/a/attachments/fil\u{00E9}/shot.png",
            "/a/attachments/\u{0660}\u{0661}/shot.png",
            "/a/attachments/file_1/../shot.png",
            "/a/attachments/a/b/shot.png",
            "/a/attachments/" + String(repeating: "a", count: SessionAttachmentBlock.maximumIDLength + 1) + "/shot.png",
        ]
        for path in paths {
            XCTAssertNil(SessionAttachmentBlock.parse("Attachments:\n" + line(path: path)), path)
        }
        let longest = String(repeating: "a", count: SessionAttachmentBlock.maximumIDLength)
        XCTAssertEqual(SessionAttachmentBlock.parse("Attachments:\n" + line(path: "/a/attachments/\(longest)/shot.png"))?
            .pictures.first?.fileID, longest)
        // Only the id is taken; the rest of the path is never used.
        let odd = SessionAttachmentBlock.parse("Attachments:\n" + line(path: "https://example.test/attachments/file_1/shot.png?x=1"))
        XCTAssertEqual(odd?.pictures.first?.attachment, RelayAttachment(fileID: "file_1", filename: "shot.png",
                                                                       contentType: "image/png", sizeBytes: 156_826))
    }

    func testPlainIDs() {
        XCTAssertTrue(SessionAttachmentBlock.isPlainID("file_1"))
        XCTAssertTrue(SessionAttachmentBlock.isPlainID("A-z_0"))
        for id in ["", ".", "..", "a/b", "a\\b", "a b", "a.b", "\u{00E9}", "a\u{0000}"] {
            XCTAssertFalse(SessionAttachmentBlock.isPlainID(id), id)
        }
        XCTAssertNil(SessionAttachmentBlock.fileID(inPath: "/a/attachments/../shot.png"))
        XCTAssertEqual(SessionAttachmentBlock.fileID(inPath: "/a/attachments/file_1/shot.png"), "file_1")
    }

    func testSizesAreRoundedUp() {
        XCTAssertEqual(SessionAttachmentBlock.bytes("0 B"), 0)
        XCTAssertEqual(SessionAttachmentBlock.bytes("512 B"), 512)
        XCTAssertEqual(SessionAttachmentBlock.bytes("1.0 KB"), 1_076)
        XCTAssertEqual(SessionAttachmentBlock.bytes("2.4 MB"), 2_569_012)
        XCTAssertNil(SessionAttachmentBlock.bytes("2.4 GB"))
        XCTAssertNil(SessionAttachmentBlock.bytes("2.4MB"))
        XCTAssertNil(SessionAttachmentBlock.bytes("x KB"))
        // A file the core states at the cap is over it once rounded up.
        let atCap = SessionAttachmentBlock.parse("Attachments:\n" + line(size: "20.0 MB"))
        XCTAssertEqual(atCap?.pictures.first.map { AttachmentPreview.showsPicture($0.attachment) }, false)
    }
}
