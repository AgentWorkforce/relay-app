import CryptoKit
import Foundation

/// Why an envelope was refused, as on Linux and Windows
/// (gtk/src/updates/feed/mod.rs). Each case is one result name of the
/// draft's fixtures (`code`).
public enum FeedError: String, Error, CaseIterable, Sendable {
    /// Not strict I-JSON, or the envelope or manifest breaks its schema.
    case schemaInvalid = "schema_invalid"
    /// The signature does not verify with the built-in key.
    case signatureInvalid = "signature_invalid"
    /// The envelope names another profile's key.
    case keyIDRefused = "key_id_refused"
    /// The manifest is for another profile.
    case profileMismatch = "profile_mismatch"
    /// The manifest is for another channel.
    case channelMismatch = "channel_mismatch"
    /// Two assets share an id or a name.
    case duplicateAsset = "duplicate_asset"
    /// The version is not newer than the installed one.
    case notNewer = "not_newer"
    /// No asset for this computer.
    case noAsset = "no_asset"
    /// More than one asset for this computer.
    case ambiguousAsset = "ambiguous_asset"
    /// The asset's name is not the one this build expects for the version.
    case assetNameMismatch = "asset_name_mismatch"

    /// The result name the draft's fixture cases use.
    public var code: String { rawValue }

    /// What the update UI says. A manifest that fails verification in any
    /// way is "could not be verified", as the design's table says for
    /// signature and schema failures.
    public var message: String {
        switch self {
        case .notNewer: return "There is no newer update."
        case .noAsset, .ambiguousAsset: return "There is no update for this computer."
        default: return "The update could not be verified."
        }
    }
}

public enum FeedChannel: String, CaseIterable, Sendable {
    case stable
    case beta
}

/// The platform an asset is for, named as in asset names.
public enum FeedPlatform: String, Sendable {
    case macOS
    case linux = "Linux"
    case windows = "Windows"
}

/// What an asset is.
public enum FeedKind: String, Sendable {
    /// A macOS disk image.
    case image
    /// The Linux per-user archive.
    case tarball
    /// The Windows per-user archive.
    case zip
    /// The Debian package.
    case deb
    /// The Windows installer.
    case setup
    /// The bare core.
    case probe
    /// The install script.
    case installScript = "install-script"
}

/// One downloadable file of the release.
public struct FeedAsset: Equatable, Hashable, Sendable {
    /// Opaque id the download endpoints take.
    public var id: String
    /// The file name, one path segment.
    public var name: String
    public var platform: FeedPlatform
    public var arch: AppUpdateArchitecture
    /// The macOS major version floor, on floored macOS assets only.
    public var osFloor: Int?
    public var kind: FeedKind
    /// The exact size in bytes.
    public var size: Int64
    /// Lower-case hex SHA-256 of the bytes.
    public var sha256: String
}

/// What the build expects of a manifest.
public struct FeedExpectation: Sendable {
    /// The built-in profile id.
    public var profile: String
    /// The channel asked for.
    public var channel: FeedChannel
    /// The running version, which the manifest's must be newer than.
    public var installedVersion: String

    public init(profile: String, channel: FeedChannel, installedVersion: String) {
        self.profile = profile
        self.channel = channel
        self.installedVersion = installedVersion
    }
}

/// A release as its signed manifest describes it, decoded once its
/// signature has verified, with every rule of the draft's
/// `desktop-feed-manifest.schema.json` (schema 1) enforced, as on Linux and
/// Windows (gtk/src/updates/feed/manifest.rs and select.rs).
///
/// **Proposed contract, not confirmed.** The Cloud team decides it; the
/// encodings, the `key_id` shape, the result names and the check order may
/// all change.
public struct FeedManifest: Equatable, Sendable {
    public var schema: Int64
    /// The opaque profile id.
    public var profile: String
    public var channel: FeedChannel
    public var version: String
    /// RFC 3339 UTC, whole seconds.
    public var publishedAt: String
    /// Markdown release notes.
    public var notes: String
    /// The only hosts a signed download URL may name.
    public var downloadHosts: [String]
    public var assets: [FeedAsset]

