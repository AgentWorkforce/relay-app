import RelayDesktopCore
import SwiftUI

struct ClaudeProviderMessages: View {
    var relay: RelayModel
    @Environment(AppModel.self) private var model
    @State private var isConfirmingInbound = false
    @State private var inboundError: String?
    @State private var claudeSessionHookError: String?

    private var hasDelivery: Bool { relay.sessions.contains { $0.target.kind == .claudeTerminal } }

    private var showsSessionPrompt: Bool {
        model.sharingMode == .selected && model.settings?.claudeSessionHook != nil
    }

    var body: some View {
        if hasDelivery || showsSessionPrompt {
            SettingsSection(title: "Messages") {
                VStack(spacing: 0) {
                    if hasDelivery { claudeInboundRow }
                    if showsSessionPrompt {
                        if hasDelivery { SettingsDivider() }
                        claudeSessionHookRow
                    }
                }
            }
        }
    }

    private var claudeInboundRow: some View {
        let direct = model.settings?.directDelivery == true
        let managed = model.settings?.directDeliveryManaged
        let isManaged = managed != nil && managed != "accept"
        return HStack(alignment: .top, spacing: 14) {
            SettingsIcon(systemImage: direct ? "checkmark.seal" : "hand.raised")
            VStack(alignment: .leading, spacing: 4) {
                Text("Claude Code delivery").zoomFont(.body, weight: .medium)
                Group {
                    if direct {
                        Text("Relayed messages arrive directly: `crossSessionInbound` is `accept` in your Claude settings.")
                    } else if isManaged {
                        Text(LocalizedStringKey(ManagedDirectDelivery.detail(managed ?? "")))
                    } else {
                        Text("Sessions that skip permission prompts hold relayed messages for your approval. Deliver directly to let them arrive the way Codex messages do.")
                    }
                }
                .zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
                .fixedSize(horizontal: false, vertical: true)
                if let error = inboundError { notice(error, color: .red) }
            }
            Spacer(minLength: 12)
            if direct {
                if managed == nil {
                    Button("Ask Again") { setDirect(false) }
                        .help("Set crossSessionInbound to hold in your Claude settings")
                }
            } else if !isManaged {
                Button("Deliver Directly…") { isConfirmingInbound = true }
            }
        }
        .zoomPadding(16)
        .confirmationDialog("Deliver relayed messages to Claude Code without asking?", isPresented: $isConfirmingInbound) {
            Button("Deliver directly") { setDirect(true) }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This adds \"crossSessionInbound\": \"accept\" to ~/.claude/settings.json. Every local Claude Code session will then show messages from other sessions and programs, including Agent Relay, without holding them for approval. Claude Code still labels them and never lets them change permissions. Your current file is kept as settings.json.relay-desktop-backup.")
        }
    }

    private func setDirect(_ enabled: Bool) {
        Task { inboundError = await model.setSetting(SettingsChange(directDelivery: enabled)) }
    }

    /// Claude Code's SessionStart hook. On by default after sign-in. It never
    /// registers a session; it tells one that is not on the relay how to
    /// POST /register.
    private var claudeSessionHookRow: some View {
        let enabled = model.settings?.claudeSessionHook == true
        return HStack(alignment: .top, spacing: 14) {
            SettingsIcon(systemImage: enabled ? "checkmark.seal" : "bell.badge")
            VStack(alignment: .leading, spacing: 4) {
                Text(ClaudeSessionHookCopy.title).zoomFont(.body, weight: .medium)
                Text(enabled ? ClaudeSessionHookCopy.turnedOn : ClaudeSessionHookCopy.turnedOff)
                    .zoomFont(.footnote).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if let claudeSessionHookError { notice(claudeSessionHookError, color: .red) }
            }
            Spacer(minLength: 12)
            if enabled {
                Button("Turn Off") { setClaudeSessionHook(false) }
                    .help(ClaudeSessionHookCopy.turnOffHelp)
            } else {
                Button("Turn On") { setClaudeSessionHook(true) }
                    .help(ClaudeSessionHookCopy.turnOnHelp)
            }
        }
        .zoomPadding(16)
    }

    private func setClaudeSessionHook(_ enabled: Bool) {
        Task { claudeSessionHookError = await model.setSetting(SettingsChange(claudeSessionHook: enabled)) }
    }

    private func notice(_ text: String, color: Color) -> some View {
        Label(text, systemImage: "exclamationmark.circle.fill")
            .zoomFont(.footnote).foregroundStyle(color)
            .fixedSize(horizontal: false, vertical: true).zoomPadding(.top, 2)
    }
}
