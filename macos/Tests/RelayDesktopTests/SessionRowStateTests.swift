import XCTest
@testable import RelayDesktop
import RelayDesktopCore

@MainActor final class SessionRowStateTests: XCTestCase {
    private func fixture() -> (AppModel, RelayModel, LocalSessionRow) {
        let defaults = UserDefaults(suiteName: "session-row-state-\(UUID())")!
        let model = AppModel(manageSystemServices: false, defaults: defaults)
        let relay = RelayModel(client: CoreClient(socketPath: "/tmp/session-row-state-unused.sock"))
        let upload = UploadSession(included: false, isAgent: false, sessionId: "state", source: "codex", status: .notShared, title: "Row state")
        return (model, relay, LocalSessionRow.merge(uploads: [upload], sessions: [])[0])
    }

    private func row(_ state: SessionRowState, selected: Bool = false,
                     open: @escaping () -> Void = {}) -> LocalSessionTableRow {
        LocalSessionTableRow(state: state, columns: SessionColumnLayout(), selected: selected,
            actions: SessionRowActions(open: open, toggleSelected: {}, toggleRelay: {}, setHidden: { _ in }, openInCloud: { _ in }))
    }

    func testUnrelatedModelChangesKeepRowStateEqual() {
        let (model, relay, sessionRow) = fixture()
        let before = SessionRowIndex(model: model, relay: relay).state(for: sessionRow)
        model.sessionSearch = "unrelated"
        model.sessionColumns.setVisible(true, for: .model)
        XCTAssertEqual(SessionRowIndex(model: model, relay: relay).state(for: sessionRow), before)
    }

    func testRowStateResolvesUploadPresenceAndStatus() {
        let (model, relay, sessionRow) = fixture()
        let state = SessionRowIndex(model: model, relay: relay).state(for: sessionRow)
        XCTAssertEqual(state.upload?.id, sessionRow.id)
        XCTAssertEqual(state.uploadState, model.uploadState(for: sessionRow.upload!))
        XCTAssertEqual(state.presence, .off)
        XCTAssertFalse(state.canToggleRelay, "Without a running session there is nothing to put on the relay")
        XCTAssertNil(state.notRunningReason, "Without a local session there is nothing to explain")
        XCTAssertEqual(state.activityStatus, "Finished")
    }

    // relay#2239: a session not seen running says why, with what to do next.
    func testNotRunningReasonNamesTheNextStep() {
        XCTAssertEqual(RelayModel.notRunningMessage(for: .codex),
                       "Agent Relay can't see this Codex chat running. Open it in Codex and try again.")
        XCTAssertEqual(RelayModel.notRunningMessage(for: .claudeTerminal),
                       "Agent Relay can't see this session running. Open it and try again.")
    }

    func testRowsRedrawOnlyForTheirOwnValues() {
        let (model, relay, sessionRow) = fixture()
        let state = SessionRowIndex(model: model, relay: relay).state(for: sessionRow)
        XCTAssertEqual(row(state, open: {}), row(state, open: { _ = 1 }), "Handlers do not redraw a row")
        XCTAssertNotEqual(row(state), row(state, selected: true))
    }
}
