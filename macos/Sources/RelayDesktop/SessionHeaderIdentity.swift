import SwiftUI
import RelayDesktopCore

/// The session header's second line: the provider, the session's name on the
/// relay, and its live presence. The Linux app's transcript heading matches it.
struct SessionHeaderIdentity: View {
    let row: LocalSessionRow
    var relay: RelayModel

    private var session: LocalSession? { LocalSessionRow.relaySession(for: row.id, in: relay.sessions) }

    var body: some View {
        let presence = session.map { relay.presence(of: $0) } ?? .off
        let agent = presence.isOn ? session.flatMap { relay.agent(for: $0) }?.displayName : nil
        let folder = row.directory.isEmpty ? row.project : row.directory
        HStack(spacing: 8) {
            Text(SessionHeader.identity(provider: AgentIconView.name(for: row.source), agentName: agent))
                .zoomFont(size: 11).foregroundStyle(CloudTheme.secondary).lineLimit(1)
                .help(folder)
            if let badge = presence.headerBadge {
                Text(badge)
                    .zoomFont(size: 10, weight: .semibold)
                    .foregroundStyle(presence == .online ? CloudTheme.success : CloudTheme.secondary)
                    .zoomPadding(.horizontal, 7).zoomPadding(.vertical, 1)
                    .background((presence == .online ? CloudTheme.success : CloudTheme.secondary).opacity(0.14), in: Capsule())
                    .help(presence.detail)
            }
        }
    }
}