    /// Decodes and schema-checks a manifest. Unique ids and names are
    /// checked by `checkUniqueAssets`.
    public init(_ value: FeedJSON) throws {
        let object = try fields(value, required: Self.requiredFields, optional: [])
        guard case .integer(1) = object["schema"] else { throw FeedError.schemaInvalid }
        guard let channel = ascii(object["channel"]).flatMap(FeedChannel.init(rawValue:)) else {
            throw FeedError.schemaInvalid
        }
        schema = 1
        profile = try checked(object["profile"], validProfile)
        self.channel = channel
        version = try checked(object["version"]) { validVersion($0, channel: channel) }
        publishedAt = try checked(object["published_at"], validPublishedAt)
        notes = try checked(object["notes"]) { $0.unicodeScalars.count <= 65_536 }
        guard case let .array(hosts) = object["download_hosts"], hosts.count <= 8 else {
            throw FeedError.schemaInvalid
        }
        downloadHosts = try hosts.map { try checked($0, validHost) }
        guard Set(downloadHosts).count == downloadHosts.count,
              case let .array(items) = object["assets"], (1...64).contains(items.count) else {
            throw FeedError.schemaInvalid
        }
        assets = try items.map(asset)
        guard Set(assets).count == assets.count else { throw FeedError.schemaInvalid }
    }

    private static let requiredFields = [
        "schema", "profile", "channel", "version", "published_at", "notes", "download_hosts", "assets",
    ]

    /// The draft's cross-field rule: asset ids and names are unique.
    public func checkUniqueAssets() throws {
        guard Set(assets.map(\.id)).count == assets.count,
              Set(assets.map(\.name)).count == assets.count else { throw FeedError.duplicateAsset }
    }

    /// The asset for `platform`, `arch`, `kind` and `osFloor` (nil for an
    /// unfloored asset). Exactly one may match, and its name must be the
    /// one `assetPrefix` gives that asset of this version.
    public func select(
        assetPrefix: String, platform: FeedPlatform, arch: AppUpdateArchitecture, kind: FeedKind, osFloor: Int?
    ) throws -> FeedAsset {
        let matching = assets.filter {
            $0.platform == platform && $0.arch == arch && $0.kind == kind && $0.osFloor == osFloor
        }
        guard let asset = matching.first else { throw FeedError.noAsset }
        guard matching.count == 1 else { throw FeedError.ambiguousAsset }
        guard asset.name == Self.expectedName(assetPrefix: assetPrefix, version: version, asset: asset) else {
            throw FeedError.assetNameMismatch
        }
        return asset
    }

    /// This Mac's disk image: the highest-floored one it can open, else the
    /// unfloored one, as the GitHub reader chooses. A release with floored
    /// images this Mac cannot open and no unfloored one throws
    /// `requiresNewerMacOS` with the lowest floor.
    public func diskImage(
        assetPrefix: String, arch: AppUpdateArchitecture, macOSMajorVersion: Int
    ) throws -> FeedAsset {
        let images = assets.filter { $0.platform == .macOS && $0.arch == arch && $0.kind == .image }
        let floors = images.compactMap(\.osFloor)
        let floor = floors.filter { $0 <= macOSMajorVersion }.max()
        if floor == nil, let lowest = floors.min(), !images.contains(where: { $0.osFloor == nil }) {
            throw AppUpdateMetadataError.requiresNewerMacOS(lowest)
        }
        return try select(assetPrefix: assetPrefix, platform: .macOS, arch: arch, kind: .image, osFloor: floor)
    }

    /// The name `assetPrefix` gives `asset` of `version`:
    /// `<prefix>-<version>-<platform>[floor]-<arch>` with the kind's ending,
    /// or the install script's fixed name.
    public static func expectedName(assetPrefix: String, version: String, asset: FeedAsset) -> String {
        let platform = asset.platform.rawValue + (asset.osFloor.map(String.init) ?? "")
        let base = "\(assetPrefix)-\(version)-\(platform)-\(asset.arch.rawValue)"
        let windows = asset.platform == .windows
        switch asset.kind {
        case .image: return "\(base).dmg"
        case .tarball: return "\(base).tar.gz"
        case .zip: return "\(base).zip"
        case .deb: return "\(base).deb"
        case .setup: return "\(base)-Setup.exe"
        case .probe: return windows ? "\(base)-probe.zip" : "\(base)-probe.tar.gz"
        case .installScript: return windows ? "install.ps1" : "install.sh"
        }
    }
}

