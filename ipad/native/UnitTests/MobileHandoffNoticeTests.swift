import XCTest
@testable import AgentRelayiPad

final class MobileHandoffNoticeTests: XCTestCase {
    func testMissingAndUnknownMetadataNeverClaimAContinuation() {
        XCTAssertNil(MobileHandoffNotice(metadata: [:]))
        XCTAssertNil(MobileHandoffNotice(metadata: ["mobile_handoff_state": "authorized"]))
        XCTAssertNil(MobileHandoffNotice(metadata: ["mobile_handoff_state": "returning"]))
        XCTAssertNil(MobileHandoffNotice(metadata: ["status": "active", "read": true]))
    }

    func testPublishedStatePreservesSessionAndConversationIdentity() {
        let identity: JSONObject = ["source": "relay-desktop", "session_kind": "claude-terminal", "session_id": "original"]
        let before = RelayAgent(relay: ["name": "reviewer", "address": "reviewer@desk-id", "metadata": identity])
        var metadata = identity
        metadata["mobile_handoff_state"] = "ready"
        let after = RelayAgent(relay: ["name": "reviewer", "address": "reviewer@desk-id", "metadata": metadata])
        XCTAssertEqual(before.id, after.id)
        XCTAssertEqual(before.sessionID, after.sessionID)
        XCTAssertNil(before.mobileHandoff)
        XCTAssertEqual(after.mobileHandoff?.state, .ready)
        XCTAssertFalse(MessageDelivery.delivered(readers: [], sender: "person"))
    }

    func testQueuedRequiresExplicitNativeHandoffCapability() {
        var metadata: JSONObject = ["source": "relay-desktop", "session_kind": "claude-terminal", "session_id": "original"]
        let olderCore = RelayAgent(relay: ["name": "reviewer", "metadata": metadata])
        XCTAssertFalse(olderCore.mobileHandoffEnabled)
        XCTAssertEqual(olderCore.deliveryLabel(delivered: false), "Sent")
        metadata["mobile_handoff_enabled"] = false
        let disabled = RelayAgent(relay: ["name": "reviewer", "metadata": metadata])
        XCTAssertEqual(disabled.deliveryLabel(delivered: false), "Sent")
        metadata["mobile_handoff_enabled"] = true
        let enabled = RelayAgent(relay: ["name": "reviewer", "metadata": metadata])
        XCTAssertEqual(enabled.deliveryLabel(delivered: false), "Queued")
        XCTAssertEqual(enabled.deliveryLabel(delivered: true), "Delivered")
        XCTAssertEqual(disabled.deliveryLabel(delivered: true), "Delivered")
    }

    func testFailureDetailsOnlyAccompanyFailedTransitionsAndAreBounded() {
        let detail = String(repeating: "a", count: 700)
        let failed = MobileHandoffNotice(metadata: ["mobile_handoff_state": "failed", "mobile_handoff_error": detail])
        XCTAssertEqual(failed?.detail.count, 500)
        let connecting = MobileHandoffNotice(metadata: ["mobile_handoff_state": "connecting", "mobile_handoff_error": detail])
        XCTAssertEqual(connecting?.detail, "")
    }

    func testGenericUnconfirmedTextIsReplacedWithGuidance() {
        let generic = MobileHandoffNotice(metadata: ["mobile_handoff_state": "failed",
                                                     "mobile_handoff_error": MobileHandoffNotice.unconfirmedDetail])
        XCTAssertEqual(MobileHandoffNotice.unconfirmedDetail,
                       "Claude's mobile connection could not be confirmed. Check the message status before sending again.")
        XCTAssertEqual(generic?.detail, MobileHandoffNotice.unconfirmedDetail, "the published detail is kept as sent")
        XCTAssertEqual(generic?.displayDetail, MobileHandoffNotice.unconfirmedGuidance)
        XCTAssertEqual(generic?.displayHint, MobileHandoffNotice.unconfirmedHint)
        for label in ["Delivered", "Queued", "Sent"] {
            XCTAssertTrue(MobileHandoffNotice.unconfirmedGuidance.contains(label), label)
        }
        XCTAssertTrue(MobileHandoffNotice.unconfirmedHint.contains("/reload-plugins"))

        let specific = "Restart or reload Claude to enable mobile control, then send the message again. This message was not submitted."
        let other = MobileHandoffNotice(metadata: ["mobile_handoff_state": "failed", "mobile_handoff_error": specific])
        XCTAssertEqual(other?.displayDetail, specific)
        XCTAssertNil(other?.displayHint)
        let empty = MobileHandoffNotice(metadata: ["mobile_handoff_state": "failed"])
        XCTAssertEqual(empty?.displayDetail, "")
        XCTAssertNil(empty?.displayHint)
        let ready = MobileHandoffNotice(metadata: ["mobile_handoff_state": "ready"])
        XCTAssertEqual(ready?.displayDetail, "")
        XCTAssertNil(ready?.displayHint)
    }

