import UserNotifications

extension NotificationPermission {
    /// What the core is told of macOS's saved notification choice for the
    /// app. Read with `getNotificationSettings`, which never prompts.
    /// (`ephemeral` is iOS only and not in the macOS SDK.)
    public init(authorization: UNAuthorizationStatus) {
        switch authorization {
        case .authorized, .provisional: self = .allowed
        case .denied: self = .denied
        case .notDetermined: self = .unknown
        @unknown default: self = .unknown
        }
    }
}