/// The signed feed's checks, in the order Linux and Windows run them
/// (gtk/src/updates/feed/mod.rs), on the raw bytes:
///
/// 1. Strict I-JSON (`StrictJSON`): anything it refuses is schema-invalid
///    before any parse.
/// 2. The envelope: exactly `manifest`, `signature` (base64url without
///    padding, 86 characters) and `key_id` (`fk_<profile>_<label>`); then a
///    `key_id` for another profile is refused before any verification.
/// 3. Ed25519 (RFC 8032) over the RFC 8785 bytes (`JCS`) of `manifest` as
///    received, with the built-in key.
/// 4. The manifest's schema, then its profile, channel, unique assets and a
///    version newer than the installed one.
public enum ReleaseFeedVerifier {
    /// The built-in feed key (Resources/feed-public-key.txt): standard
    /// base64 of the 32-byte Ed25519 public key; nil for anything else.
    public static func decodePublicKey(_ text: String) -> Data? {
        guard text.range(of: "^[A-Za-z0-9+/]{42}[AEIMQUYcgkosw048]=$", options: .regularExpression) != nil,
              let key = Data(base64Encoded: text), key.count == 32 else { return nil }
        return key
    }

    /// Verifies the envelope `raw` against the built-in `key` and `expected`.
    public static func verify(_ raw: Data, key: Data, expected: FeedExpectation) throws -> FeedManifest {
        guard case let .success(envelope) = StrictJSON.parse([UInt8](raw)) else { throw FeedError.schemaInvalid }
        guard case let .object(members) = envelope, members.count == 3,
              let manifest = envelope["manifest"], case .object = manifest,
              let signatureText = envelope["signature"]?.text,
              let keyID = envelope["key_id"]?.text,
              let keyProfile = keyIDProfile(keyID) else { throw FeedError.schemaInvalid }
        let signature = try decodeSignature(signatureText)
        guard keyProfile.utf8.elementsEqual(expected.profile.utf8) else { throw FeedError.keyIDRefused }
        guard let canonical = try? JCS.canonicalize(manifest) else { throw FeedError.schemaInvalid }
        guard let publicKey = try? Curve25519.Signing.PublicKey(rawRepresentation: key),
              publicKey.isValidSignature(signature, for: Data(canonical)) else { throw FeedError.signatureInvalid }

        let verified = try FeedManifest(manifest)
        guard verified.profile.utf8.elementsEqual(expected.profile.utf8) else { throw FeedError.profileMismatch }
        guard verified.channel == expected.channel else { throw FeedError.channelMismatch }
        try verified.checkUniqueAssets()
        guard AppUpdateMetadata.isNewer(verified.version, than: expected.installedVersion) else {
            throw FeedError.notNewer
        }
        return verified
    }

    /// `^[A-Za-z0-9_-]{85}[AQgw]$`: 64 bytes of base64url without padding,
    /// whose last character carries two data bits and four zero bits.
    static func decodeSignature(_ text: String) throws -> Data {
        let bytes = Array(text.utf8)
        let alphabet = { (byte: UInt8) in
            isAlphanumeric(byte) || byte == UInt8(ascii: "_") || byte == UInt8(ascii: "-")
        }
        guard bytes.count == 86, bytes.allSatisfy(alphabet), let last = bytes.last,
              "AQgw".utf8.contains(last) else { throw FeedError.schemaInvalid }
        let standard = String(decoding: bytes, as: UTF8.self)
            .replacingOccurrences(of: "-", with: "+")
            .replacingOccurrences(of: "_", with: "/") + "=="
        guard let signature = Data(base64Encoded: standard), signature.count == 64 else {
            throw FeedError.schemaInvalid
        }
        return signature
    }

    /// The profile a well-formed `key_id` (`^fk_[a-z0-9]{1,32}_[a-z0-9]{1,16}$`)
    /// names.
    static func keyIDProfile(_ keyID: String) -> String? {
        let bytes = Array(keyID.utf8)
        guard bytes.starts(with: Array("fk_".utf8)) else { return nil }
        let rest = bytes.dropFirst(3)
        guard let split = rest.firstIndex(of: UInt8(ascii: "_")) else { return nil }
        let profile = String(decoding: rest[..<split], as: UTF8.self)
        let label = rest[rest.index(after: split)...]
        let labelOK = (1...16).contains(label.count) && label.allSatisfy(isLowerAlphanumeric)
        return validProfile(profile) && labelOK ? profile : nil
    }
}

