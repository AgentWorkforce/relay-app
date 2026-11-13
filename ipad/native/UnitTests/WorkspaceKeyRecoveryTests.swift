import XCTest
@testable import AgentRelayiPad

/// Placeholder keys only: these never reach a server.
private let refusedKey = "rk_live_0000000000000000000000000000000000000000"
private let rotatedKey = "rk_live_rotatedRotatedRotatedRotated_0123456789"
private let relaycast = "https://cast.agentrelay.com"

private func resolution(key: String, relaycastURL: String? = relaycast) -> [String: Any] {
    var data: [String: Any] = ["relaycastApiKey": key, "name": "Review"]
    if let relaycastURL { data["urls"] = ["relaycastUrl": relaycastURL] }
    return ["data": data]
}

/// Records what a recovery asked Cloud and saved.
@MainActor private final class FakeCloud {
    var answer: [String: Any]
    var rejectsSession = false
    var unreachableResolves = 0
    var resolves = 0
    var saved: [String] = []
    var gate: CheckedContinuation<Void, Never>?
    var holdResolve = false

    init(answer: [String: Any]) { self.answer = answer }

    func resolve() async throws -> [String: Any] {
        resolves += 1
        if holdResolve { await withCheckedContinuation { gate = $0 } }
        if rejectsSession { throw WorkspaceKeyRecovery.CloudSessionRejected() }
        if unreachableResolves > 0 {
            unreachableResolves -= 1
            throw URLError(.notConnectedToInternet)
        }
        return answer
    }

    func save(_ refused: String, _ replacement: String) throws -> Bool {
        saved.append(replacement)
        return true
    }

    func recover(_ recovery: WorkspaceKeyRecovery, refused: String = refusedKey) async throws -> String {
        try await recovery.replacement(
            for: refused, currentBaseURL: relaycast, defaultBaseURL: relaycast,
            resolve: { try await self.resolve() },
            save: { try self.save($0, $1) })
    }
}

@MainActor final class WorkspaceKeyRecoveryTests: XCTestCase {
    func testRotatedKeyIsResolvedSavedAndReturnedOnce() async throws {
        let cloud = FakeCloud(answer: resolution(key: rotatedKey))
        let recovery = WorkspaceKeyRecovery()
        let replacement = try await cloud.recover(recovery)
        XCTAssertEqual(replacement, rotatedKey)
        XCTAssertEqual(cloud.saved, [rotatedKey])
        XCTAssertEqual(cloud.resolves, 1)

        // The same refused key is never recovered twice: a request that still
        // carries it after recovery fails instead of asking Cloud again.
        do {
            _ = try await cloud.recover(recovery)
            XCTFail("A recovered key must not be recovered again")
        } catch let error as WorkspaceKeyRecoveryError {
            XCTAssertEqual(error, .keyRefused)
        }
        XCTAssertEqual(cloud.resolves, 1)
    }

    func testConcurrentRefusalsShareOneResolve() async throws {
        let cloud = FakeCloud(answer: resolution(key: rotatedKey))
        cloud.holdResolve = true
        let recovery = WorkspaceKeyRecovery()
        async let first = cloud.recover(recovery)
        async let second = cloud.recover(recovery)
        while cloud.gate == nil { await Task.yield() }
        cloud.gate?.resume()
        let keys = try await [first, second]
        XCTAssertEqual(keys, [rotatedKey, rotatedKey])
        XCTAssertEqual(cloud.resolves, 1)
        XCTAssertEqual(cloud.saved, [rotatedKey])
    }

    func testCloudReturningTheRefusedKeyFailsWithoutALoop() async {
        let cloud = FakeCloud(answer: resolution(key: refusedKey))
        let recovery = WorkspaceKeyRecovery()
        for _ in 0..<3 {
            do {
                _ = try await cloud.recover(recovery)
                XCTFail("The refused key must not be accepted back")
            } catch let error as WorkspaceKeyRecoveryError {
                XCTAssertEqual(error, .keyRefused)
            } catch {
                XCTFail("Unexpected error \(error)")
            }
        }
        XCTAssertEqual(cloud.resolves, 1)
        XCTAssertTrue(cloud.saved.isEmpty)
    }

    func testDifferentRelaycastOriginIsRefused() async {
        for origin in ["https://other.example", "https://cast.agentrelay.com.evil.example"] {
            let cloud = FakeCloud(answer: resolution(key: rotatedKey, relaycastURL: origin))
            do {
                _ = try await cloud.recover(WorkspaceKeyRecovery())
                XCTFail("A key for \(origin) must be refused")
            } catch let error as WorkspaceKeyRecoveryError {
                XCTAssertEqual(error, .differentServer)
            } catch {
                XCTFail("Unexpected error \(error)")
            }
            XCTAssertTrue(cloud.saved.isEmpty)
        }
    }

    func testInvalidResolutionsAreRefusedLikeSignIn() async {
        for answer in [
            resolution(key: "rk_test_11111111111111111111111111111111"),
            resolution(key: "rk_live_YOUR_KEY"),
            resolution(key: "rk_live_has spaces in it 0123456789"),
            resolution(key: rotatedKey, relaycastURL: "http://cast.agentrelay.com"),
            resolution(key: rotatedKey, relaycastURL: "https://user@cast.agentrelay.com"),
            resolution(key: rotatedKey, relaycastURL: "https://cast.agentrelay.com/v1"),
        ] {
            let cloud = FakeCloud(answer: answer)
            do {
                _ = try await cloud.recover(WorkspaceKeyRecovery())
                XCTFail("An invalid resolution must be refused")
            } catch {
                XCTAssertTrue(cloud.saved.isEmpty)
            }
        }
    }

    func testRejectedCloudSessionAsksToSignInAgain() async {
        let cloud = FakeCloud(answer: resolution(key: rotatedKey))
        cloud.rejectsSession = true
        do {
            _ = try await cloud.recover(WorkspaceKeyRecovery())
            XCTFail("A rejected session cannot recover the key")
        } catch let error as WorkspaceKeyRecoveryError {
            XCTAssertEqual(error, .signInAgain)
            XCTAssertTrue(error.localizedDescription.contains("Sign in again"))
        } catch {
            XCTFail("Unexpected error \(error)")
        }
        XCTAssertTrue(cloud.saved.isEmpty)
    }

    func testUnreachableCloudCanBeRetriedLater() async throws {
        let cloud = FakeCloud(answer: resolution(key: rotatedKey))
        cloud.unreachableResolves = 1
        let recovery = WorkspaceKeyRecovery()
        do {
            _ = try await cloud.recover(recovery)
            XCTFail("An unreachable Cloud cannot recover the key")
        } catch {
            XCTAssertTrue(cloud.saved.isEmpty)
        }
        let key = try await cloud.recover(recovery)
        XCTAssertEqual(key, rotatedKey)
        XCTAssertEqual(cloud.resolves, 2)
    }

    func testResolutionDefaultsToTheDefaultRelaycastServer() throws {
        let resolved = try WorkspaceKeyResolution(resolution: resolution(key: rotatedKey, relaycastURL: nil),
                                                  defaultBaseURL: relaycast + "/")
        XCTAssertEqual(resolved.baseURL, relaycast)
        XCTAssertEqual(resolved.workspaceName, "Review")
    }
}
