import Foundation
import UIKit

/// How sign-in answers one RFC 8628 device-token error from Cloud.
enum CloudDeviceGrantPoll: Equatable {
    case keepPolling
    case slowDown
    /// The device code can never succeed again: Cloud already used it, the
    /// person denied it, or it expired. Keeping it would replay it on every
    /// launch, so sign-in forgets it and asks the person to start again.
    case startOver(String)
    case fail

    static func classify(message: String, status: Int?) -> CloudDeviceGrantPoll {
        switch message {
        case "authorization_pending": return .keepPolling
        case "slow_down": return .slowDown
        case "invalid_grant":
            return .startOver("This sign-in code was already used or is no longer valid. Start sign-in again.")
        case "expired_token": return .startOver("Sign-in expired. Start again.")
        case "access_denied": return .startOver("Sign-in was declined. Start again to try once more.")
        default:
            if let status, [429, 502, 503].contains(status) { return .keepPolling }
            return .fail
        }
    }
}

/// Background time for one request whose response must not be lost. Cloud
/// consumes the device code when it answers with tokens, so a token response
/// dropped because iOS suspended the app cannot be fetched again. Approval
/// happens on an in-app page, but the person may still switch apps meanwhile
/// (to read a sign-in email, say). Ends at most once.
@MainActor final class BackgroundRequest {
    private var identifier: UIBackgroundTaskIdentifier = .invalid

    init(name: String) {
        identifier = UIApplication.shared.beginBackgroundTask(withName: name) { [weak self] in
            MainActor.assumeIsolated { self?.end() }
        }
    }

    func end() {
        guard identifier != .invalid else { return }
        UIApplication.shared.endBackgroundTask(identifier)
        identifier = .invalid
    }
}