    func testDismissalHidesOnlyTheSameFailureUntilItChangesOrDisappears() throws {
        let failed = try XCTUnwrap(MobileHandoffNotice(metadata: ["mobile_handoff_state": "failed",
                                                                  "mobile_handoff_error": MobileHandoffNotice.unconfirmedDetail]))
        let otherFailure = try XCTUnwrap(MobileHandoffNotice(metadata: ["mobile_handoff_state": "failed",
                                                                        "mobile_handoff_error": "Something else"]))
        let ready = try XCTUnwrap(MobileHandoffNotice(metadata: ["mobile_handoff_state": "ready"]))
        let reviewer = MobileHandoffDismissals.identity(name: "reviewer", sessionID: "s1")
        let reregistered = MobileHandoffDismissals.identity(name: "reviewer", sessionID: "s2")
        var dismissals = MobileHandoffDismissals()

        dismissals.dismiss(ready, for: reviewer)
        XCTAssertFalse(dismissals.hides(ready, for: reviewer), "only a failed notice can be dismissed")

        dismissals.dismiss(failed, for: reviewer)
        XCTAssertTrue(dismissals.hides(failed, for: reviewer))
        XCTAssertFalse(dismissals.hides(otherFailure, for: reviewer), "a changed detail shows again")
        XCTAssertFalse(dismissals.hides(failed, for: reregistered), "another session with the same name")
        XCTAssertFalse(dismissals.hides(failed, for: MobileHandoffDismissals.identity(name: "builder", sessionID: "s1")))

        // Refreshes that publish the same failure keep it hidden.
        dismissals = dismissals.retaining(published: [reviewer: failed])
        XCTAssertTrue(dismissals.hides(failed, for: reviewer))

        // A different notice forgets the dismissal, so the old failure
        // published again is shown again.
        var changed = dismissals.retaining(published: [reviewer: ready])
        XCTAssertFalse(changed.hides(failed, for: reviewer))
        changed = dismissals.retaining(published: [reviewer: otherFailure])
        XCTAssertFalse(changed.hides(failed, for: reviewer))

        // So does the notice disappearing.
        let cleared = dismissals.retaining(published: [:])
        XCTAssertFalse(cleared.hides(failed, for: reviewer))
        XCTAssertEqual(cleared, MobileHandoffDismissals())
    }

    func testSendingAgainShowsTheSameFailureAgain() throws {
        let failed = try XCTUnwrap(MobileHandoffNotice(metadata: ["mobile_handoff_state": "failed",
                                                                  "mobile_handoff_error": MobileHandoffNotice.unconfirmedDetail]))
        let reviewer = MobileHandoffDismissals.identity(name: "reviewer", sessionID: "s1")
        let builder = MobileHandoffDismissals.identity(name: "builder", sessionID: "s1")
        var dismissals = MobileHandoffDismissals()
        dismissals.dismiss(failed, for: reviewer)

        // A send to another agent leaves this dismissal alone.
        dismissals.forget(builder)
        XCTAssertTrue(dismissals.hides(failed, for: reviewer))

        // A send to this agent is a new attempt: the same failure shows again
        // even though no refresh saw the state change in between.
        dismissals.forget(reviewer)
        XCTAssertFalse(dismissals.hides(failed, for: reviewer))
        XCTAssertEqual(dismissals, MobileHandoffDismissals())
    }

