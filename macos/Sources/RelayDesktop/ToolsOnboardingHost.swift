import SwiftUI

/// An unavailable or older core leaves the normal app usable.
struct ToolsOnboardingHost<Content: View>: View {
    var tools: ToolsModel
    let identity: String
    let isCoreConnected: Bool
    let isSignedIn: Bool
    @ViewBuilder let content: () -> Content

    var body: some View {
        content()
            .task(id: "\(identity)|\(isCoreConnected)|\(isSignedIn)") {
                if isCoreConnected { await tools.load(identity: identity) }
            }
            .sheet(isPresented: Binding(
                get: { isCoreConnected && isSignedIn && tools.shouldChooseTools },
                set: { if !$0 && isCoreConnected && isSignedIn { tools.continueToApp() } }
            )) {
                ChooseToolsView(tools: tools)
            }
    }
}
