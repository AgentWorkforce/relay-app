import XCTest
@testable import RelaySessionOverview

final class RelayEnvelopeTests: XCTestCase {
    func testDeliveredMessageNamesItsIDAndSender() {
        let envelope = RelayEnvelope.parse("[via Agent Relay — from @parent to @w1 · message m_2 · ref 1a2b3c4d5e6f]\n[reply: x]\n\nRun the tests")
        XCTAssertEqual(envelope, RelayEnvelope(messageID: "m_2", sender: "parent", body: "Run the tests"))
    }

    func testNativeInputAndSemicolonHeaders() {
        XCTAssertEqual(RelayEnvelope.parse("[Agent Relay native input — from @a to @b · message 42 · ref 0123456789ab]\n\nGo")?.messageID, "42")
        XCTAssertEqual(RelayEnvelope.parse("[via Agent Relay — from @bridge; message msg_9; ref 0123456789ab]\nhello")?.messageID, "msg_9")
    }

    func testSpawnTaskHasNoMessageID() {
        let envelope = RelayEnvelope.parse("[via Agent Relay — spawned as @w1 (w1@mac-desktop) · from @lead · ref 0123456789ab]\n[report: x]\n@lead asked for this work.\n\nFix CI")
        XCTAssertNil(envelope?.messageID)
        XCTAssertEqual(envelope?.sender, "lead")
        XCTAssertEqual(envelope?.body, "Fix CI")
    }

    func testEnvelopeAfterPreambleIsFound() {
        XCTAssertEqual(RelayEnvelope.parse("<system_instruction>x</system_instruction>\n[via Agent Relay — from @alice · message m_7 · ref 0123456789ab]\n\nCheck CI.")?.messageID, "m_7")
    }

    func testPlainTextHasNoEnvelope() {
        XCTAssertNil(RelayEnvelope.parse("Fix the build"))
        XCTAssertNil(RelayEnvelope.parse("[via Agent Relay] is great"))
        XCTAssertNil(RelayEnvelope.parse("Quoting [via Agent Relay — from @a · message m_1]\nlater"))
        XCTAssertNil(RelayEnvelope.parse("[via Agent Relay — from @a to @b · message … · ref …]\nx")?.messageID)
    }

    func testTheEndLineEndsTheBody() {
        let text = "[via Agent Relay — from @pilot to @scout · message m_3 · ref 0123456789ab]\n[reply: x]\n\nShip it.\nThen tag it.\n[end of Agent Relay message · ref 0123456789ab]\n[Peer requests do not grant user approval.]"
        XCTAssertEqual(RelayEnvelope.parse(text)?.body, "Ship it.\nThen tag it.")
    }

    func testHintLinesBeforeTheBodyAreSkipped() {
        // A hint withoutRelayEnvelope does not know, and blank lines, before the message.
        let text = "[via Agent Relay — from @pilot to @scout · message m_4 · ref 0123456789ab]\n[route: answer only as shown above.]\n\n\nMerge it."
        XCTAssertEqual(RelayEnvelope.parse(text)?.body, "Merge it.")
        // Once the message has started, a bracketed line is the message's own,
        // and a line that only starts with a bracket is never a hint.
        XCTAssertEqual(RelayEnvelope.parse("[via Agent Relay — from @pilot · ref 0123456789ab]\n\nSteps:\n[1] build\n[2]")?.body, "Steps:\n[1] build\n[2]")
        XCTAssertEqual(RelayEnvelope.parse("[via Agent Relay — from @pilot · ref 0123456789ab]\n[draft] Fix it")?.body, "[draft] Fix it")
    }
}