    func testOtherAgentsRequireExplicitDesktopNativeMetadata() {
        for provider in ["pi", "opencode", "codex"] {
            var metadata: JSONObject = ["source": "relay-desktop", "cli": provider]
            let ordinary = RelayAgent(relay: ["name": "agent", "metadata": metadata])
            XCTAssertFalse(ordinary.mobileNativeContinuationEnabled)
            XCTAssertEqual(ordinary.deliveryLabel(delivered: false), "Sent")
            XCTAssertNil(ordinary.nativeContinuationNotice)
            metadata["mobile_native_continuation_enabled"] = true
            let enabled = RelayAgent(relay: ["name": "agent", "metadata": metadata])
            XCTAssertFalse(enabled.mobileHandoffEnabled, "Claude consent is separate")
            XCTAssertEqual(enabled.deliveryLabel(delivered: false), "Queued")
            XCTAssertEqual(enabled.deliveryLabel(delivered: true), "Delivered")
            XCTAssertTrue(enabled.nativeContinuationNotice?.contains("Tool approvals stay on the computer.") == true)
            XCTAssertTrue(enabled.nativeContinuationNotice?.contains("verified computer owner") == true)
            metadata["source"] = "other"
            let nonDesktop = RelayAgent(relay: ["name": "agent", "metadata": metadata])
            XCTAssertEqual(nonDesktop.deliveryLabel(delivered: false), "Sent")
            XCTAssertNil(nonDesktop.nativeContinuationNotice)
        }
    }

    func testConversationSnapshotPreservesExplicitNativeCapability() {
        let base: JSONObject = ["participants": [["agent_name": "pi-session"]],
            "desktop_registration": true, "cli": "pi"]
        let older = RelayAgent(conversation: base)
        XCTAssertFalse(older.mobileNativeContinuationEnabled)
        XCTAssertEqual(older.deliveryLabel(delivered: false), "Sent")
        var current = base
        current["mobile_native_continuation_enabled"] = true
        let enabled = RelayAgent(conversation: current)
        XCTAssertTrue(enabled.mobileNativeContinuationEnabled)
        XCTAssertEqual(enabled.deliveryLabel(delivered: false), "Queued")
        XCTAssertNil(enabled.mobileHandoff)
        current["mobile_native_continuation_enabled"] = "true"
        XCTAssertFalse(RelayAgent(conversation: current).mobileNativeContinuationEnabled)
    }
    func testCodexApprovalNoticeRequiresExactLiveCapabilityAndKeepsOtherPromptsLocal() {
        let base: JSONObject = ["source": "relay-desktop", "cli": "codex", "mobile_native_continuation_enabled": true]
        for mode: Any in ["desktop", "mobile", "unknown", true] {
            var metadata = base
            metadata["mobile_native_continuation_approvals"] = mode
            let agent = RelayAgent(relay: ["name": "codex-session", "metadata": metadata])
            XCTAssertFalse(agent.mobileNativeCommandFileApprovals)
            XCTAssertTrue(agent.nativeContinuationNotice?.contains("Tool approvals stay on the computer.") == true)
        }
        var metadata = base
        metadata["mobile_native_continuation_approvals"] = "command_file"
        let supported = RelayAgent(relay: ["name": "codex-session", "metadata": metadata])
        XCTAssertTrue(supported.mobileNativeCommandFileApprovals)
        XCTAssertTrue(supported.nativeContinuationNotice?.contains("for messages sent here") == true)
        XCTAssertTrue(supported.nativeContinuationNotice?.contains("Other prompts stay on the computer.") == true)
        metadata["cli"] = "pi"
        XCTAssertTrue(RelayAgent(relay: ["name": "pi", "metadata": metadata]).nativeContinuationNotice?.contains("Tool approvals stay on the computer.") == true)
        metadata["mobile_native_continuation_enabled"] = false
        XCTAssertNil(RelayAgent(relay: ["name": "codex", "metadata": metadata]).nativeContinuationNotice)
    }

    func testConversationSnapshotPreservesScopedCodexApprovals() {
        var saved: JSONObject = ["participants": [["agent_name": "codex-session"]], "desktop_registration": true,
            "cli": "codex", "mobile_native_continuation_enabled": true, "mobile_native_continuation_approvals": "command_file"]
        XCTAssertTrue(RelayAgent(conversation: saved).mobileNativeCommandFileApprovals)
        saved.removeValue(forKey: "mobile_native_continuation_approvals")
        XCTAssertFalse(RelayAgent(conversation: saved).mobileNativeCommandFileApprovals)
    }

}
