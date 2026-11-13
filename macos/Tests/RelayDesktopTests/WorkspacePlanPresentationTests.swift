@testable import RelayDesktop
@testable import RelayDesktopCore
import XCTest

final class WorkspacePlanPresentationTests: XCTestCase {
    private let intro = WorkspacePlan(
        key: "intro",
        limits: PlanLimits(agents: 15, devices: 5, humans: 2, sessionRetentionDays: 7),
        name: "Intro",
        upgradeUrl: "https://agentrelay.com/cloud/dashboard/settings#billing"
    )
    private let enterprise = WorkspacePlan(
        key: "enterprise",
        limits: PlanLimits(agents: nil, devices: nil, humans: nil, sessionRetentionDays: 365),
        name: "Enterprise"
    )

    func testPlanLineWordingMatchesLinux() {
        XCTAssertEqual(WorkspacePlanPresentation.manageAction, "Manage plan")
        XCTAssertEqual(WorkspacePlanPresentation.upgradeAction, "Upgrade plan")
        XCTAssertEqual(WorkspacePlanPresentation.title(intro), "Intro plan")
        XCTAssertEqual(WorkspacePlanPresentation.summary(intro, usage: WorkspaceUsage(agents: 12, humans: 1)),
                       "12 of 15 agents · 1 of 2 humans · up to 5 devices · 7-day session history")
        XCTAssertEqual(WorkspacePlanPresentation.summary(intro, usage: nil),
                       "up to 15 agents · up to 2 humans · up to 5 devices · 7-day session history")
        XCTAssertEqual(WorkspacePlanPresentation.summary(enterprise, usage: WorkspaceUsage(agents: 12, humans: 1)),
                       "unlimited agents · unlimited humans · unlimited devices · 365-day session history")
        XCTAssertEqual(WorkspacePlanPresentation.planName("pro"), "Pro")
    }

    func testManagePlanPrefersThePlansLinkAndFallsBackToBilling() {
        XCTAssertEqual(WorkspacePlanPresentation.manageURL(intro, siteURL: "https://agentrelay.com")?.absoluteString,
                       "https://agentrelay.com/cloud/dashboard/settings#billing")
        XCTAssertEqual(WorkspacePlanPresentation.manageURL(enterprise, siteURL: "https://agentrelay.com")?.absoluteString,
                       "https://agentrelay.com/cloud/dashboard/settings?reason=relaycast-plan-limit#billing")
        XCTAssertEqual(WorkspacePlanPresentation.manageURL(nil, siteURL: "http://local.test:3000")?.absoluteString,
                       "http://local.test:3000/cloud/dashboard/settings?reason=relaycast-plan-limit#billing")
        let insecure = WorkspacePlan(key: "intro", limits: PlanLimits(), name: "Intro", upgradeUrl: "http://evil.test/billing")
        XCTAssertEqual(WorkspacePlanPresentation.manageURL(insecure, siteURL: "https://agentrelay.com")?.absoluteString,
                       "https://agentrelay.com/cloud/dashboard/settings?reason=relaycast-plan-limit#billing")
    }

    func testAReachedCapNamesTheCapTheCountersAndThePlan() {
        let agents = PlanLimitExceeded(current: 15, limit: .agents, max: 15, plan: "intro")
        XCTAssertEqual(WorkspacePlanPresentation.limitTitle(agents), "Agent limit reached")
        XCTAssertEqual(WorkspacePlanPresentation.limitMessage(agents),
                       "This workspace is using 15 of 15 agents on the Intro plan. Upgrade the plan to add more.")
        XCTAssertEqual(WorkspacePlanPresentation.limitTitle(PlanLimitExceeded(current: 5, limit: .devices, max: 5, plan: "intro")), "Device limit reached")
        let humans = PlanLimitExceeded(current: 10, limit: .humans, max: 10, plan: "pro")
        XCTAssertEqual(WorkspacePlanPresentation.limitTitle(humans), "Human limit reached")
        XCTAssertEqual(WorkspacePlanPresentation.limitMessage(humans),
                       "This workspace is using 10 of 10 humans on the Pro plan. Upgrade the plan to add more.")
        XCTAssertEqual(WorkspacePlanPresentation.limitTitle(PlanLimitExceeded(current: 1, limit: .other, max: 1, plan: "pro")), "Plan limit reached")
    }