/// Whether a download may come from `url`: `https`, no user info, no
/// explicit port (not even `:443`), and a host that is one of the
/// manifest's `downloadHosts` exactly or the host of `site` (a streamed
/// download). Only a cloud feed's downloads are held to this; GitHub's stay
/// pinned to github.com.
public func feedDownloadHostAllowed(_ url: String, downloadHosts: [String], site: String) -> Bool {
    let bytes = Array(url.utf8)
    guard bytes.starts(with: Array("https://".utf8)) else { return false }
    let rest = bytes.dropFirst(8)
    let end = rest.firstIndex { $0 == UInt8(ascii: "/") || $0 == UInt8(ascii: "?") || $0 == UInt8(ascii: "#") }
    let authority = String(decoding: rest[..<(end ?? rest.endIndex)], as: UTF8.self)
    // A DNS name only: no user info, port, IP literal or percent escape.
    guard validHost(authority) else { return false }
    // The URL as a whole must parse, to the same host.
    guard URL(string: url)?.host == authority else { return false }
    let siteURL = URL(string: site)
    let siteHost = siteURL?.scheme == "https" ? siteURL?.host : nil
    return downloadHosts.contains(authority) || siteHost == authority
}

// MARK: Schema rules

private func isAlphanumeric(_ byte: UInt8) -> Bool {
    (UInt8(ascii: "0")...UInt8(ascii: "9")).contains(byte) || (UInt8(ascii: "a")...UInt8(ascii: "z")).contains(byte)
        || (UInt8(ascii: "A")...UInt8(ascii: "Z")).contains(byte)
}

private func isLowerAlphanumeric(_ byte: UInt8) -> Bool {
    (UInt8(ascii: "0")...UInt8(ascii: "9")).contains(byte) || (UInt8(ascii: "a")...UInt8(ascii: "z")).contains(byte)
}

private func isDigit(_ byte: UInt8) -> Bool {
    (UInt8(ascii: "0")...UInt8(ascii: "9")).contains(byte)
}

/// The object's members, if it has every `required` one and nothing but
/// those and the `optional` ones.
private func fields(_ value: FeedJSON, required: [String], optional: [String]) throws -> FeedJSON {
    guard case let .object(members) = value else { throw FeedError.schemaInvalid }
    let names = members.map { Array($0.name.utf8) }
    let known = Set((required + optional).map { Array($0.utf8) })
    guard names.allSatisfy(known.contains),
          required.allSatisfy({ value[$0] != nil }) else { throw FeedError.schemaInvalid }
    return value
}

/// A string value that is all ASCII, so an enum's `rawValue` comparison is
/// a byte comparison (Swift's `==` takes U+212A KELVIN SIGN for `K`).
private func ascii(_ value: FeedJSON?) -> String? {
    guard let text = value?.text, text.utf8.allSatisfy({ $0 < 0x80 }) else { return nil }
    return text
}

private func checked(_ value: FeedJSON?, _ valid: (String) -> Bool) throws -> String {
    guard let text = value?.text, valid(text) else { throw FeedError.schemaInvalid }
    return text
}

/// `^[a-z0-9]{1,32}$`, the profile id (also the `key_id` profile part).
func validProfile(_ profile: String) -> Bool {
    (1...32).contains(profile.utf8.count) && profile.utf8.allSatisfy(isLowerAlphanumeric)
}

/// `0|[1-9][0-9]*`.
private func plainInteger<C: Collection>(_ text: C) -> Bool where C.Element == UInt8 {
    guard let first = text.first, text.allSatisfy(isDigit) else { return false }
    return text.count == 1 || first != UInt8(ascii: "0")
}

/// `YYYY.M.N` on stable, `YYYY.M.N-beta.N` on beta.
private func validVersion(_ version: String, channel: FeedChannel) -> Bool {
    let bytes = Array(version.utf8)
    var release = bytes[...]
    var beta = false
    if let dash = bytes.firstIndex(of: UInt8(ascii: "-")) {
        let suffix = bytes[(dash + 1)...]
        let prefix = Array("beta.".utf8)
        guard suffix.starts(with: prefix), plainInteger(suffix.dropFirst(prefix.count)) else { return false }
        release = bytes[..<dash]
        beta = true
    }
    let parts = release.split(separator: UInt8(ascii: "."), omittingEmptySubsequences: false)
    guard parts.count == 3 else { return false }
    let (year, month, patch) = (parts[0], parts[1], parts[2])
    let monthOK = plainInteger(month) && month.count <= 2
        && (1...12).contains(Int(String(decoding: month, as: UTF8.self)) ?? 0)
    return year.count == 4 && year.allSatisfy(isDigit) && monthOK && plainInteger(patch)
        && beta == (channel == .beta)
}

