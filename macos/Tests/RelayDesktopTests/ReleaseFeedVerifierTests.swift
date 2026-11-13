import CryptoKit
import Foundation
import XCTest
@testable import RelayDesktopCore

/// The draft contract's fixtures, shared with Linux and Windows
/// (gtk/tests/fixtures/desktop-feed/), each signed at run time with a
/// throwaway key, so no signature is committed. These mirror
/// gtk/src/updates/feed/tests.rs.
enum FeedFixtures {
    static let keyID = "fk_x7k2q_2026a"
    /// macos/Tests/RelayDesktopTests/<file> → the repository root.
    static let directory = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().appendingPathComponent("gtk/tests/fixtures/desktop-feed")

    // Force-tried: a 32-byte raw representation is always a valid seed.
    static let signingKey = try! Curve25519.Signing.PrivateKey(rawRepresentation: Data(repeating: 7, count: 32))
    static let otherKey = try! Curve25519.Signing.PrivateKey(rawRepresentation: Data(repeating: 9, count: 32))

    static let stable = FeedExpectation(profile: "x7k2q", channel: .stable, installedVersion: "2026.11.8")

    static func text(_ path: String) throws -> String {
        try String(contentsOf: directory.appendingPathComponent(path), encoding: .utf8)
    }

    static func json(_ text: String) throws -> FeedJSON {
        try StrictJSON.parse(Array(text.utf8)).get()
    }

    /// The fixture's case list, read as plain test data.
    static func cases(_ path: String) throws -> [String: Any] {
        let object = try JSONSerialization.jsonObject(with: Data(try text(path).utf8))
        return try XCTUnwrap(object as? [String: Any])
    }

    static func base64url(_ data: Data) -> String {
        data.base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }

    static func decodeBase64url(_ text: String) -> Data? {
        Data(base64Encoded: text.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/") + "==")
    }

    /// base64url without padding of `key`'s signature over `manifest`'s JCS bytes.
    static func sign(_ manifest: FeedJSON, key: Curve25519.Signing.PrivateKey = signingKey) throws -> String {
        base64url(try key.signature(for: Data(try JCS.canonicalize(manifest))))
    }

    /// An envelope around the manifest's text exactly as given.
    static func envelope(_ manifestText: String, _ signature: String, _ keyID: String = keyID) -> String {
        "{\"manifest\":\(manifestText),\"signature\":\"\(signature)\",\"key_id\":\"\(keyID)\"}"
    }

    static func signed(_ file: String) throws -> String {
        let text = try text(file)
        return envelope(text, try sign(try json(text)))
    }

    /// The verifier's result name for `raw`: `accept` or the error's code.
    static func result(
        _ raw: String, key: Curve25519.Signing.PrivateKey = signingKey, expected: FeedExpectation = stable
    ) -> String {
        do {
            _ = try ReleaseFeedVerifier.verify(Data(raw.utf8), key: key.publicKey.rawRepresentation, expected: expected)
            return "accept"
        } catch let error as FeedError {
            return error.code
        } catch {
            return "unexpected \(error)"
        }
    }
}

final class ReleaseFeedVerifierTests: XCTestCase {
    private typealias F = FeedFixtures

    func testEveryManifestCaseReachesItsClientResult() throws {
        let cases = try XCTUnwrap(F.cases("manifest-cases.json")["cases"] as? [[String: Any]])
        XCTAssertEqual(cases.count, 27)
        for item in cases {
            let file = try XCTUnwrap(item["file"] as? String)
            let text = try F.text(file.hasPrefix("./") ? String(file.dropFirst(2)) : file)
            let value = try? F.json(text)
            // A number JCS cannot write has no conformant signature; the
            // strict pass refuses the text before the signature is read.
            let signature = value.flatMap { try? F.sign($0) } ?? F.base64url(Data(count: 64))
            let schemaValid = item["schema"] as? String == "valid"
            XCTAssertEqual(value.flatMap { try? FeedManifest($0) } != nil, schemaValid, file)
            let request = item["request"] as? [String: Any] ?? [:]
            let channel = request["channel"] as? String ?? value?["channel"]?.text ?? "stable"
            let expected = FeedExpectation(
                profile: request["profile"] as? String ?? "x7k2q",
                channel: channel == "beta" ? .beta : .stable,
                installedVersion: "2026.1.0")
            XCTAssertEqual(F.result(F.envelope(text, signature), expected: expected), item["client"] as? String, file)
        }
    }

