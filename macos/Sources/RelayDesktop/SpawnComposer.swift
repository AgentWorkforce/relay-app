import SwiftUI
import RelayDesktopCore

/// The message box under a spawned agent's live transcript: it sends into the
/// running agent through the relay, as any message to it is delivered, and
/// starts no process of its own. The Linux app's `ui::spawn_join` matches it.
struct SpawnComposer: View {
    var relay: RelayModel
    let name: String
    let address: String
    @State private var text = ""
    @State private var error: String?
    @State private var sending = false
    @FocusState private var focused: Bool

    private var canSend: Bool { !sending && !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            // One rounded shell holds the growing input and Send at its far
            // right, with a single focus ring around the whole shell.
            HStack(alignment: .bottom, spacing: 8) {
                TextField(SpawnHandover.composerPlaceholder(name), text: $text, axis: .vertical)
                    .textFieldStyle(.plain).lineLimit(1...8)
                    .focused($focused)
                    .zoomPadding(.vertical, 5)
                    .accessibilityLabel(SpawnHandover.composerPlaceholder(name))
                    .onSubmit(send)
                Button(sending ? "Sending…" : SpawnHandover.send, action: send)
                    .buttonStyle(AgentRelayPrimaryButtonStyle())
                    .disabled(!canSend)
            }
            .zoomPadding(.leading, 14).zoomPadding(.trailing, 6).zoomPadding(.vertical, 6)
            .background(CloudTheme.card, in: AgentRelayButtonShape(radius: 20))
            .overlay(AgentRelayButtonShape(radius: 20).stroke(focused ? CloudTheme.ring : CloudTheme.border, lineWidth: focused ? 2 : 1))
            .contentShape(AgentRelayButtonShape(radius: 20))
            .onTapGesture { focused = true }
            status
        }
        .zoomFrame(maxWidth: 760, alignment: .leading)
        .zoomPadding(.horizontal, 32).zoomPadding(.vertical, 12)
        .frame(maxWidth: .infinity)
        .onAppear { relay.openComposer(address) }
        .onDisappear { relay.closeComposer() }
        .onChange(of: text) { if !$1.isEmpty { relay.forgetLastSend() } }
    }

    @ViewBuilder private var status: some View {
        if let error {
            Text(error).zoomFont(.caption).foregroundStyle(.red)
        } else if let note = relay.deliveryNote(at: address) {
            Text(note.text).zoomFont(.caption).foregroundStyle(CloudTheme.secondary)
        } else {
            Text(SpawnHandover.composerNote).zoomFont(.caption).foregroundStyle(CloudTheme.secondary)
        }
    }

    /// The box clears at once; a failed send puts the text back unless
    /// something else was typed meanwhile.
    private func send() {
        guard canSend else { return }
        sending = true
        error = nil
        let body = text
        text = ""
        Task {
            error = await relay.sendMessage(to: address, text: body)
            if error != nil && text.isEmpty { text = body }
            sending = false
        }
    }
}
