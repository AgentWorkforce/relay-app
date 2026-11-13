import Foundation
import OSLog
import ServiceManagement

/// "Open Agent Relay at login", backed by `SMAppService.mainApp`: the menu
/// bar and notifications at login. AppModel applies the same choice to the
/// core's LaunchAgent; with it off, the core is loaded only for this session.
enum LoginItem {
    static let logger = Logger(subsystem: AppInfo.subsystem, category: "loginitem")

    static var status: SMAppService.Status {
        SMAppService.mainApp.status
    }

    /// Every release build opens at login, whichever site it was made for.
    /// Only a development build (Agent Relay Dev, made for a loopback site)
    /// does not: registered, it would relaunch itself at every login.
    static var isAvailable: Bool { !AppInfo.isDevelopment }

    static var isEnabled: Bool {
        status == .enabled
    }

    /// True when macOS wants the person to allow the login item in System
    /// Settings before it will run.
    static var needsApproval: Bool {
        status == .requiresApproval
    }

    /// Turns the login item on or off. Returns true when the change stuck.
    @discardableResult
    static func setEnabled(_ enabled: Bool) -> Bool {
        do {
            if enabled {
                guard isAvailable else { return false }
                if SMAppService.mainApp.status == .enabled { return true }
                try SMAppService.mainApp.register()
            } else {
                if SMAppService.mainApp.status == .notRegistered { return true }
                try SMAppService.mainApp.unregister()
            }
            return true
        } catch {
            logger.error("could not change the login item: \(error.localizedDescription, privacy: .public)")
            return false
        }
    }

    /// Removes the registration a development build holds, so it stops
    /// opening at login.
    static func removeIfUnavailable() {
        if !isAvailable { setEnabled(false) }
    }
}

/// Replaceable so coordinated startup changes can be tested without changing
/// this computer's actual Login Items.
protocol LoginItemManaging {
    var isEnabled: Bool { get }
    var needsApproval: Bool { get }
    @discardableResult func setEnabled(_ enabled: Bool) -> Bool
    func removeIfUnavailable()
}

struct SystemLoginItem: LoginItemManaging {
    var isEnabled: Bool { LoginItem.isEnabled }
    var needsApproval: Bool { LoginItem.needsApproval }
    func setEnabled(_ enabled: Bool) -> Bool { LoginItem.setEnabled(enabled) }
    func removeIfUnavailable() { LoginItem.removeIfUnavailable() }
}