    func testEveryRawCaseReachesItsClientResultBeforeTheSignature() throws {
        let list = try F.cases("raw/cases.json")
        XCTAssertEqual(list["placeholder"] as? String, "$SIGNATURE")
        let signature = try F.sign(try F.json(try F.text("manifest/valid-stable.json")))
        let cases = try XCTUnwrap(list["cases"] as? [[String: Any]])
        XCTAssertEqual(cases.count, 7)
        for item in cases {
            let file = try XCTUnwrap(item["file"] as? String)
            let template = try F.text(file)
            XCTAssertEqual(template.components(separatedBy: "$SIGNATURE").count, 2, file)
            let raw = template.replacingOccurrences(of: "$SIGNATURE", with: signature)
            XCTAssertEqual(F.result(raw), item["client"] as? String, file)
            switch item["strict"] as? String {
            case "ok": XCTAssertNil(StrictJSON.check(Array(raw.utf8)), file)
            case "duplicate_name": XCTAssertEqual(StrictJSON.check(Array(raw.utf8)), .duplicateName, file)
            case let other: XCTFail("\(file): strict \(String(describing: other))")
            }
        }
    }

    func testEveryEnvelopeFixtureIsRefused() throws {
        let envelopes = try XCTUnwrap(F.cases("manifest-cases.json")["envelopes"] as? [[String: Any]])
        XCTAssertEqual(envelopes.count, 8)
        for item in envelopes {
            let file = try XCTUnwrap(item["file"] as? String)
            XCTAssertEqual(item["schema"] as? String, "invalid", file)
            let raw = try F.text(file)
            var want = "schema_invalid"
            // Its manifest breaks only the manifest schema, which is checked
            // after the (placeholder) signature; signed properly it is
            // schema-invalid.
            if file.hasSuffix("invalid-manifest-invalid.json") {
                let manifest = try XCTUnwrap(try F.json(raw)["manifest"])
                let text = String(decoding: try JCS.canonicalize(manifest), as: UTF8.self)
                XCTAssertEqual(F.result(F.envelope(text, try F.sign(manifest))), "schema_invalid")
                want = "signature_invalid"
            }
            XCTAssertEqual(F.result(raw), want, file)
        }
    }

    func testTheSignedStableAndBetaManifestsVerify() throws {
        let key = F.signingKey.publicKey.rawRepresentation
        let manifest = try ReleaseFeedVerifier.verify(
            Data(try F.signed("manifest/valid-stable.json").utf8), key: key, expected: F.stable)
        XCTAssertEqual(manifest.version, "2026.11.9")
        XCTAssertEqual(manifest.assets.count, 8)
        XCTAssertEqual(manifest.downloadHosts, ["storage.googleapis.com"])
        let beta = try ReleaseFeedVerifier.verify(
            Data(try F.signed("manifest/valid-beta.json").utf8), key: key,
            expected: FeedExpectation(profile: "x7k2q", channel: .beta, installedVersion: "2026.11.10-beta.1"))
        XCTAssertEqual(beta.notes, "- Beta: tray menu rework. Unicode survives JCS: é €.\n")
    }

    func testTamperingAnotherKeyAndAnotherKeyIDFail() throws {
        let raw = try F.signed("manifest/valid-stable.json")
        XCTAssertEqual(F.result(raw), "accept")
        // One byte of the signed content.
        for (from, to) in [("10:04:31Z", "10:04:32Z"), ("\"size\": 21311", "\"size\": 21312")] {
            let tampered = replacingFirst(from, with: to, in: raw)
            XCTAssertNotEqual(tampered, raw)
            XCTAssertEqual(F.result(tampered), "signature_invalid", to)
        }
        // One bit of the signature.
        let signature = try XCTUnwrap(try F.json(raw)["signature"]?.text)
        var bytes = try XCTUnwrap(F.decodeBase64url(signature))
        bytes[0] ^= 1
        XCTAssertEqual(F.result(raw.replacingOccurrences(of: signature, with: F.base64url(bytes))), "signature_invalid")
        // The right key_id, signed by another key.
        XCTAssertEqual(F.result(raw, key: F.otherKey), "signature_invalid")
        // Another profile's key_id is refused before verification, as are
        // another profile's manifests under this one's key.
        for keyID in ["fk_y9p3r_2026a", "fk_x7k2qq_2026a"] {
            XCTAssertEqual(F.result(raw.replacingOccurrences(of: F.keyID, with: keyID)), "key_id_refused", keyID)
        }
        XCTAssertEqual(F.result(try F.signed("manifest/other-profile-stable.json")), "profile_mismatch")
    }

    func testTransportWhitespaceAndMemberOrderDoNotMatter() throws {
        let raw = try F.signed("manifest/valid-stable.json")
        let value = try F.json(raw)
        let compact = String(decoding: try JCS.canonicalize(value), as: UTF8.self)
        let part = { (name: String) in String(decoding: try JCS.canonicalize(XCTUnwrap(value[name])), as: UTF8.self) }
        let keyID = try part("key_id")
        let signature = try part("signature")
        let manifest = try part("manifest")
        let reordered = "{\"key_id\":\(keyID),\"signature\":\(signature),\"manifest\":\(manifest)}"
        let crlfTabs = " \r\n" + raw.replacingOccurrences(of: "  ", with: "\t")
            .replacingOccurrences(of: "\n", with: "\r\n")
            .replacingOccurrences(of: "\": ", with: "\" :\t") + "\t\n"
        for text in [raw, compact, reordered, crlfTabs] {
            XCTAssertEqual(F.result(text), "accept", text)
        }
    }

