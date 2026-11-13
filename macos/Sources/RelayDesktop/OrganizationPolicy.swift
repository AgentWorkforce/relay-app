import RelayDesktopCore
import SwiftUI

/// The settings the workspace operator's upload policy sets, as
/// `UploaderStatus.enforcedSettings` reports them. A control a present
/// setting governs is removed, not disabled; GTK removes the same ones.
enum OrganizationPolicy {
    /// Shown where a control the policy sets was removed; GTK's
    /// `MANAGED_BY_ORGANIZATION` says the same.
    static let managed = "Managed by your organization."

    static let sharingModeKey = "sharing_mode"
    static let uploadControlKey = "upload_control"
    static let userRetroactiveDeleteKey = "user_retroactive_delete"

    /// The policy enforces this mode; the picker is removed.
    static func sharingMode(in settings: [String: String]?) -> SharingMode? {
        settings?[sharingModeKey].flatMap(SharingMode.init(rawValue:))
    }

    /// The operator runs uploads; the person may not pause or resume them.
    static func controlsUploads(in settings: [String: String]?) -> Bool {
        settings?[uploadControlKey] == "operator"
    }

    /// The person may delete their sessions from Cloud unless the policy
    /// withdraws it.
    static func allowsDelete(in settings: [String: String]?) -> Bool {
        settings?[userRetroactiveDeleteKey] != "false"
    }
}

/// The secondary line shown where the policy removed a control.
struct ManagedByOrganizationLine: View {
    var body: some View {
        Text(OrganizationPolicy.managed)
            .zoomFont(.footnote)
            .foregroundStyle(CloudTheme.secondary)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// A read-only row for the upload policy the operator enforces: its title
/// and explanation, with nothing to choose.
struct EnforcedUploadPolicyRow: View {
    let mode: SharingMode

    var body: some View {
        HStack(alignment: .top, spacing: 14) {
            Image(systemName: "checkmark.circle.fill")
                .zoomFont(size: 18, weight: .medium)
                .foregroundStyle(CloudTheme.primary)
                .zoomFrame(width: 24, height: 24)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 4) {
                Text(mode.title)
                    .zoomFont(.body, weight: .medium)
                    .foregroundStyle(CloudTheme.foreground)
                Text(mode.explanation)
                    .zoomFont(.footnote)
                    .foregroundStyle(CloudTheme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 12)
        }
        .zoomPadding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
    }
}
