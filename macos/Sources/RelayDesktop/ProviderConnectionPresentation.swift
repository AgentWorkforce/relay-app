import RelayDesktopCore

/// A useful next step, separate from the core's diagnostic explanation.
/// The summary is shown only when the status and action leave something unsaid.
struct ProviderConnectionPresentation {
    enum Action: Equatable {
        case install, repair, account, check, enable
        /// Names what the action connects or fixes.
        func title(for provider: String, selected: Bool) -> String {
            switch self {
            case .install: return "Connect \(provider)"
            case .enable: return "Enable \(provider)"
            case .repair: return selected ? "Fix \(provider) connection" : "Connect \(provider)"
            case .account: return "Sign in to Agent Relay"
            case .check: return "Check connection"
            }
        }
    }
    let status: String
    let summary: String?
    let action: Action?
    let actionTitle: String?

    init(_ provider: ToolIntegration, health: String?) {
        if ["capy", "grok-bot", "dot"].contains(provider.id) || provider.selected,
           ["capy", "grok-bot", "dot", "conductor", "paseo"].contains(provider.id), provider.state == "configured" {
            status = health == "ready" ? "Connected" : "Connection not verified"
            summary = health == "ready" ? provider.detail : "Check your Agent Relay account connection, then check this provider again."
            action = ["capy", "grok-bot", "dot"].contains(provider.id) ? .check : nil
            actionTitle = action?.title(for: provider.name, selected: provider.selected)
            return
        }
        if ["conductor", "paseo"].contains(provider.id), provider.selected, provider.state == "action_required" {
            status = provider.statusLabel; summary = provider.detail; action = nil; actionTitle = nil
            return
        }
        let external: (String, String)?
        switch provider.id {
        case "conductor": external = ("Uses Codex or Claude Code", "Manage the connection through the agent running inside Conductor.")
        case "paseo": external = ("Uses your coding agents", "Manage each agent’s connection below.")
        case "cursor" where !provider.supported: external = (provider.statusLabel, provider.detail)
        case "copilot" where !provider.supported: external = (provider.statusLabel, provider.detail)
        case "grok-bot", "dot", "muse-assistant": external = (provider.statusLabel, provider.detail)
        case "capy": external = provider.statusLabel == "Cloud integration"
            ? ("Connect in Capy", "Add Agent Relay to your Capy threads to send messages and share sessions.")
            : (provider.statusLabel, provider.detail)
        case "amp": external = (AmpConnections.status(state: provider.state), provider.detail)
        case "replicas": external = ("Managed in Agent Relay Cloud", "Replicas uses your cloud connection for launching agents and messaging.")
        case "claude-cloud": external = ("Uses your Claude account", "Manage cloud-session sharing in Claude Code settings.")
        default: external = nil
        }
        var next: Action?
        if let external {
            status = external.0; summary = external.1
            if ["conductor", "paseo"].contains(provider.id) && !provider.selected { next = .enable }
        } else if !provider.supported {
            status = "Setup not available yet"
            summary = "Agent Relay hasn’t added automatic setup for this provider yet."
        } else if provider.state == "checking" {
            status = "Checking"
            summary = "Agent Relay is checking setup. This will finish automatically."
        } else if provider.selected && (provider.state == "signed_out" || health == "sign_in_required") {
            status = "Sign in required"
            summary = nil
            next = .account
        } else if provider.selected && health == "offline" {
            status = "Can’t verify connection"
            summary = "Check your internet connection, then try again."
            next = .check
        } else if !provider.selected && provider.state == "action_required" {
            status = "Needs attention"
            summary = "Agent Relay couldn’t finish changing this connection. Check Connection details and try again."
        } else if !provider.selected {
            status = "Not connected"
            summary = provider.detected ? nil : "Install \(provider.name) on this computer to connect it."
            if provider.canInstall { next = .install }
            else if provider.canRepair { next = .repair }
        } else if provider.statusLabel == "Messaging unavailable" {
            status = "Messaging unavailable"
            summary = nil
        } else if provider.id == "codex" && provider.statusLabel == "MCP not ready" {
            status = "Reload Codex chats"
            summary = "Codex chats haven’t loaded Agent Relay. Reload or start a new Codex chat, then check the connection."
            next = .check
        } else if provider.state == "installed" && provider.statusLabel == "Connection not verified" {
            status = provider.statusLabel
            summary = provider.detail
            next = .check
        } else if provider.state == "installed" {
            status = "Setup saved"
            summary = "Access from running chats hasn’t been verified yet."
        } else if provider.state == "configured" && health == "ready" {
            status = "Connected"
            summary = provider.statusLabel == "Connected" ? provider.detail : nil
            if provider.statusLabel == "Connected" { next = .check }
        } else {
            status = "Needs attention"
            summary = provider.detail
            if provider.canRepair { next = .repair }
        }
        action = next
        actionTitle = next?.title(for: provider.name, selected: provider.selected)
    }
}
