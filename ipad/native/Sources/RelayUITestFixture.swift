#if DEBUG
import Foundation

/// Canned Relaycast responses for UI tests, enabled only in Debug builds with
/// the `-RelayUITestConversations` launch argument. It lets the Messages list
/// be exercised on a simulator that is not signed in. `-RelayUITestNewMessage`
/// gives one conversation a newer incoming message, and `-RelayUITestReset`
/// forgets the fixture account's hidden conversations.
enum RelayUITestFixture {
    static let enabled = ProcessInfo.processInfo.arguments.contains("-RelayUITestConversations")
    private static let me = "ui-test-person"

    static func prepare() {
        guard enabled else { return }
        UserDefaults.standard.set(me, forKey: "relay.mobileName")
        UserDefaults.standard.set("ui-test-workspace", forKey: "relay.workspaceID")
        UserDefaults.standard.set("UI Test", forKey: "relay.workspaceName")
        UserDefaults.standard.set("cloud", forKey: "relay.messagingIdentitySource")
        if ProcessInfo.processInfo.arguments.contains("-RelayUITestReset") {
            UserDefaults.standard.removeObject(forKey: "relay.mobileUnread.ui-test-workspace.\(me).hidden")
        }
    }

    static func response(_ path: String, method: String = "GET", body: JSONObject? = nil) -> Any? {
        guard enabled else { return nil }
        if let handoff = MobileHandoffFixture.response(path, method: method, body: body) { return handoff }
        if path == "/dm/conversations" {
            let newer = ProcessInfo.processInfo.arguments.contains("-RelayUITestNewMessage")
            return [
                thread("dm_reviewer", peer: "reviewer", messageID: newer ? "m_reviewer_2" : "m_reviewer_1",
                       text: newer ? "One more thing on the PR" : "Review is done"),
                thread("dm_builder", peer: "builder", messageID: "m_builder_1", text: "Build passed"),
                thread("dm_sam", peer: "sam-lee", messageID: "m_sam_1", text: "Lunch at noon?"),
            ]
        }
        // Newest first, as Relaycast returns them. The builder has read the
        // person's last message; the reviewer has not.
        if path.hasPrefix("/dm/dm_builder/messages") {
            return [message("m_me_builder", from: me, text: "Ship it"), message("m_builder_1", from: "builder", text: "Build passed")]
        }
        if path.hasPrefix("/dm/dm_reviewer/messages") {
            return [message("m_me_reviewer", from: me, text: "Thanks"), message("m_reviewer_1", from: "reviewer", text: "Review is done")]
        }
        if path == "/messages/m_me_builder/readers" {
            return [["agent_name": "builder", "agent_id": "a_builder", "read_at": "2026-10-08T12:00:00.000Z"]]
        }
        if path.hasSuffix("/readers") { return [JSONObject]() }
        if path.hasPrefix("/dm/") { return [JSONObject]() }
        // A group chat with a person and an agent; the person has joined
        // #launch but not #general.
        if path == "/channels" {
            return [["name": "launch", "topic": "Launch prep", "member_count": 3],
                    ["name": "general", "member_count": 12]] as [JSONObject]
        }
        if path == "/agents/\(me)" {
            return ["name": me, "type": "human", "channels": [["name": "launch"]]] as JSONObject
        }
        if path.hasPrefix("/channels/launch/messages") {
            return [message("m_launch_4", from: "pat-doe", text: "Count me in"),
                    message("m_launch_3", from: me, text: "Shipping Friday"),
                    message("m_launch_2", from: "builder", text: "Build is green"),
                    message("m_launch_1", from: "sam-lee", text: "Are we on track?")]
        }
        if path.hasPrefix("/channels/") { return [JSONObject]() }
        // A person who has since left the roster.
        if path == "/agents/pat-doe" {
            return ["name": "pat-doe", "type": "human", "persona": "Pat Doe", "status": "offline"] as JSONObject
        }
        // Another person in the workspace, as Cloud registers them.
        if path == "/agents/sam-lee" {
            return ["name": "sam-lee", "type": "human", "persona": "Sam Lee",
                    "address": "sam-lee@cloud-1234567890", "status": "online",
                    "metadata": ["source": "cloud-person", "role": "person"]] as JSONObject
        }
        if path == "/agents" || path == "/agents?status=active" || path == "/agents?status=all" {
            let departed = ProcessInfo.processInfo.arguments.contains("-RelayUITestDepartedPeer")
            let mixed = ProcessInfo.processInfo.arguments.contains("-RelayUITestMixedPresence")
            let offlinePerson = ProcessInfo.processInfo.arguments.contains("-RelayUITestOfflinePerson")
            let reviewer: JSONObject = ["name": "reviewer", "status": departed ? "idle" : mixed ? "online" : "active",
                                         "address": "reviewer@test-mini-123",
                                         "metadata": ["host": "Test Mini.local", "source": "relay-desktop",
                                                      "session_id": "session-reviewer", "cli": "codex"]]
            var roster: [JSONObject] = [
                ["name": me, "type": "human", "persona": "My Own Profile", "status": "active"],
                ["name": "reviewer-before-rename", "status": "active",
                 "metadata": ["renamed_to": "reviewer", "source": "relay-desktop"]],
                ["name": "builder", "status": mixed ? "waiting" : "active", "address": "builder@test-mini-456",
                 "metadata": ["host": "test-mini", "session_id": "session-builder", "cli": "claudecode"]],
                ["name": "sam-lee", "type": "human", "persona": "Sam Lee",
                 "address": "sam-lee@cloud-1234567890", "status": offlinePerson ? "offline" : "active"],
            ]
            if !departed || path != "/agents?status=active" { roster.append(reviewer) }
            return (path == "/agents?status=active" ? roster.filter { $0["status"] as? String == "active" } : roster).map {
                var row = $0
                row["last_seen"] = Date.ISO8601FormatStyle(includingFractionalSeconds: true).format(Date())
                return row
            }
        }
        if path.hasPrefix("/agents") || path == "/nodes" { return [JSONObject]() }
        return JSONObject()
    }

