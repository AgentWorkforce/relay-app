import RelayDesktopCore
import SwiftUI

struct ClaudeProviderMessages: View {
    var relay: RelayModel
    @Environment(AppModel.self) private var model
    @State private var isConfirmingInbound = false
    @State private var inboundError: String?
    @State private var claudeSessionHookError: String?
    @State private var questionEscalationError: String?

    private var hasDelivery: Bool { relay.sessions.contains { $0.target.kind == .claudeTerminal } }

    private var showsSessionPrompt: Bool {
        model.sharingMode == .selected && model.settings?.claudeSessionHook != nil
    }

    /// Question escalation shows once settings are read, beside the session
    /// prompt; a core without the setting shows it unavailable.
    private var showsQuestionEscalation: Bool { model.settings != nil }

    var body: some View {
        if hasDelivery || showsSessionPrompt || showsQuestionEscalation {
            SettingsSection(title: "Messages") {
                VStack(spacing: 0) {
                    if hasDelivery { claudeInboundRow }
                    if showsSessionPrompt {
                        if hasDelivery { SettingsDivider() }
                        claudeSessionHookRow
                    }
                    if showsQuestionEscalation {
                        if hasDelivery || showsSessionPrompt { SettingsDivider() }
                        questionEscalationRow
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

    /// Question escalation (`question_escalation`). It shows what the core
    /// reports, with the core's own availability line; a core that does not
    /// report it gets an unavailable row, not a hidden one.
    private var questionEscalationRow: some View {
        let row = QuestionEscalationRow(model.settings)
        return HStack(alignment: .top, spacing: 14) {
            SettingsIcon(systemImage: row.enabled == true ? "checkmark.seal" : "bell.badge")
            VStack(alignment: .leading, spacing: 4) {
                Text(QuestionEscalationCopy.title).zoomFont(.body, weight: .medium)
                Text(QuestionEscalationCopy.detail)
                    .zoomFont(.footnote).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if row.enabled == nil {
                    notice(QuestionEscalationCopy.unavailable, color: .secondary)
                }
                if let status = row.status {
                    // The core's own text: capped at four lines, as on Linux.
                    Text(verbatim: status)
                        .zoomFont(.footnote).foregroundStyle(.secondary)
                        .lineLimit(4)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if let inbox = row.inbox {
                    Text(verbatim: inbox)
                        .zoomFont(.footnote).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if let questionEscalationError { notice(questionEscalationError, color: .red) }
            }
            Spacer(minLength: 12)
            switch row.enabled {
            case true?:
                Button("Turn Off") { setQuestionEscalation(false) }
                    .help(QuestionEscalationCopy.turnOffHelp)
            case false?:
                Button("Turn On") { setQuestionEscalation(true) }
                    .help(QuestionEscalationCopy.turnOnHelp)
            case nil:
                EmptyView()
            }
        }
        .zoomPadding(16)
    }

    private func setQuestionEscalation(_ enabled: Bool) {
        Task {
            questionEscalationError = await model.setSetting(
                SettingsChange(questionEscalation: enabled ? "on" : "off"))
        }
    }

    private func notice(_ text: String, color: Color) -> some View {
        Label(text, systemImage: "exclamationmark.circle.fill")
            .zoomFont(.footnote).foregroundStyle(color)
            .fixedSize(horizontal: false, vertical: true).zoomPadding(.top, 2)
    }
}
