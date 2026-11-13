import RelayDesktopCore
import SwiftUI

struct GlobalSessionSharingSettings: View {
    /// Shown under the rows while uploads wait for the operator's policy to
    /// be read; GTK's sharing pane says the same.
    static let withheldExplanation = "Uploads wait until your organization's upload policy can be checked."

    /// What the section shows: the picker, locked while uploads wait for the
    /// operator's policy, or, when the policy sets the mode, that mode alone
    /// with no picker.
    enum Presentation: Equatable {
        case picker(lock: String?)
        case managed(SharingMode)
    }

    static func presentation(enforced: SharingMode?, withheld: Bool) -> Presentation {
        if withheld { return .picker(lock: withheldExplanation) }
        if let enforced { return .managed(enforced) }
        return .picker(lock: nil)
    }

    @Environment(AppModel.self) private var model
    @State private var pendingMode: SharingMode?

    var body: some View {
        VStack(alignment: .leading, spacing: 28) { sections }
        // Driven by the mode itself, so the sheet always asks about the
        // row that was clicked (an isPresented flag could build the sheet
        // before pendingMode landed and ask about the current mode).
        .sheet(item: $pendingMode) { mode in
            SharingConfirmationSheet(
                mode: mode,
                onCancel: { pendingMode = nil },
                onConfirm: { mode in
                    pendingMode = nil
                    Task {
                        await model.setSharingMode(mode)
                    }
                }
            )
        }
    }

    private var presentation: Presentation {
        Self.presentation(enforced: model.enforcedSharingMode, withheld: model.areUploadsWithheld)
    }

    @ViewBuilder private func picker(lock: String?) -> some View {
        ForEach(Array(SharingMode.displayOrder.enumerated()), id: \.element) { index, mode in
            if index > 0 {
                SettingsDivider(leadingInset: 52)
            }

            // The chosen policy moves at once and the rows stay
            // usable: a later choice wins over one still saving.
            UploadPolicyRow(
                mode: mode,
                isSelected: model.sharingMode == mode,
                isPending: model.isChangingSharingMode && model.sharingMode == mode
            ) {
                guard mode != model.sharingMode else { return }
                pendingMode = mode
            }
            .disabled(lock != nil)
        }
        if let lock {
            SettingsDivider()
            Label(lock, systemImage: "lock.fill")
                .font(.footnote).foregroundStyle(CloudTheme.secondary)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(16)
        }
    }

    @ViewBuilder private var sections: some View {
        if model.phase == .attached {
            SettingsSection(
                title: "Upload policy",
                description: "Applies to all providers. Shared sessions are visible to your workspace. Sessions you put on the relay are always shared."
            ) {
                VStack(spacing: 0) {
                    switch presentation {
                    case .managed(let mode):
                        EnforcedUploadPolicyRow(mode: mode)
                        SettingsDivider()
                        ManagedByOrganizationLine().zoomPadding(16)
                    case .picker(let lock):
                        picker(lock: lock)
                    }
                    if let error = model.sessionActionError {
                        SettingsDivider()
                        Label(error, systemImage: "exclamationmark.circle.fill")
                            .zoomFont(.footnote).foregroundStyle(CloudTheme.warning)
                            .fixedSize(horizontal: false, vertical: true)
                            .zoomPadding(16)
                    }
                }
            }
        } else {
            SettingsSection(title: "Upload policy") {
                SettingsUnavailableRow(
                    title: model.phase == .preparingSessions
                        ? "Finding your sessions"
                        : "Sign in to manage uploads",
                    detail: model.phase == .preparingSessions
                        ? "Upload settings will be available when session preparation finishes."
                        : "Connect Agent Relay to choose which sessions are uploaded."
                )
            }
        }
        DisclosureGroup("Upload details") { UploadStagesView().zoomPadding(.top, 12) }
            .zoomFont(size: 13, weight: .medium)
    }
}