    func testTheEnvelopeIsStrictBeforeAnythingElse() throws {
        let raw = try F.signed("manifest/valid-stable.json")
        for text in [
            "\u{FEFF}\(raw)",
            "\(raw){}",
            replacingFirst("\"x7k2q\"", with: "\"x7k2q\",\"profile\":\"x7k2q\"", in: raw),
            replacingFirst("Windows arm64.", with: "Windows arm64.\\ud800", in: raw),
            replacingFirst("\"schema\": 1", with: "\"schema\": 1.0", in: raw),
            "[]",
            "\"manifest\"",
        ] {
            XCTAssertNotEqual(text, raw)
            XCTAssertEqual(F.result(text), "schema_invalid", text)
        }
    }

    func testTheSignatureAndKeyIDGrammars() throws {
        let good = F.base64url(Data(count: 64))
        XCTAssertEqual(good.count, 86)
        XCTAssertNoThrow(try ReleaseFeedVerifier.decodeSignature(good))
        for bad in [
            good + "==", String(good.prefix(85)), String(good.prefix(85)) + "B",
            good.replacingOccurrences(of: "A", with: "+"), String(good.prefix(84)) + "/A", "",
        ] {
            XCTAssertThrowsError(try ReleaseFeedVerifier.decodeSignature(bad), bad) {
                XCTAssertEqual($0 as? FeedError, .schemaInvalid, bad)
            }
        }
        XCTAssertEqual(ReleaseFeedVerifier.keyIDProfile("fk_x7k2q_2026a"), "x7k2q")
        for bad in [
            "fk-x7k2q-2026a", "fk_X7K2Q_2026a", "fk__2026a", "fk_x7k2q_", "fk_x7k2q_2026_a",
            "fk_x7k2q_abcdefghijklmnopq", "fk_x7k2q", "",
        ] {
            XCTAssertNil(ReleaseFeedVerifier.keyIDProfile(bad), bad)
        }
    }

    func testAVersionMustBeNewerThanTheInstalledOne() throws {
        let raw = try F.signed("manifest/valid-stable.json")
        for installed in ["2026.11.9", "2026.12.0", "2027.1.0"] {
            let expected = FeedExpectation(profile: "x7k2q", channel: .stable, installedVersion: installed)
            XCTAssertEqual(F.result(raw, expected: expected), "not_newer", installed)
        }
        for installed in ["2026.11.8", "2026.11.9-beta.3", "2025.12.40"] {
            let expected = FeedExpectation(profile: "x7k2q", channel: .stable, installedVersion: installed)
            XCTAssertEqual(F.result(raw, expected: expected), "accept", installed)
        }
    }

    func testTheCompiledKeyDecodesFromStandardBase64() {
        let key = F.signingKey.publicKey.rawRepresentation
        let text = key.base64EncodedString()
        XCTAssertEqual(text.count, 44)
        XCTAssertEqual(ReleaseFeedVerifier.decodePublicKey(text), key)
        XCTAssertNil(ReleaseFeedVerifier.decodePublicKey(F.base64url(key)))
        XCTAssertNil(ReleaseFeedVerifier.decodePublicKey(Data(repeating: 1, count: 31).base64EncodedString()))
        XCTAssertNil(ReleaseFeedVerifier.decodePublicKey(""))
    }

    func testEveryErrorHasACodeAndASentence() {
        XCTAssertEqual(FeedError.allCases.count, 10)
        XCTAssertEqual(Set(FeedError.allCases.map(\.code)).count, 10)
        XCTAssertEqual(
            FeedError.allCases.map(\.code),
            ["schema_invalid", "signature_invalid", "key_id_refused", "profile_mismatch", "channel_mismatch",
             "duplicate_asset", "not_newer", "no_asset", "ambiguous_asset", "asset_name_mismatch"])
        for error in [FeedError.schemaInvalid, .signatureInvalid] {
            XCTAssertEqual(error.message, "The update could not be verified.")
        }
        XCTAssertEqual(FeedError.notNewer.message, "There is no newer update.")
        XCTAssertEqual(FeedError.noAsset.message, "There is no update for this computer.")
        XCTAssertTrue(FeedError.allCases.allSatisfy { $0.message.hasSuffix(".") })
    }

    private func replacingFirst(_ target: String, with replacement: String, in text: String) -> String {
        guard let range = text.range(of: target) else { return text }
        return text.replacingCharacters(in: range, with: replacement)
    }
}
