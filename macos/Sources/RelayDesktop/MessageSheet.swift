import AppKit
import RelayDesktopCore
import SwiftUI

extension View {
    /// Hosts the message composer over the whole window; `agentrelay://`
    /// message links open it.
    func messageComposer(_ relay: RelayModel, model: AppModel) -> some View {
        sheet(isPresented: Binding(get: { model.isComposingMessage }, set: { model.isComposingMessage = $0 })) {
            MessageSheet(relay: relay).environment(model)
        }
    }
}

/// A message to any relay address — a name, or the exact address one agent
/// on one machine has. It says whether the recipient runs before a send,
/// what the send met after it, and whether a reply is expected.
struct MessageSheet: View {
    var relay: RelayModel
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var text = ""
    @State private var error: String?
    @State private var isSending = false

    private var endpoint: String { RelayModel.endpoint(model.messageEndpoint) }

    private var canSend: Bool {
        !isSending && !endpoint.isEmpty && !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    /// The history read last, with messages sent since shown as on their way.
    private var history: History? { relay.shownHistory(at: endpoint) }

    private var recipient: Recipient? { relay.recipient(at: endpoint) }

    /// Sending to a recipient that is not running queues the message; the
    /// button says so.
    private var queues: Bool { MessageDelivery.queues(recipient) }

    var body: some View {
        @Bindable var model = model
        VStack(alignment: .leading, spacing: 12) {
            Text("Message an Agent").zoomFont(.headline)
            HStack(spacing: 8) {
                TextField("To: @name or agent@machine", text: $model.messageEndpoint)
                    .textFieldStyle(.roundedBorder).zoomFont(.body, design: .monospaced)
                Button {
                    Task {
                        await relay.lookUpRecipient(endpoint)
                        await relay.loadConversation(endpoint)
                    }
                } label: {
                    if relay.isLoadingConversation {
                        ProgressView().controlSize(.small)
                    } else {
                        Image(systemName: "arrow.clockwise")
                    }
                }
                .help("Refresh this conversation")
                .disabled(relay.isLoadingConversation || endpoint.isEmpty)
            }
            TextField("Message", text: $text, axis: .vertical)
                .textFieldStyle(.roundedBorder).lineLimit(3...8)
            statusLine
            HStack {
                if let note = recipient.flatMap({ MessageDelivery.replyNote(for: $0) }) {
                    Text(note).zoomFont(.caption).foregroundStyle(CloudTheme.secondary)
                }
                Spacer()
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Button(isSending ? "Sending…" : (queues ? "Queue Anyway" : "Send"), action: send)
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(AgentRelayPrimaryButtonStyle())
                    .disabled(!canSend)
            }
            if let error = relay.conversationError, history == nil {
                Text(error).zoomFont(.caption).foregroundStyle(.red)
            }
            if let history, !history.messages.isEmpty {
                Divider()
                ScrollView {
                    LazyVStack(spacing: 8) {
                        ForEach(history.messages) { directMessage($0) }
                    }
                }
                .zoomFrame(maxHeight: 260)
            }
        }
        .zoomPadding(20)
        .zoomFrame(width: 560)
        .zoomFrame(minHeight: 300)
        .onChange(of: endpoint) { _, endpoint in
            error = nil
            relay.composerShows(endpoint)
        }
        // A new message gives the last send's note way to the recipient's
        // state, so a warning about it is never hidden behind it.
        .onChange(of: text) { if !$1.isEmpty { relay.forgetLastSend() } }
        .onAppear { relay.openComposer(endpoint) }
        .onDisappear { relay.closeComposer() }
    }

    /// Before a send: whether the recipient runs. After one: what it met,
    /// updated as the recipient comes and goes.
    @ViewBuilder private var statusLine: some View {
        if let error {
            Text(error).zoomFont(.caption).foregroundStyle(.red)
        } else if let note = relay.deliveryNote(at: endpoint) {
            Text(note.text).zoomFont(.caption).foregroundStyle(color(note.tone))
        } else if let recipient, let warning = MessageDelivery.warning(for: recipient) {
            Text(warning).zoomFont(.caption)
                .foregroundStyle(recipient.agent == nil ? Color.red : Color.orange)
        } else {
            Text("An exact address reaches only that agent on that machine.")
                .zoomFont(.caption).foregroundStyle(CloudTheme.secondary)
        }
    }

    private func color(_ tone: DeliveryTone) -> Color {
        switch tone {
        case .delivered: return .green
        case .waiting: return .orange
        case .failed: return .red
        case .unknown: return .secondary
        }
    }

    /// The box clears at once and the message shows as on its way; a failed
    /// send puts the text back unless something else was typed meanwhile.
    private func send() {
        isSending = true
        error = nil
        let target = model.messageEndpoint
        let body = text
        text = ""
        Task {
            error = await relay.sendMessage(to: target, text: body)
            if error != nil && text.isEmpty { text = body }
            isSending = false
        }
    }

    @ViewBuilder
    private func directMessage(_ message: DirectMessage) -> some View {
        HStack {
            if message.mine { Spacer(minLength: 60) }
            VStack(alignment: message.mine ? .trailing : .leading, spacing: 3) {
                Text((message.threadId == nil ? "" : "↳ Reply · ") + (message.mine ? "You" : "@\(message.sender)"))
                    .zoomFont(.caption2).foregroundStyle(CloudTheme.secondary)
                Text(message.text)
                    .textSelection(.enabled)
                    .zoomPadding(.horizontal, 10).zoomPadding(.vertical, 7)
                    .background(
                        message.mine ? Color.accentColor.opacity(0.16) : Color.primary.opacity(0.07),
                        in: RoundedRectangle(cornerRadius: 9)
                    )
                    .opacity(PendingMessages.isPending(message) ? 0.7 : 1)
                if PendingMessages.isPending(message) {
                    Text(PendingMessages.sending).zoomFont(.caption2).foregroundStyle(CloudTheme.secondary)
                }
            }
            if !message.mine { Spacer(minLength: 60) }
        }
        .zoomPadding(.leading, message.threadId == nil ? 0 : 28)
    }
}
