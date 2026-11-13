import AuthenticationServices
import SafariServices
import SwiftUI
import UIKit

/// Shows Cloud's device-approval page inside the app with
/// `ASWebAuthenticationSession`, so sign-in and account creation never leave
/// for Safari (App Review guideline 4).
///
/// Sign-in is an RFC 8628 device grant: `RelayClient` polls Cloud while the
/// person approves on the page, and the page has no redirect back to the app.
/// So the session has no callback scheme and ends one of three ways:
/// - the person taps Cancel: the grant keeps polling and **Continue sign-in**
///   reopens the page;
/// - the grant finishes, fails or is replaced: the view sees
///   `verificationURL` change and calls `close()`, which dismisses the page;
/// - the app is signed out: same as above, `verificationURL` becomes nil.
/// The page shares Safari's cookies (`prefersEphemeralWebBrowserSession` is
/// false), so a person already signed in on agentrelay.com only approves.
@MainActor final class CloudWebSignIn: NSObject, ObservableObject {
    @Published private(set) var openURL: URL?
    private var session: ASWebAuthenticationSession?
    /// Identifies the showing session, so a late completion from an earlier
    /// one can't clear it.
    private var sessionID = UUID()

    /// Opens the page for `url`, unless it is already showing.
    func open(_ url: URL) {
        if session != nil {
            if openURL == url { return }
            close()
        }
        let id = UUID()
        let newSession = ASWebAuthenticationSession(url: url, callbackURLScheme: nil) { [weak self] _, error in
            // Cancel, a failed load or `close()`; the grant's polling is
            // independent of the page, so none of these changes sign-in.
            if let error, (error as? ASWebAuthenticationSessionError)?.code != .canceledLogin {
                NSLog("Agent Relay sign-in page closed: %@", error.localizedDescription)
            }
            Task { @MainActor in self?.ended(id) }
        }
        newSession.presentationContextProvider = self
        newSession.prefersEphemeralWebBrowserSession = false
        session = newSession
        sessionID = id
        openURL = url
        if !newSession.start() {
            session = nil
            openURL = nil
        }
    }

    /// Dismisses the page, if it is showing.
    func close() {
        let showing = session
        session = nil
        openURL = nil
        showing?.cancel()
    }

    private func ended(_ id: UUID) {
        // A later `open` may already show another page.
        guard sessionID == id, session != nil else { return }
        session = nil
        openURL = nil
    }
}

extension CloudWebSignIn: ASWebAuthenticationPresentationContextProviding {
    nonisolated func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        MainActor.assumeIsolated { InAppWindow.key() }
    }
}

/// The window the app shows now, on iPhone and iPad.
@MainActor enum InAppWindow {
    static func key() -> UIWindow {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        let scene = scenes.first { $0.activationState == .foregroundActive } ?? scenes.first
        if let window = scene?.windows.first(where: \.isKeyWindow) ?? scene?.windows.first {
            return window
        }
        if let scene { return UIWindow(windowScene: scene) }
        return UIWindow()
    }
}

/// An agentrelay.com page in an in-app Safari view (`SFSafariViewController`),
/// presented as a sheet; **Done** closes the sheet. Used for account deletion,
/// which is a page to read and act on rather than a sign-in.
struct InAppSafariPage: UIViewControllerRepresentable {
    let url: URL
    let onDone: () -> Void

    func makeCoordinator() -> Coordinator { Coordinator(onDone: onDone) }

    func makeUIViewController(context: Context) -> SFSafariViewController {
        let controller = SFSafariViewController(url: url)
        controller.dismissButtonStyle = .done
        controller.delegate = context.coordinator
        return controller
    }

    func updateUIViewController(_ controller: SFSafariViewController, context: Context) {
        context.coordinator.onDone = onDone
    }

    final class Coordinator: NSObject, SFSafariViewControllerDelegate {
        var onDone: () -> Void

        init(onDone: @escaping () -> Void) { self.onDone = onDone }

        func safariViewControllerDidFinish(_ controller: SFSafariViewController) { onDone() }
    }
}
