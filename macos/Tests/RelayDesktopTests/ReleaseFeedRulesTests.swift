import Foundation
import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

/// The strict I-JSON pass, RFC 8785 canonical bytes, asset selection, the
/// download host rule and the manifest rules the draft's fixtures do not
/// cover. These mirror gtk/src/updates/feed/{strict_json,jcs}.rs and
/// select_tests.rs.
final class ReleaseFeedRulesTests: XCTestCase {
    private typealias F = FeedFixtures

    private func strict(_ text: String) -> StrictJSONError? {
        StrictJSON.check(Array(text.utf8))
    }

    private func canonical(_ text: String) throws -> [UInt8] {
        try JCS.canonicalize(try F.json(text))
    }

    // MARK: Strict I-JSON

    func testPlainJSONIsAccepted() {
        for text in [
            "{}", "[]", " \t\r\n{\"a\" : [1, -2, 0, true, false, null, \"x\"]}\r\n",
            #"{"a":{"b":[{}]},"c":"\u00e9\ud83d\ude00\"\/"}"#,
            "9007199254740991", "-9007199254740991", #""pro\u0066ile""#, #"[{"a":1},{"a":2}]"#,
        ] {
            XCTAssertNil(strict(text), text)
        }
        XCTAssertEqual(try F.json(#"{"c":"\u00e9\ud83d\ude00\"\/"}"#)["c"]?.text, "é😀\"/")
    }

    func testDuplicateNamesAreRefusedAtAnyDepthAndAfterEscapes() {
        for text in [
            #"{"a":1,"a":1}"#, #"{"m":{"p":1,"q":2,"p":3}}"#, #"{"m":[{"x":1},{"y":2,"y":2}]}"#,
            #"{"profile":"a","pro\u0066ile":"b"}"#,
        ] {
            XCTAssertEqual(strict(text), .duplicateName, text)
        }
    }

    func testLoneSurrogatesAreRefused() {
        for text in [#""\ud800""#, #""\ud800x""#, #""\ud800\u0041""#, #""\udc00""#, #"{"\udfff":1}"#] {
            XCTAssertEqual(strict(text), .loneSurrogate, text)
        }
    }

    func testByteOrderMarksTrailingTextAndInvalidUTF8AreRefused() {
        XCTAssertEqual(strict("\u{FEFF}{}"), .byteOrderMark)
        XCTAssertEqual(strict("{} {}"), .trailingCharacters)
        XCTAssertEqual(strict("{}x"), .trailingCharacters)
        XCTAssertEqual(StrictJSON.check([0x22, 0xFF, 0x22]), .invalidUTF8)
        XCTAssertEqual(StrictJSON.check([0x22, 0xED, 0xA0, 0x80, 0x22]), .invalidUTF8)
    }

    func testOnlySafeIntegersAreNumbers() {
        for text in [
            "1.5", "1.0", "1e3", "1E3", "-0", "9007199254740992", "-9007199254740992",
            "123456789012345678901234567890",
        ] {
            XCTAssertEqual(strict(text), .number, text)
        }
    }

    func testMalformedTextIsASyntaxError() {
        for text in [
            "", " ", "{", #"{"a"}"#, #"{"a":1,}"#, "[1,]", "[1 2]", "{a:1}", "{'a':1}", "01", "-", "+1", "NaN",
            "Infinity", "tru", #""\x""#, #""\u12""#, "\"a\nb\"", "\"\u{1}\"", "\"open", "/* c */ {}", "\u{A0}{}",
        ] {
            XCTAssertEqual(strict(text), .syntax, text.debugDescription)
        }
        XCTAssertEqual(strict(String(repeating: "[", count: 40) + String(repeating: "]", count: 40)), .tooDeep)
    }

    // MARK: JCS

    func testMembersSortByUTF16CodeUnits() throws {
        XCTAssertEqual(
            try canonical(#"{"\ufb01":1,"\ud83d\ude00":2,"b":3,"a":4,"é":5,"A":6}"#),
            Array(#"{"A":6,"a":4,"b":3,"é":5,"😀":2,"ﬁ":1}"#.utf8))
    }

    func testStringsUseTheShortestEscapes() throws {
        XCTAssertEqual(
            try canonical(#"["\b\t\n\f\r\"\\","\u0000\u001f\u0001","/\u007fé€"]"#),
            Array((#"["\b\t\n\f\r\"\\","\u0000\u001f\u0001","/"# + "\u{7F}é€\"]").utf8))
    }

    func testNestedAndEmptyValuesKeepTheirShape() throws {
        for text in [
            #"{"a":[{"b":false,"c":true},"s"],"z":{"x":null,"y":[1,-2,9007199254740991]}}"#,
            #"{"":0,"a":[],"n":[{},[]],"o":{}}"#, "{}", "[]", "-9007199254740991",
        ] {
            XCTAssertEqual(try canonical(text), Array(text.utf8), text)
        }
        XCTAssertThrowsError(try JCS.canonicalize(.integer(9_007_199_254_740_992))) {
            XCTAssertEqual($0 as? JCSError, .number)
        }
    }

    // MARK: Selection and download hosts

    private func stableManifest() throws -> FeedManifest {
        try FeedManifest(try F.json(try F.text("manifest/valid-stable.json")))
    }

    func testSelectsTheOneAssetAndChecksItsName() throws {
        let manifest = try stableManifest()
        let pick = { (platform: FeedPlatform, arch: AppUpdateArchitecture, kind: FeedKind, floor: Int?) in
            try manifest.select(assetPrefix: "Desktop-x7k2q", platform: platform, arch: arch, kind: kind, osFloor: floor)
                .name
        }
        let cases: [(FeedPlatform, AppUpdateArchitecture, FeedKind, Int?, String)] = [
            (.linux, .x64, .tarball, nil, "Desktop-x7k2q-2026.11.9-Linux-x64.tar.gz"),
            (.linux, .x64, .deb, nil, "Desktop-x7k2q-2026.11.9-Linux-x64.deb"),
            (.linux, .arm64, .probe, nil, "Desktop-x7k2q-2026.11.9-Linux-arm64-probe.tar.gz"),
            (.windows, .x64, .setup, nil, "Desktop-x7k2q-2026.11.9-Windows-x64-Setup.exe"),
            (.windows, .arm64, .zip, nil, "Desktop-x7k2q-2026.11.9-Windows-arm64.zip"),
            (.macOS, .arm64, .image, 15, "Desktop-x7k2q-2026.11.9-macOS15-arm64.dmg"),
            (.macOS, .arm64, .image, nil, "Desktop-x7k2q-2026.11.9-macOS-arm64.dmg"),
            (.linux, .x64, .installScript, nil, "install.sh"),
        ]
        for (platform, arch, kind, floor, name) in cases {
            XCTAssertEqual(try pick(platform, arch, kind, floor), name)
        }
        let missing: [(FeedPlatform, AppUpdateArchitecture, FeedKind, Int?)] = [
            (.windows, .arm64, .setup, nil), (.linux, .arm64, .tarball, nil), (.macOS, .arm64, .image, 14),
        ]
        for (platform, arch, kind, floor) in missing {
            XCTAssertThrowsError(try pick(platform, arch, kind, floor)) { XCTAssertEqual($0 as? FeedError, .noAsset) }
        }
        XCTAssertThrowsError(
            try manifest.select(assetPrefix: "Desktop-y9p3r", platform: .linux, arch: .x64, kind: .tarball, osFloor: nil)
        ) { XCTAssertEqual($0 as? FeedError, .assetNameMismatch) }
        var doubled = manifest
        var copy = doubled.assets[2]
        copy.id = "a_copy"
        copy.name = "Desktop-x7k2q-2026.11.9-Linux-x64.copy.tar.gz"
        doubled.assets.append(copy)
        XCTAssertThrowsError(
            try doubled.select(assetPrefix: "Desktop-x7k2q", platform: .linux, arch: .x64, kind: .tarball, osFloor: nil)
        ) { XCTAssertEqual($0 as? FeedError, .ambiguousAsset) }
    }

    func testTheMacPicksTheHighestFloorItCanOpen() throws {
        let manifest = try stableManifest()
        let image = { (major: Int) in
            try manifest.diskImage(assetPrefix: "Desktop-x7k2q", arch: .arm64, macOSMajorVersion: major).name
        }
        XCTAssertEqual(try image(15), "Desktop-x7k2q-2026.11.9-macOS15-arm64.dmg")
        XCTAssertEqual(try image(26), "Desktop-x7k2q-2026.11.9-macOS15-arm64.dmg")
        // Below every floor, the unfloored image the release also offers.
        XCTAssertEqual(try image(14), "Desktop-x7k2q-2026.11.9-macOS-arm64.dmg")
        var floored = manifest
        floored.assets.removeAll { $0.platform == .macOS && $0.osFloor == nil }
        XCTAssertThrowsError(
            try floored.diskImage(assetPrefix: "Desktop-x7k2q", arch: .arm64, macOSMajorVersion: 14)
        ) { XCTAssertEqual($0 as? AppUpdateMetadataError, .requiresNewerMacOS(15)) }
        var unfloored = manifest
        unfloored.assets.removeAll { $0.osFloor != nil }
        XCTAssertEqual(
            try unfloored.diskImage(assetPrefix: "Desktop-x7k2q", arch: .arm64, macOSMajorVersion: 14).name,
            "Desktop-x7k2q-2026.11.9-macOS-arm64.dmg")
        XCTAssertThrowsError(try manifest.diskImage(assetPrefix: "Desktop-x7k2q", arch: .x64, macOSMajorVersion: 15)) {
            XCTAssertEqual($0 as? FeedError, .noAsset)
        }
    }

    func testADownloadHostIsListedOrTheSite() throws {
        let hosts = try stableManifest().downloadHosts
        let site = "https://relay.example.com"
        for url in [
            "https://storage.googleapis.com/bucket/object?X-Goog-Signature=ab", "https://storage.googleapis.com",
            "https://relay.example.com/cloud/api/v1/desktop/feed/assets/a_01",
        ] {
            XCTAssertTrue(feedDownloadHostAllowed(url, downloadHosts: hosts, site: site), url)
        }
        for url in [
            "http://storage.googleapis.com/x", "https://storage.googleapis.com:443/x",
            "https://storage.googleapis.com:8443/x", "https://user@storage.googleapis.com/x",
            "https://Storage.googleapis.com/x", "https://evil.storage.googleapis.com/x",
            "https://storage.googleapis.com.evil.com/x", "https://googleapis.com/x", "https://10.0.0.7/x",
            "https://[::1]/x", "https://storage.googleapis.com%2eevil.com/x", "https://relay.example.com:443/x",
            "storage.googleapis.com/x", "",
        ] {
            XCTAssertFalse(feedDownloadHostAllowed(url, downloadHosts: hosts, site: site), url)
        }
        XCTAssertFalse(
            feedDownloadHostAllowed("https://relay.example.com/x", downloadHosts: hosts, site: "http://relay.example.com"))
        XCTAssertFalse(feedDownloadHostAllowed("https://storage.googleapis.com/x", downloadHosts: [], site: site))
        XCTAssertTrue(feedDownloadHostAllowed("https://relay.example.com/x", downloadHosts: [], site: site))
    }

    // MARK: Manifest rules

    /// `value` with the member or element at `path` replaced by `new`.
    private func replacing(_ value: FeedJSON, at path: [String], with new: FeedJSON) -> FeedJSON {
        guard let head = path.first else { return new }
        let rest = Array(path.dropFirst())
        switch value {
        case let .object(members):
            return .object(members.map {
                $0.name == head ? FeedJSONMember($0.name, replacing($0.value, at: rest, with: new)) : $0
            })
        case var .array(items):
            if let index = Int(head), items.indices.contains(index) {
                items[index] = replacing(items[index], at: rest, with: new)
            }
            return .array(items)
        default:
            return value
        }
    }

    func testManifestRulesBeyondTheFixtures() throws {
        let base = try F.json(try F.text("manifest/valid-stable.json"))
        let refusal = { (path: String, value: FeedJSON) -> FeedError? in
            do {
                let changed = self.replacing(base, at: path.split(separator: "/").map(String.init), with: value)
                XCTAssertNotEqual(changed, base, path)
                _ = try FeedManifest(changed)
                return nil
            } catch {
                return error as? FeedError
            }
        }
        let strings = { (items: [String]) in FeedJSON.array(items.map(FeedJSON.string)) }
        let nineHosts = strings("abcdefghi".map { "\($0).com" })
        let cases: [(String, FeedJSON)] = [
            ("version", .string("2026.13.1")), ("version", .string("2026.0.1")), ("version", .string("2026.01.1")),
            ("version", .string("26.11.1")), ("version", .string("2026.11")),
            ("published_at", .string("2026-02-30T10:00:00Z")), ("published_at", .string("2026-11-20T24:00:00Z")),
            ("published_at", .string("2026-11-20 10:04:31Z")),
            ("notes", .string(String(repeating: "x", count: 65_537))),
            ("download_hosts", strings(["a.com", "a.com"])), ("download_hosts", strings(["localhost"])),
            ("download_hosts", strings(["-a.com"])), ("download_hosts", strings(["a.com."])),
            ("download_hosts", nineHosts),
            ("assets/0/os_floor", .integer(9)), ("assets/0/os_floor", .integer(100)), ("assets/0/size", .integer(0)),
            ("assets/0/id", .string("b_1")), ("assets/0/name", .string(".hidden")),
            ("assets/0/name", .string("a..b")), ("schema", .string("1")), ("profile", .string("")),
        ]
        for (path, value) in cases {
            XCTAssertEqual(refusal(path, value), .schemaInvalid, "\(path) \(value)")
        }
        XCTAssertNil(refusal("notes", .string(String(repeating: "x", count: 65_536))))
        XCTAssertNil(refusal("published_at", .string("2028-02-29T23:59:60Z")))
        guard case let .array(assets)? = base["assets"] else { return XCTFail("assets") }
        let repeated = replacing(base, at: ["assets"], with: .array(assets + [assets[0]]))
        XCTAssertThrowsError(try FeedManifest(repeated)) { XCTAssertEqual($0 as? FeedError, .schemaInvalid) }
    }

    func testTheProfileComesFromTheAssetPrefix() {
        XCTAssertEqual(AppInfo.profileID(fromAssetPrefix: "Desktop-x7k2q"), "x7k2q")
        XCTAssertEqual(AppInfo.profileID(fromAssetPrefix: "Desktop"), "")
        XCTAssertEqual(AppInfo.profileID(fromAssetPrefix: "RelayDesktop-x7k2q"), "")
    }
}
