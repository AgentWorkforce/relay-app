import SwiftUI
import UIKit
import XCTest
@testable import AgentRelayiPad

/// Stands in for the client: history the content reads but that nothing
/// above the content observes, as with a chat pushed onto a navigation stack.
private final class History: ObservableObject {
    @Published var ids = ["handoff-original"]
}

/// What the content saw each time it was built, and each change its
/// `onChange` reported.
private final class Builds {
    var seen: [[String]] = []
    var changes: [[String]] = []
}

@MainActor final class ObservingContentTests: XCTestCase {
    private func show<V: View>(_ view: V) -> (UIWindow, UIHostingController<V>) {
        let controller = UIHostingController(rootView: view)
        let scene = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.first
        let window = scene.map(UIWindow.init(windowScene:)) ?? UIWindow()
        window.frame = CGRect(x: 0, y: 0, width: 390, height: 844)
        window.rootViewController = controller
        window.makeKeyAndVisible()
        controller.view.layoutIfNeeded()
        return (window, controller)
    }

    /// Lets SwiftUI apply a published change, up to about a second.
    private func settle(_ controller: UIViewController, until done: () -> Bool) async {
        var rounds = 0
        while !done(), rounds < 50 {
            try? await Task.sleep(for: .milliseconds(20))
            controller.view.setNeedsLayout()
            controller.view.layoutIfNeeded()
            rounds += 1
        }
    }

    func testTheContentIsBuiltAgainWhenTheObjectPublishes() async {
        let history = History()
        let builds = Builds()
        let (window, controller) = show(ObservingContent(object: history) {
            let _ = builds.seen.append(history.ids)
            Text(history.ids.joined(separator: ","))
        })
        defer { window.isHidden = true }
        XCTAssertEqual(builds.seen.last, ["handoff-original"])

        history.ids = ["handoff-send", "handoff-original"]
        await settle(controller) { builds.seen.last == history.ids }

        XCTAssertEqual(builds.seen.last, ["handoff-send", "handoff-original"])
    }

    func testAChangeTheObjectPublishesReachesTheContentsOnChange() async {
        let history = History()
        let builds = Builds()
        let (window, controller) = show(ObservingContent(object: history) {
            Text(history.ids.joined(separator: ","))
                .onChange(of: history.ids) { _, ids in builds.changes.append(ids) }
        })
        defer { window.isHidden = true }
        XCTAssertEqual(builds.changes, [])

        history.ids = ["handoff-send", "handoff-original"]
        await settle(controller) { !builds.changes.isEmpty }

        XCTAssertEqual(builds.changes, [["handoff-send", "handoff-original"]])
    }
}
