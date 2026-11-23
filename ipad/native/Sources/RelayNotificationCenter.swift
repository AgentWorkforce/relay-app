import Foundation
import UserNotifications

/// The notification centre calls RelayClient makes, replaceable in tests.
struct RelayNotificationCenter {
    var delivered: () async -> [ConversationReadState.Delivered]
    var remove: ([String]) -> Void
    var post: (UNNotificationRequest) -> Void

    static let system = RelayNotificationCenter(
        delivered: {
            await UNUserNotificationCenter.current().deliveredNotifications().map {
                (identifier: $0.request.identifier, userInfo: $0.request.content.userInfo, date: $0.date)
            }
        },
        remove: { UNUserNotificationCenter.current().removeDeliveredNotifications(withIdentifiers: $0) },
        post: { UNUserNotificationCenter.current().add($0) })
}