    func testTeamSaysWhenEveryHumanSeatIsTaken() {
        XCTAssertEqual(WorkspacePlanPresentation.humansAtCap(intro, usage: WorkspaceUsage(humans: 2)),
                       "Your workspace has all 2 humans the Intro plan allows. Upgrade the plan to invite more.")
        XCTAssertNil(WorkspacePlanPresentation.humansAtCap(intro, usage: WorkspaceUsage(humans: 1)))
        XCTAssertNil(WorkspacePlanPresentation.humansAtCap(intro, usage: nil), "not counted yet")
        XCTAssertNil(WorkspacePlanPresentation.humansAtCap(enterprise, usage: WorkspaceUsage(humans: 2)), "no cap")
        XCTAssertNil(WorkspacePlanPresentation.humansAtCap(nil, usage: WorkspaceUsage(humans: 2)), "no plan")
    }

    func testHistorySaysWhatRetentionLeftOut() {
        let seven = HistoryRetention(cutoff: "2026-10-18T00:00:00Z", days: 7)
        XCTAssertEqual(WorkspacePlanPresentation.retentionNotice(seven, plan: intro),
                       "Sessions older than 7 days aren't available on the Intro plan.")
        XCTAssertEqual(WorkspacePlanPresentation.retentionNotice(seven, plan: nil),
                       "Sessions older than 7 days aren't available on your plan.")
        XCTAssertEqual(WorkspacePlanPresentation.retentionNotice(HistoryRetention(cutoff: "", days: 1), plan: intro),
                       "Sessions older than 1 day aren't available on the Intro plan.")
    }

    func testTheContractReadsAnOlderCoreWithoutPlanFields() throws {
        let decoder = JSONDecoder()
        let workspace = try decoder.decode(WorkspaceStatus.self, from: Data("""
        {"state": "found", "base_url": "https://cast.agentrelay.com", "workspace_id": "rw_1",
         "workspace_name": "Acme", "verification": "verified", "checked_at_ms": 1}
        """.utf8))
        XCTAssertNil(workspace.plan)
        XCTAssertNil(workspace.usage)
        let page = try decoder.decode(HistoryPage.self, from: Data("{\"events\": [], \"next_cursor\": null}".utf8))
        XCTAssertNil(page.retention)
        let refused = try decoder.decode(ErrorData.self, from: Data("""
        {"kind": "plan_limit_exceeded", "retryable": false,
         "plan_limit": {"limit": "channels", "max": 3, "current": 3, "plan": "pro"}}
        """.utf8))
        XCTAssertEqual(refused.kind, .planLimitExceeded)
        XCTAssertEqual(refused.planLimit?.limit, .other, "a cap this build does not know reads as other")
    }

    @MainActor
    func testAFailedCallCarriesTheTypedRefusalToTheModel() async throws {
        let core = FakeCore()
        defer { core.stop() }
        core.on("agents.register") { _ in
            throw FakeCore.Failure(kind: "plan_limit_exceeded",
                                   message: "Could not register @api on Agent Relay. This workspace is using 15 of 15 agents on the Intro plan. Upgrade the plan to add more.",
                                   data: ["plan_limit": ["limit": "agents", "max": 15, "current": 15, "plan": "intro"]])
        }
        let client = CoreClient(socketPath: core.path)
        do {
            _ = try await client.agentsRegister(RegisterParams(name: "api", target: Target(id: "thread-1", kind: .codex)))
            XCTFail("expected the refusal")
        } catch let error as CoreError {
            XCTAssertEqual(error.kind, "plan_limit_exceeded")
            XCTAssertEqual(error.planLimit, PlanLimitExceeded(current: 15, limit: .agents, max: 15, plan: "intro"))
            XCTAssertFalse(error.retryable)
        }
        // Without the counters the kind alone is kept.
        core.on("agents.register") { _ in throw FakeCore.Failure(kind: "plan_limit_exceeded", message: "Plan limit.") }
        do {
            _ = try await client.agentsRegister(RegisterParams(name: "api", target: Target(id: "thread-1", kind: .codex)))
            XCTFail("expected the refusal")
        } catch let error as CoreError {
            XCTAssertEqual(error.kind, "plan_limit_exceeded")
            XCTAssertNil(error.planLimit)
        }
    }
}
