import Security
import XCTest
@testable import AgentRelayiPad

/// Answers the Cloud calls a saved sign-in makes, with invented values. Any
/// other request (Relaycast after pairing) gets a 404, so nothing leaves the
/// test host.
final class RepairCloudURLProtocol: URLProtocol {
    static let workspaceID = "0f1e2d3c-4b5a-4968-8778-695a4b3c2d1e"
    static let relaycast = "https://relaycast.example"

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        let path = request.url?.path ?? ""
        let body: [String: Any]?
        switch path {
        case "/cloud/api/v1/auth/whoami":
            body = ["user": ["name": "Fixture Person"], "currentWorkspace": ["id": Self.workspaceID]]
        case "/cloud/api/v1/workspaces/\(Self.workspaceID)/resolve":
            body = ["relaycastApiKey": "rk_live_fixturefixturefixture01", "name": "Fixture Workspace",
                    "urls": ["relaycastUrl": Self.relaycast]]
        case "/cloud/api/v1/workspaces/\(Self.workspaceID)/messaging/identity/credential":
            body = ["identity": ["name": "fixture-person", "token": "at_live_fixture_new", "agentId": "agent-fixture",
                                 "relaycastUrl": Self.relaycast]]
        default:
            body = nil
        }
        let data = (try? JSONSerialization.data(withJSONObject: body ?? [:])) ?? Data()
        let response = HTTPURLResponse(url: request.url!, statusCode: body == nil ? 404 : 200, httpVersion: nil,
                                       headerFields: ["Content-Type": "application/json"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: data)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}

/// Pairing again with another messaging identity in the same workspace, as
/// a saved Cloud sign-in finishes it: the session events of the old identity
/// leave memory and disk.
@MainActor
final class MessagingIdentityRepairTests: XCTestCase {
    private static let service = "com.agentworkforce.relayipad"
    private static let secureKeys = ["cloudRefreshToken", "cloudAccessToken", "cloudAccessExpiresAt", "agentToken",
                                     "workspaceKey", "relaycastURL", "cloudPendingDeviceCode", "cloudPendingExpiresAt",
                                     "cloudPendingUserCode", "cloudPendingURL"]
    private static let defaultsKeys = ["relay.messagingIdentitySource", "relay.workspaceID", "relay.mobileName",
                                       "relay.accountName", "relay.workspaceName", "relay.mobileIdentity"]
    private var savedSecure: [String: String?] = [:]
    private var savedDefaults: [String: Any?] = [:]

    private let agent = RelayAgent(relay: ["name": "builder", "status": "active",
                                           "metadata": ["session_id": "s1", "source": "relay-desktop"]])

    // The app's keychain item, written the way the app writes it (with the
    // simulator's fallback when the test host has no keychain entitlement).
    private func query(_ key: String) -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: Self.service,
         kSecAttrAccount as String: key]
    }
    private func readSecure(_ key: String) -> String? {
        var item: CFTypeRef?
        var search = query(key)
        search[kSecReturnData as String] = true
        search[kSecMatchLimit as String] = kSecMatchLimitOne
        let status = SecItemCopyMatching(search as CFDictionary, &item)
        if status == errSecMissingEntitlement { return UserDefaults.standard.string(forKey: "relay.simulator.\(key)") }
        return (item as? Data).flatMap { String(data: $0, encoding: .utf8) }
    }
    private func writeSecure(_ value: String?, for key: String) {
        SecItemDelete(query(key) as CFDictionary)
        UserDefaults.standard.removeObject(forKey: "relay.simulator.\(key)")
        guard let value else { return }
        var item = query(key)
        item[kSecValueData as String] = Data(value.utf8)
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        if SecItemAdd(item as CFDictionary, nil) == errSecMissingEntitlement {
            UserDefaults.standard.set(value, forKey: "relay.simulator.\(key)")
        }
    }

    override func setUp() async throws {
        for key in Self.secureKeys { savedSecure.updateValue(readSecure(key), forKey: key) }
        for key in Self.defaultsKeys { savedDefaults.updateValue(UserDefaults.standard.object(forKey: key), forKey: key) }
        for key in Self.secureKeys { writeSecure(nil, for: key) }
        URLProtocol.registerClass(RepairCloudURLProtocol.self)
    }

    override func tearDown() async throws {
        URLProtocol.unregisterClass(RepairCloudURLProtocol.self)
        for (key, value) in savedSecure { writeSecure(value, for: key) }
        for (key, value) in savedDefaults {
            if let value { UserDefaults.standard.set(value, forKey: key) } else { UserDefaults.standard.removeObject(forKey: key) }
        }
    }

    func testASavedSignInWithAnotherIdentityClearsSessionEventsAndTheDiskCache() async {
        // Signed in to the same workspace under an older, device-made identity.
        let expires = ISO8601DateFormatter().string(from: Date().addingTimeInterval(3_600))
        writeSecure("refresh-fixture", for: "cloudRefreshToken")
        writeSecure("access-fixture", for: "cloudAccessToken")
        writeSecure(expires, for: "cloudAccessExpiresAt")
        writeSecure("at_live_fixture_old", for: "agentToken")
        writeSecure(RepairCloudURLProtocol.relaycast, for: "relaycastURL")
        UserDefaults.standard.set("device", forKey: "relay.messagingIdentitySource")
        UserDefaults.standard.set(RepairCloudURLProtocol.workspaceID, forKey: "relay.workspaceID")

        let spy = SpySessionEventSnapshots()
        let client = RelayClient(sessionEventSnapshots: spy, sessionEventFetch: { _, _ in
            SessionEventPage(sessionID: "s1", events: [
                SessionEvent(key: .init(source: "fixture", machineID: "m", kind: "assistant", eventID: "e1"),
                             ts: Date(timeIntervalSince1970: 1_790_000_000), kind: "assistant",
                             actorRole: "assistant", content: "synthetic")
            ], nextCursor: nil)
        })
        // The workspace stays the same, so only the identity change can clear.
        client.cloudWorkspaceID = RepairCloudURLProtocol.workspaceID
        await client.refreshSessionEvents(for: agent)
        XCTAssertEqual(client.sessionEvents["s1"]?.events.count, 1)
        let clearsBefore = spy.removeAllCount

        await client.resumePendingSignIn()

        XCTAssertEqual(UserDefaults.standard.string(forKey: "relay.messagingIdentitySource"), "cloud")
        XCTAssertEqual(readSecure("agentToken"), "at_live_fixture_new", "the new identity was paired")
        XCTAssertEqual(client.cloudWorkspaceID, RepairCloudURLProtocol.workspaceID)
        XCTAssertTrue(client.sessionEvents.isEmpty, "the old identity's events left memory")
        XCTAssertEqual(spy.removeAllCount, clearsBefore + 1, "and the disk")
    }
}
