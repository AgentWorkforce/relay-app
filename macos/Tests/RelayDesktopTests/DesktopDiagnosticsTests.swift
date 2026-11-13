import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

final class DesktopDiagnosticsTests: XCTestCase {
    private func snapshot(verification: String = "verified", push: [String: Any] = [:], found: Bool = true) throws -> Status {
        var json = FakeCore.snapshot(revision: 1, account: FakeCore.signedIn, push: push)
        var workspace = json["workspace"] as! [String: Any]
        workspace["verification"] = verification
        workspace["state"] = found ? "found" : "missing"
        json["workspace"] = workspace
        return try JSONDecoder().decode(Status.self, from: JSONSerialization.data(withJSONObject: json))
    }

    func testRelaycastAuthenticationIsNotConfusedWithAnActiveConnection() throws {
        XCTAssertEqual(DesktopDiagnostics.relaycast(try snapshot(), connected: true), "Ready · no active channels")
        XCTAssertEqual(DesktopDiagnostics.relaycast(try snapshot(found: false), connected: true), "Not configured")
        XCTAssertEqual(DesktopDiagnostics.relaycast(try snapshot(verification: "rejected"), connected: true), "Credentials rejected")
        XCTAssertEqual(DesktopDiagnostics.relaycast(try snapshot(verification: "unreachable"), connected: true), "Unreachable")
        XCTAssertEqual(DesktopDiagnostics.relaycast(try snapshot(push: ["a": ["state": "live"]]), connected: false), "Unavailable · service disconnected")
    }

    func testMixedAndReconnectingChannelsAreNotReportedAsFullyConnected() throws {
        let mixed: [String: Any] = ["a": ["state": "live"], "b": ["state": "waiting", "retry_in_ms": 500]]
        XCTAssertEqual(DesktopDiagnostics.relaycast(try snapshot(push: mixed), connected: true), "Partially connected · 1/2 live channels")
        XCTAssertEqual(DesktopDiagnostics.relaycast(try snapshot(push: ["a": ["state": "connecting"]]), connected: true), "Reconnecting · no live channels")
    }

    func testEndpointsExcludeCredentialsAndQuerySecrets() {
        XCTAssertEqual(DesktopDiagnostics.endpoint("https://user:secret@cast.example.com/api?token=secret#secret"), "https://cast.example.com/api")
        XCTAssertNil(DesktopDiagnostics.endpoint("not a URL"))
    }

    func testOldServiceSnapshotsDecodeWithoutBuildMetadata() throws {
        let old = try snapshot()
        XCTAssertNil(old.daemon.historyVersion)
        XCTAssertNil(old.daemon.buildRevision)
    }

    func testNewServiceMetadataReachesDiagnosticsContract() throws {
        var json = FakeCore.snapshot(revision: 1, account: ["state": "signed_out"])
        var daemon = json["daemon"] as! [String: Any]
        daemon["history_version"] = "0.32.3"
        daemon["build_revision"] = "abc123"
        json["daemon"] = daemon
        let decoded = try JSONDecoder().decode(Status.self, from: JSONSerialization.data(withJSONObject: json))
        XCTAssertEqual(decoded.daemon.historyVersion, "0.32.3")
        XCTAssertEqual(decoded.daemon.buildRevision, "abc123")
    }

    func testDiagnosticsNameTheCoreCratesTheAppWasBuiltWith() throws {
        let json = #"{"agent-relay-node":{"rev":"0123456789abcdef0123456789abcdef01234567"},"relaycast":{"version":"10.0.1"}}"#
        let crates = try XCTUnwrap(AppInfo.coreCrates(from: Data(json.utf8)))
        XCTAssertEqual(crates, AppInfo.CoreCrates(agentRelayNode: "0123456789abcdef0123456789abcdef01234567", relaycast: "10.0.1"))
        let report = DesktopDiagnostics.report([DesktopDiagnostics.Section(title: "Build", rows: DesktopDiagnostics.crateRows(crates))])
        XCTAssertEqual(report, DesktopDiagnostics.report([DesktopDiagnostics.Section(title: "Build", rows: [
            DesktopDiagnostics.Row("agent-relay-node crate", "0123456789ab"),
            DesktopDiagnostics.Row("relaycast crate", "10.0.1"),
        ])]))
        // About's Build section has them right after the bundled service.
        let build = DesktopDiagnostics.buildRows(nil, crates: crates)
        let bundled = try XCTUnwrap(build.firstIndex { $0.label == "Bundled service" })
        XCTAssertEqual(build[(bundled + 1)...].prefix(3).map { "\($0.label): \($0.value)" }, [
            "agent-relay-node crate: 0123456789ab",
            "relaycast crate: 10.0.1",
            "RelayHistory (ai-hist): Unavailable",
        ])
        let unknown = DesktopDiagnostics.buildRows(nil, crates: nil)
        XCTAssertEqual(unknown[(bundled + 1)...].prefix(2).map { "\($0.label): \($0.value)" }, [
            "agent-relay-node crate: Unavailable",
            "relaycast crate: Unavailable",
        ])
        XCTAssertEqual(DesktopDiagnostics.crateRows(nil).map(\.value), ["Unavailable", "Unavailable"])
        XCTAssertEqual(DesktopDiagnostics.crateRows(nil).map(\.label), ["agent-relay-node crate", "relaycast crate"])
        for malformed in ["", "{}", #"{"agent-relay-node":{"rev":""},"relaycast":{"version":"10.0.1"}}"#, #"{"agent-relay-node":"abc","relaycast":{"version":"10.0.1"}}"#] {
            XCTAssertNil(AppInfo.coreCrates(from: Data(malformed.utf8)), malformed)
        }
    }
}
