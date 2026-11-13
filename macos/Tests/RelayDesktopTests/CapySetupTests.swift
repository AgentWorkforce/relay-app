import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

@MainActor
final class CapySetupTests: XCTestCase {
    private var root: URL!
    private var home: URL!
    private var applications: URL!

    override func setUpWithError() throws {
        root = FileManager.default.temporaryDirectory.appendingPathComponent("capy-\(UUID())")
        home = root.appendingPathComponent("home")
        applications = root.appendingPathComponent("Applications")
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: applications, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: root)
    }

    private func installed() -> Bool {
        CapySetup.isInstalled(home: home, applications: applications)
    }

    func testNotDetectedWithoutTheAppOrConnector() throws {
        XCTAssertFalse(installed())
        try Data().write(to: home.appendingPathComponent(".capy"))
        XCTAssertFalse(installed(), "a file named .capy is not the connector's directory")
    }

    func testDetectsTheAppInApplications() throws {
        try FileManager.default.createDirectory(at: applications.appendingPathComponent("Capy.app"),
                                                withIntermediateDirectories: true)
        XCTAssertTrue(installed())
    }

    func testDetectsTheAppInTheHomeApplicationsFolder() throws {
        try FileManager.default.createDirectory(at: home.appendingPathComponent("Applications/Capy.app"),
                                                withIntermediateDirectories: true)
        XCTAssertTrue(installed())
    }

    func testDetectsTheDeviceConnectorDirectory() throws {
        try FileManager.default.createDirectory(at: home.appendingPathComponent(".capy"),
                                                withIntermediateDirectories: true)
        XCTAssertTrue(installed())
    }

    func testShowsOnlyWhileDetectedAndNotDismissed() {
        XCTAssertTrue(CapySetup.shows(installed: true, dismissed: false))
        XCTAssertFalse(CapySetup.shows(installed: true, dismissed: true))
        XCTAssertFalse(CapySetup.shows(installed: false, dismissed: false))
    }

    func testNotNowHidesTheCardForGood() {
        let suite = "capy-setup-test-\(UUID())"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let model = AppModel(manageSystemServices: false, defaults: defaults, detectCapy: { true })
        XCTAssertTrue(model.showsCapySetup)
        XCTAssertFalse(model.capyPromptCopied)
        model.dismissCapySetup()
        XCTAssertFalse(model.showsCapySetup)
        XCTAssertTrue(defaults.bool(forKey: CapySetup.dismissedKey))
        let restarted = AppModel(manageSystemServices: false, defaults: defaults, detectCapy: { true })
        XCTAssertFalse(restarted.showsCapySetup)
        let withoutCapy = AppModel(manageSystemServices: false, defaults: UserDefaults(suiteName: "\(suite)-none")!,
                                   detectCapy: { false })
        XCTAssertFalse(withoutCapy.showsCapySetup)
        UserDefaults(suiteName: "\(suite)-none")?.removePersistentDomain(forName: "\(suite)-none")
    }

    func testCapyInstalledWhileRunningShowsTheCardOnTheNextLook() {
        let suite = "capy-setup-test-\(UUID())"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        var installed = false
        let model = AppModel(manageSystemServices: false, defaults: defaults, detectCapy: { installed })
        XCTAssertFalse(model.showsCapySetup)
        installed = true
        model.refreshCapySetup()
        XCTAssertTrue(model.showsCapySetup)
        model.dismissCapySetup()
        model.refreshCapySetup()
        XCTAssertFalse(model.showsCapySetup)
    }

    func testWordingMatchesLinux() {
        XCTAssertEqual(CapySetup.title, "Capy detected")
        XCTAssertEqual(CapySetup.message,
                       "Put your Capy threads on Agent Relay so other agents can message them and their conversations appear in Sessions. Capy asks you to approve the Agent Relay plugin once.")
        XCTAssertEqual(CapySetup.setUpAction, "Set up in Capy")
        XCTAssertEqual(CapySetup.notNowAction, "Not now")
        XCTAssertEqual(CapySetup.prompt,
                       "Set me up on Agent Relay: https://github.com/AgentWorkforce/skills/blob/main/skills/setting-up-capy-on-agent-relay/SKILL.md")
        XCTAssertEqual(CapySetup.copied, "Setup prompt copied. Send it in Capy to begin.")
        XCTAssertEqual(CapySetup.url.absoluteString, "https://capy.ai/new?prompt=Set%20me%20up%20on%20Agent%20Relay%3A%20https%3A%2F%2Fgithub.com%2FAgentWorkforce%2Fskills%2Fblob%2Fmain%2Fskills%2Fsetting-up-capy-on-agent-relay%2FSKILL.md")
        XCTAssertEqual(CapySetup.dismissedKey, "capySetupDismissed")
    }

    func testCloudSetupLinksPrefillWithoutSending() throws {
        for (setup, scheme, path, key) in [(CloudProviderSetup.capy, "capy", "/new", "prompt"),
                                            (.grokBot, "grokbot", "/v1/prompt", "text")] {
            let url = try XCTUnwrap(setup.appURL)
            let components = try XCTUnwrap(URLComponents(url: url, resolvingAgainstBaseURL: false))
            XCTAssertEqual(url.scheme, scheme)
            XCTAssertEqual(url.host, "app")
            XCTAssertEqual(url.path, path)
            XCTAssertEqual(components.queryItems, [URLQueryItem(name: key, value: setup.prompt)])
        }
        XCTAssertTrue(CloudProviderSetup.grokBot.prompt!.contains("/skills/agent-relay-grok-bot/SKILL.md"))
        XCTAssertNil(CloudProviderSetup.dot.appURL)
        XCTAssertNil(CloudProviderSetup.dot.prompt)
        XCTAssertEqual(CloudProviderSetup.dot.webURL.absoluteString,
                       "https://chatgpt.com/plugins/plugin_asdk_app_6ad1e2038840819189295d49199c2915")
    }

    func testMuseCopiesItsPublishedSkillWithoutInventingAPrefillURL() throws {
        let muse = CloudProviderSetup.museAssistant
        XCTAssertTrue(muse.copyOnly)
        let skill = try XCTUnwrap(muse.skillURL)
        XCTAssertEqual(skill.absoluteString, "https://github.com/AgentWorkforce/skills/blob/main/skills/agent-relay-muse/SKILL.md")
        XCTAssertTrue(try XCTUnwrap(muse.prompt).contains(skill.absoluteString))
        XCTAssertNil(muse.appURL)
        XCTAssertEqual(muse.webURL.absoluteString, "https://muse.ai/")
        XCTAssertNotNil(muse.copied)
    }

    func testSetupPrefersInstalledAppsAndFallsBackWhenOpeningFails() {
        for setup in [CloudProviderSetup.capy, .grokBot, .dot, .museAssistant] {
            for installed in [false, true] {
                for appOpens in [false, true] {
                    var opened: [URL] = []
                    XCTAssertTrue(AppModel.openCloudProvider(setup, appAvailable: { _ in installed }, open: { url in
                        opened.append(url)
                        return url == setup.webURL || appOpens
                    }))
                    let expected: [URL]
                    if installed, let app = setup.appURL { expected = appOpens ? [app] : [app, setup.webURL] }
                    else { expected = [setup.webURL] }
                    XCTAssertEqual(opened, expected)
                }
            }
        }
    }
}
