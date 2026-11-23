import SwiftUI

/// One row of the agent and conversation lists: the avatar with its status
/// circle, the name, the session's title and one-line summary, a caption
/// with the CLI and the machine, and the unread dot.
///
/// Every line uses a text style, so the row grows with Dynamic Type instead
/// of clipping; `minHeight` is a floor, never a fixed height.
struct AgentRowLabel<Avatar: View>: View {
    let model: AgentRowModel
    let accent: Color
    let showsChevron: Bool
    @ViewBuilder let avatar: () -> Avatar

    static var minHeight: CGFloat { 72 }

    var body: some View {
        HStack(alignment: .center, spacing: 11) {
            avatar()
                .overlay(alignment: .bottomTrailing) {
                    if let status = model.status {
                        AgentStatusCircle(status: status).offset(x: 3, y: 3)
                    }
                }
            VStack(alignment: .leading, spacing: 2) {
                HStack(alignment: .firstTextBaseline, spacing: 5) {
                    Text(model.name)
                        .font(.callout.weight(.semibold)).lineLimit(1)
                        .accessibilityIdentifier("Agent row name")
                    if let handle = model.handle {
                        // A person's handle is what agents address them by.
                        Text(handle).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                            .layoutPriority(-1)
                    }
                }
                // The title and the caption wrap to a second line so they
                // stay readable in the narrow iPad sidebar; the summary is
                // always one line.
                if let title = model.title {
                    Text(title)
                        .font(.subheadline.weight(.medium)).lineLimit(2)
                        .multilineTextAlignment(.leading)
                        .accessibilityIdentifier("Agent row title")
                }
                if let summary = model.summary {
                    Text(summary)
                        .font(.footnote).foregroundStyle(.secondary).lineLimit(1)
                        .accessibilityIdentifier("Agent row summary")
                }
                if let caption = model.caption {
                    Text(caption)
                        .font(.caption).foregroundStyle(.secondary).lineLimit(2)
                        .multilineTextAlignment(.leading)
                        .accessibilityIdentifier("Agent row caption")
                }
            }
            // The text takes the free width: no spacing is spent on a
            // trailing spacer when there is no unread dot or chevron.
            .frame(maxWidth: .infinity, alignment: .leading)
            if model.unread {
                Circle().fill(accent).frame(width: 10, height: 10)
                    .accessibilityLabel("Unread messages")
                    .accessibilityIdentifier("Agent row unread")
            }
            if showsChevron {
                Image(systemName: "chevron.right").font(.caption).foregroundStyle(.tertiary)
                    .accessibilityHidden(true)
            }
        }
        .padding(.vertical, 9)
        .frame(maxWidth: .infinity, minHeight: Self.minHeight, alignment: .leading)
        .contentShape(Rectangle())
    }
}

/// The reachable signal: filled when the agent can be reached (green, or
/// orange while it waits on a person, gray while idle), a hollow ring when
/// it is offline or reports nothing.
struct AgentStatusCircle: View {
    let status: AgentRowModel.Status

    var body: some View {
        Circle()
            .fill(fill)
            .overlay(Circle().strokeBorder(ring, lineWidth: 1.5))
            .frame(width: 11, height: 11)
            .background(Circle().fill(Color(.systemBackground)).padding(-1.5))
            .accessibilityLabel(status.label)
            .accessibilityIdentifier("Agent row status")
    }

    private var fill: Color {
        switch status {
        case .online: .green
        case .waiting: .orange
        case .idle: Color(.systemGray2)
        case .offline, .unknown: Color(.systemBackground)
        }
    }

    private var ring: Color {
        switch status {
        case .online, .waiting, .idle: .clear
        case .offline: Color(.systemGray2)
        case .unknown: Color(.systemGray4)
        }
    }
}
