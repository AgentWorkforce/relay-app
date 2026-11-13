import Foundation

/// Capy's setup action opens a new thread with the setup skill prompt prefilled.
/// The Linux and Windows app's `models::capy_setup` matches the wording.
public enum CapySetup {
    public static let title = "Capy detected"
    public static let message = "Put your Capy threads on Agent Relay so other agents can message them and their conversations appear in Sessions. Capy asks you to approve the Agent Relay plugin once."
    public static let setUpAction = "Set up in Capy"
    public static let notNowAction = "Not now"
    /// Prefilled in the new thread, and copied as a fallback.
    public static let prompt = "Set me up on Agent Relay: https://github.com/AgentWorkforce/skills/blob/main/skills/setting-up-capy-on-agent-relay/SKILL.md"
    public static let copied = "Setup prompt copied. Send it in Capy to begin."
    public static let url = URL(string: "https://capy.ai/new?prompt=Set%20me%20up%20on%20Agent%20Relay%3A%20https%3A%2F%2Fgithub.com%2FAgentWorkforce%2Fskills%2Fblob%2Fmain%2Fskills%2Fsetting-up-capy-on-agent-relay%2FSKILL.md")!
    /// Set once the person chose Not now; the card never returns.
    public static let dismissedKey = "capySetupDismissed"

    /// Capy is installed: its app in `/Applications` or `~/Applications`, or
    /// the `~/.capy` directory its device connector creates on a paired
    /// computer.
    public static func isInstalled(home: URL,
                                   applications: URL = URL(fileURLWithPath: "/Applications", isDirectory: true),
                                   fileManager: FileManager = .default) -> Bool {
        [applications.appendingPathComponent("Capy.app"),
         home.appendingPathComponent("Applications/Capy.app"),
         home.appendingPathComponent(".capy")].contains { candidate in
            var isDirectory: ObjCBool = false
            return fileManager.fileExists(atPath: candidate.path, isDirectory: &isDirectory) && isDirectory.boolValue
        }
    }

    public static func shows(installed: Bool, dismissed: Bool) -> Bool {
        installed && !dismissed
    }
}
