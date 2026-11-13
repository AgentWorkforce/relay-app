import Foundation

/// External setup is a handoff, never a local MCP installation or a sent prompt.
public enum CloudProviderSetup: String, CaseIterable, Sendable {
    case capy
    case grokBot = "grok-bot"
    case dot
    case museAssistant = "muse-assistant"

    public var action: String {
        switch self {
        case .capy: return CapySetup.setUpAction
        case .grokBot: return "Set up in Grok Bot"
        case .dot: return "Open Agent Relay plugin"
        case .museAssistant: return "Copy setup prompt"
        }
    }

    /// Muse uses an explicit copy/paste handoff rather than a prompt URL.
    public var copyOnly: Bool { self == .museAssistant }

    public var prompt: String? {
        switch self {
        case .capy: return CapySetup.prompt
        case .grokBot: return "Set me up on Agent Relay: https://github.com/AgentWorkforce/skills/blob/main/skills/agent-relay-grok-bot/SKILL.md"
        case .museAssistant: return "Set me up on Agent Relay: https://github.com/AgentWorkforce/skills/blob/main/skills/agent-relay-muse/SKILL.md"
        case .dot: return nil
        }
    }

    public var skillURL: URL? {
        self == .museAssistant ? URL(string: "https://github.com/AgentWorkforce/skills/blob/main/skills/agent-relay-muse/SKILL.md") : nil
    }

    public var webURL: URL {
        switch self {
        case .capy: return CapySetup.url
        case .grokBot: return URL(string: "https://cursor.com/bot")!
        case .dot: return URL(string: "https://chatgpt.com/plugins/plugin_asdk_app_6ad1e2038840819189295d49199c2915")!
        case .museAssistant: return URL(string: "https://muse.ai/")!
        }
    }

    public var appURL: URL? {
        let base: String
        let key: String
        switch self {
        case .capy: base = "capy://app/new"; key = "prompt"
        case .grokBot: base = "grokbot://app/v1/prompt"; key = "text"
        case .dot, .museAssistant: return nil
        }
        var url = URLComponents(string: base)!
        url.queryItems = [URLQueryItem(name: key, value: prompt!)]
        return url.url!
    }

    public var copied: String? {
        switch self {
        case .capy: return CapySetup.copied
        case .grokBot: return "Setup prompt copied. Send it in Grok Bot to begin. If you don’t have a Bot yet, create one and paste the prompt."
        case .museAssistant: return "Setup prompt copied. Open Muse, then paste and send it to begin."
        case .dot: return nil
        }
    }
}