/// `YYYY-MM-DDTHH:MM:SSZ`, a real date and time (a leap second allowed).
private func validPublishedAt(_ text: String) -> Bool {
    let bytes = Array(text.utf8)
    guard bytes.count == 20 else { return false }
    for (index, byte) in bytes.enumerated() {
        let wanted: UInt8?
        switch index {
        case 4, 7: wanted = UInt8(ascii: "-")
        case 10: wanted = UInt8(ascii: "T")
        case 13, 16: wanted = UInt8(ascii: ":")
        case 19: wanted = UInt8(ascii: "Z")
        default: wanted = nil
        }
        guard wanted.map({ byte == $0 }) ?? isDigit(byte) else { return false }
    }
    let number = { (range: Range<Int>) in bytes[range].reduce(0) { $0 * 10 + Int($1 - UInt8(ascii: "0")) } }
    let (year, month, day) = (number(0..<4), number(5..<7), number(8..<10))
    let (hour, minute, second) = (number(11..<13), number(14..<16), number(17..<19))
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
    let days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    guard (1...12).contains(month), (1...days[month - 1]).contains(day) else { return false }
    return hour <= 23 && minute <= 59 && second <= 60
}

/// A lower-case DNS name of at least two labels whose last label starts
/// with a letter, so no IP literal, port or upper case.
func validHost(_ host: String) -> Bool {
    let bytes = Array(host.utf8)
    let labels = bytes.split(separator: UInt8(ascii: "."), omittingEmptySubsequences: false)
    let labelOK = { (label: ArraySlice<UInt8>) in
        (1...63).contains(label.count)
            && label.allSatisfy { isLowerAlphanumeric($0) || $0 == UInt8(ascii: "-") }
            && label.first != UInt8(ascii: "-") && label.last != UInt8(ascii: "-")
    }
    guard bytes.count <= 253, labels.count >= 2, labels.allSatisfy(labelOK),
          let first = labels.last?.first else { return false }
    return (UInt8(ascii: "a")...UInt8(ascii: "z")).contains(first)
}

private func asset(_ value: FeedJSON) throws -> FeedAsset {
    let object = try fields(
        value, required: ["id", "name", "platform", "arch", "kind", "size", "sha256"], optional: ["os_floor"])
    guard let platform = ascii(object["platform"]).flatMap(FeedPlatform.init(rawValue:)),
          let arch = ascii(object["arch"]).flatMap(AppUpdateArchitecture.init(rawValue:)),
          let kind = ascii(object["kind"]).flatMap(FeedKind.init(rawValue:)),
          case let .integer(size) = object["size"], size >= 1, size.magnitude <= StrictJSON.maxSafeInteger else {
        throw FeedError.schemaInvalid
    }
    var floor: Int?
    if let value = object["os_floor"] {
        // 10...99, and (the schema's dependentSchemas) only on a macOS asset.
        guard platform == .macOS, case let .integer(number) = value, (10...99).contains(number) else {
            throw FeedError.schemaInvalid
        }
        floor = Int(number)
    }
    return FeedAsset(
        id: try checked(object["id"], validAssetID),
        name: try checked(object["name"], validAssetName),
        platform: platform,
        arch: arch,
        osFloor: floor,
        kind: kind,
        size: size,
        sha256: try checked(object["sha256"]) { hex in
            hex.utf8.count == 64 && hex.utf8.allSatisfy { isDigit($0) || (UInt8(ascii: "a")...UInt8(ascii: "f")).contains($0) }
        })
}

/// `^a_[0-9A-Za-z]{1,64}$`.
private func validAssetID(_ id: String) -> Bool {
    let bytes = Array(id.utf8)
    guard bytes.starts(with: Array("a_".utf8)) else { return false }
    let rest = bytes.dropFirst(2)
    return (1...64).contains(rest.count) && rest.allSatisfy(isAlphanumeric)
}

/// One path segment: dot-separated non-empty runs of `[A-Za-z0-9_+-]`,
/// starting with a letter or digit, at most 255 characters.
private func validAssetName(_ name: String) -> Bool {
    let bytes = Array(name.utf8)
    let runOK = { (run: ArraySlice<UInt8>) in
        !run.isEmpty && run.allSatisfy {
            isAlphanumeric($0) || $0 == UInt8(ascii: "_") || $0 == UInt8(ascii: "+") || $0 == UInt8(ascii: "-")
        }
    }
    guard bytes.count <= 255, let first = bytes.first, isAlphanumeric(first) else { return false }
    return bytes.split(separator: UInt8(ascii: "."), omittingEmptySubsequences: false).allSatisfy(runOK)
}
