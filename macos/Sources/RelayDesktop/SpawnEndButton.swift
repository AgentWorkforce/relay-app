import RelayDesktopCore
import SwiftUI

/// Ends a session Agent Relay spawned and still owns. The core stops or
/// archives the exact harness session, confirms it ended, then removes its
/// relay identity. GTK's `ui::spawn_end` matches this behavior and wording.
struct SpawnEndButton: View {
    let session: LocalSession
    var relay: RelayModel
    @State private var confirming = false

    private var name: String { session.registration?.name ?? "" }
    private var ending: Bool { relay.endingSpawnedSessions.contains(session.id) }
    private var error: String? { relay.endSpawnedErrors[session.id] }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button(role: .destructive) { confirming = true } label: {
                HStack(spacing: 6) {
                    if ending { ProgressView().controlSize(.small) }
                    else { Image(systemName: "stop.circle") }
                    Text(ending ? SpawnHandover.ending : SpawnHandover.endButton)
                }
                .frame(maxWidth: .infinity)
            }
            .buttonStyle(CloudControlStyle(height: 38, horizontalPadding: 12))
            .disabled(ending || name.isEmpty)
            .help(SpawnHandover.endHelp)
            .accessibilityLabel(SpawnHandover.endButton)
            if let error {
                Text("\(SpawnHandover.endFailedTitle): \(error)")
                    .zoomFont(.caption).foregroundStyle(CloudTheme.warning)
                    .fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
            }
        }
        .confirmationDialog(SpawnHandover.endTitle(name), isPresented: $confirming) {
            Button(SpawnHandover.endButton, role: .destructive) { end() }
            Button(SpawnHandover.cancel, role: .cancel) {}
        } message: {
            Text(SpawnHandover.endMessage(name))
        }
    }

    private func end() {
        Task { try? await relay.endSpawned(name, sessionID: session.id) }
    }
}