    static func peopleResponse() -> JSONObject? {
        guard enabled else { return nil }
        // An unsafe URL deterministically exercises the initials fallback
        // without allowing a UI test to contact a profile provider.
        let photo = ProcessInfo.processInfo.arguments.contains("-RelayUITestProfilePhoto")
        return ["people": [["handle": "sam-lee", "displayName": "Sam Lee",
                             "avatarUrl": photo ? "https://relay-profile-fixture.invalid/sam.png"
                                                : "http://example.invalid/sam.png"]]]
    }

    static var delaysPeople: Bool {
        enabled && ProcessInfo.processInfo.arguments.contains("-RelayUITestDelayedProfilePhoto")
    }

    static var photoProtocolClasses: [AnyClass]? {
        enabled ? [RelayUITestPhotoProtocol.self] : nil
    }

    static func summarySearchResponse(_ query: String, cursor: String?) -> JSONObject? {
        guard enabled else { return nil }
        let normalized = SummarySearchQuery.normalize(query).lowercased()
        let rows: [JSONObject] = [
            ["sessionId": "session-reviewer", "source": "codex", "summary": "Mobile release review",
             "agentName": "reviewer",
             "search": ["snippet": "Confirmed the release checklist and CI status before distribution.",
                        "matchedBy": ["keyword", "semantic"]],
             "createdAt": "2026-10-18T17:30:00.000Z", "lastTs": "2026-10-18T18:30:00.000Z",
             "sessionURL": "https://agentrelay.com/sessions/session-reviewer"],
            ["sessionId": "session-builder", "source": "claudecode", "summary": "iOS summary search",
             "agentName": "builder",
             "search": ["snippet": "The iOS build passed after adding coverage for session summaries.",
                        "matchedBy": ["semantic"]],
             "createdAt": "2026-10-17T16:15:00.000Z", "lastTs": "2026-10-17T17:15:00.000Z",
             "sessionURL": "https://agentrelay.com/sessions/session-builder"],
        ]
        let matches = rows.filter {
            normalized == "release" ||
                (($0["summary"] as? String) ?? "").localizedCaseInsensitiveContains(normalized) ||
                ((($0["search"] as? JSONObject)?["snippet"] as? String) ?? "")
                    .localizedCaseInsensitiveContains(normalized)
        }
        let page: [JSONObject]
        let next: Any
        if cursor == nil, matches.count > 1 {
            page = [matches[0]]
            next = "summary-page+2"
        } else if cursor == "summary-page+2" {
            // Repeat the boundary row so the UI test also proves de-duplication.
            page = matches.isEmpty ? [] : [matches[0]] + Array(matches.dropFirst())
            next = NSNull()
        } else {
            page = matches
            next = NSNull()
        }
        return ["items": page, "members": [], "nextCursor": next, "error": false,
                "ranking": "hybrid", "now": 1_760_860_800_000]
    }

    static var summarySearchIsOffline: Bool {
        enabled && ProcessInfo.processInfo.arguments.contains("-RelayUITestSummarySearchOffline")
    }

    static var summarySearchFails: Bool {
        enabled && ProcessInfo.processInfo.arguments.contains("-RelayUITestSummarySearchError")
    }

    static var delaysSummarySearch: Bool {
        enabled && ProcessInfo.processInfo.arguments.contains("-RelayUITestSummarySearchDelay")
    }

    private static func message(_ id: String, from sender: String, text: String) -> JSONObject {
        ["id": id, "agent_name": sender, "agent_id": "a_\(sender)", "text": text, "created_at": "2026-10-08T12:00:00.000Z"]
    }

    private static func thread(_ id: String, peer: String, messageID: String, text: String) -> JSONObject {
        [
            "id": id,
            "participants": [["agent_name": me, "agent_id": "a_me"], ["agent_name": peer, "agent_id": "a_\(peer)"]],
            "last_message": ["id": messageID, "agent_id": "a_\(peer)", "agent_name": peer, "text": text],
        ]
    }
}

/// No UI fixture can fetch a real profile image or share cookies externally.
private final class RelayUITestPhotoProtocol: URLProtocol {
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        guard let url = request.url, url.host == "relay-profile-fixture.invalid" else {
            client?.urlProtocol(self, didFailWithError: URLError(.resourceUnavailable))
            return
        }
        let png = Data(base64Encoded: "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAFklEQVR4nGNkYPjPwMDAxMDAwMDAAAALHwEDmIWXfgAAAABJRU5ErkJggg==")!
        let response = HTTPURLResponse(url: url, statusCode: 200, httpVersion: "HTTP/1.1",
                                       headerFields: ["Content-Type": "image/png"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: png)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}
#endif

