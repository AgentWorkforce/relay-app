import XCTest
@testable import RelayDesktopCore

final class ViewModelTests: XCTestCase {
    private func status(_ stage: Stage, message: String? = nil, paused: Bool = false, journalFull: Bool = false,
                        stopped: Bool = false, failureCode: DeliveryFailureCode? = nil) -> UploaderStatus {
        Fixture.uploaderStatus(stage: stage, message: message, paused: paused, sharingMode: .selected, total: 3, shared: 1,
                               journalFull: journalFull, stopped: stopped, failureCode: failureCode)
    }

    func testOnlyARunningOpenCodeWindowWithoutThePluginNeedsARestart() {
        let target = Target(id: "ses_one", kind: .opencodeOpen)
        XCTAssertTrue(Fixture.localSession(target: target, title: "t", live: false, running: true).needsOpenCodeRestart)
        // An older core sends no `running`: a non-live row is taken as running.
        XCTAssertTrue(Fixture.localSession(target: target, title: "t", live: false).needsOpenCodeRestart)
        // A registered session whose window closed has ended.
        XCTAssertFalse(Fixture.localSession(target: target, title: "t", live: false, running: false).needsOpenCodeRestart)
        XCTAssertFalse(Fixture.localSession(target: target, title: "t", live: true, running: true).needsOpenCodeRestart)
    }

    func testAConductorSessionNamesItsHost() throws {
        XCTAssertEqual(SessionHost.label(provider: "Codex", host: "conductor"), "Codex in Conductor")
        XCTAssertEqual(SessionHost.label(provider: "Codex", host: "paseo"), "Codex in Paseo")
        XCTAssertEqual(SessionHost.label(provider: "Grok", host: "paseo"), "Grok in Paseo")
        XCTAssertEqual(SessionHost.label(provider: "Muse", host: "paseo"), "Muse in Paseo")
        XCTAssertEqual(SessionHost.label(provider: "Claude app", host: "claude-web"), "Claude Code on the web")
        XCTAssertEqual(SessionHost.claudeCloudPlace("claude-web"), "Claude Code on the web")
        XCTAssertEqual(SessionHost.claudeCloudPlace(nil), "Claude app")
        XCTAssertEqual(SessionHost.label(provider: "Codex", host: nil), "Codex")
        XCTAssertNil(SessionHost.name("somewhere-else"))
        let json = Data(#"{"source":"codex","session_id":"s","title":"Check in","cwd":"/Users/me/conductor/workspaces/app/caracas","git_branch":null,"first_activity_ms":null,"last_activity_ms":null,"is_agent":false,"host":"conductor","included":false,"status":"not_shared"}"#.utf8)
        let row = try JSONDecoder().decode(UploadSession.self, from: json)
        XCTAssertEqual(row.host, "conductor")
        // Including it keeps it a Conductor session.
        XCTAssertEqual(row.withInclusion(true).host, "conductor")
    }

    func testTheCoresStageIsWordedHere() {
        XCTAssertEqual(StatusViewModel(status: status(.upToDate)).headline, "Up to date")
        let attention = StatusViewModel(status: status(.needsAttention, message: "Disk full."))
        XCTAssertEqual(attention.state, .needsAttention("Disk full."))
        XCTAssertEqual(attention.detail, "Disk full.")
        let conflict = StatusViewModel(status: status(.conflict, message: "1 record quarantined; uploads continue."))
        XCTAssertEqual(conflict.headline, "Delivery conflict")
        XCTAssertEqual(conflict.detail, "1 record quarantined; uploads continue.")
        XCTAssertEqual(StatusViewModel(status: status(.offline)).detail, "Agent Relay will try again shortly.")
        XCTAssertEqual(
            StatusViewModel(status: status(.uploading, message: "Server is slow; retrying. 42 sent, 17 queued.")).detail,
            "Server is slow; retrying. 42 sent, 17 queued."
        )
        XCTAssertEqual(
            StatusViewModel(status: Fixture.uploaderStatus(
                stage: .uploading,
                message: "Server is slow; retrying. 42 sent, 17 queued.",
                skippedRecords: 1
            )).detail,
            "Server is slow; retrying. 42 sent, 17 queued. 1 record was skipped during upload."
        )
        let none = Fixture.uploaderStatus(sharingMode: .selected, total: 4, shared: 0)
        XCTAssertEqual(StatusViewModel(status: none).headline, "Choose sessions to upload")
    }

    func testRowStatesFollowTheUploaderStage() {
        let row = Fixture.uploadSession(sessionId: "a", included: true, status: .queued)
        XCTAssertEqual(SessionUploadState(session: row, status: status(.uploading)), .queued)
        XCTAssertEqual(SessionUploadState(session: row, status: status(.offline)), .blocked)
        XCTAssertEqual(SessionUploadState(session: row, status: status(.paused, paused: true)), .paused)
        let local = Fixture.uploadSession(sessionId: "b")
        XCTAssertEqual(SessionUploadState(session: local, status: status(.uploading)), .local)
        XCTAssertTrue(SessionUploadFilter.pending.matches(.queued))
        XCTAssertFalse(SessionUploadFilter.pending.matches(.checking))
    }

    /// The status pill's glyph and pipeline bar; Linux draws the same
    /// glyphs by these names (`SessionUploadState::symbol`).
    func testRowStatesNameTheirGlyphAndProgress() {
        XCTAssertEqual(SessionUploadState.uploaded.symbol, "checkmark.circle.fill")
        XCTAssertEqual(SessionUploadState.local.symbol, "desktopcomputer")
        XCTAssertEqual(SessionUploadState.queued.symbol, "tray.and.arrow.up")
        XCTAssertEqual(SessionUploadState.preparing.pipelineProgress, 0.25)
        XCTAssertEqual(SessionUploadState.queued.pipelineProgress, 0.5)
        XCTAssertEqual(SessionUploadState.uploading.pipelineProgress, 0.75)
        XCTAssertNil(SessionUploadState.local.pipelineProgress)
        XCTAssertNil(SessionUploadState.blocked.pipelineProgress)
    }

    func testUploadActivityFillsUpwardThenFadesWithoutClaimingPercent() {
        XCTAssertEqual(SessionUploadState.local.iconTitle, "Local only")
        XCTAssertEqual(SessionUploadState.uploading.iconTitle, "Uploading…")
        XCTAssertEqual(SessionUploadState.uploaded.iconTitle, "Uploaded")
        XCTAssertEqual(UploadActivityFrame(elapsed: 0).fill, 0)
        XCTAssertEqual(UploadActivityFrame(elapsed: 0.75).fill, 0.5, accuracy: 0.001)
        XCTAssertEqual(UploadActivityFrame(elapsed: 1.5).fill, 1)
        XCTAssertEqual(UploadActivityFrame(elapsed: 1.65).opacity, 0.425, accuracy: 0.001)
        XCTAssertEqual(UploadActivityFrame(elapsed: 2.55).fill, 0.5, accuracy: 0.001)
        for time in [0.0, 0.75, 1.5, 1.65, 2.55] {
            let frame = UploadActivityFrame(elapsed: time, reduceMotion: true)
            XCTAssertEqual(frame.fill, 1)
            XCTAssertEqual(frame.opacity, 0.85)
        }
    }

    func testAFullJournalOffersCompaction() {
        let full = StatusViewModel(status: status(.needsAttention, message: "Full.", journalFull: true))
        let stages = UploadStages.make(catalogReady: true, loadedSessions: 3, preparingHistory: false, capture: nil,
                                       status: full, summary: SessionUploadSummary(status: full, sessions: [], requesting: []))
        XCTAssertEqual(stages[1].state, .blocked)
        XCTAssertEqual(stages[2].action, .compact)
        XCTAssertEqual(stages[2].detail, "Upload journal full")
        // A delivery failure the core ranks first never stands in for the journal.
        let failing = StatusViewModel(status: status(.needsAttention, message: "Sign in again.", journalFull: true))
        let failingStages = UploadStages.make(catalogReady: true, loadedSessions: 3, preparingHistory: false, capture: nil,
                                              status: failing, summary: SessionUploadSummary(status: failing, sessions: [], requesting: []))
        XCTAssertEqual(failingStages[2].detail, "Upload journal full")
    }

    /// Retry or Compact clicked shows the stage as under way, without the
    /// button, until the uploader's next status.
    func testAnActionAskedForShowsTheStageUnderWay() {
        let full = StatusViewModel(status: status(.needsAttention, message: "Full.", journalFull: true))
        let compacting = UploadStages.make(catalogReady: true, loadedSessions: 3, preparingHistory: false, capture: nil,
                                           status: full, summary: SessionUploadSummary(status: full, sessions: [], requesting: []),
                                           pending: .compact)
        XCTAssertEqual(compacting[1].state, .complete, "the journal no longer reads as full")
        XCTAssertEqual(compacting[2].state, .active)
        XCTAssertEqual(compacting[2].detail, "Compacting the upload journal…")
        XCTAssertNil(compacting[2].action)
        let stopped = StatusViewModel(status: status(.needsAttention, message: "Stopped.", stopped: true, failureCode: .invalidPayload))
        let retrying = UploadStages.make(catalogReady: true, loadedSessions: 3, preparingHistory: false, capture: nil,
                                         status: stopped, summary: SessionUploadSummary(status: stopped, sessions: [], requesting: []),
                                         pending: .retry)
        XCTAssertEqual(retrying[2].state, .active)
        XCTAssertEqual(retrying[2].detail, "Retrying uploads…")
        XCTAssertNil(retrying[2].action)
    }

    /// A stopped job never retries on its own, so rows say so and the upload
    /// stage offers the retry instead of claiming one is coming.
    func testStoppedUploadsOfferARetry() {
        let row = Fixture.uploadSession(sessionId: "a", included: true, status: .queued)
        let stopped = status(.needsAttention, message: "An upload could not be prepared safely.", stopped: true,
                             failureCode: .invalidPayload)
        XCTAssertEqual(SessionUploadState(session: row, status: stopped), .stopped)
        XCTAssertEqual(SessionUploadState.stopped.title, "Upload stopped")
        XCTAssertTrue(SessionUploadFilter.pending.matches(.stopped))
        let model = StatusViewModel(status: stopped)
        XCTAssertTrue(model.canRetryUploads)
        let stages = UploadStages.make(catalogReady: true, loadedSessions: 3, preparingHistory: false, capture: nil,
                                       status: model, summary: SessionUploadSummary(status: model, sessions: [], requesting: []))
        XCTAssertEqual(stages[2].state, .blocked)
        XCTAssertEqual(stages[2].action, .retry)

        // Signing in again, not a retry, fixes an expired upload sign-in.
        let signIn = StatusViewModel(status: status(.needsAttention, message: "Sign in again.", stopped: true,
                                                    failureCode: .authenticationRequired))
        XCTAssertFalse(signIn.canRetryUploads)
        let signInStages = UploadStages.make(catalogReady: true, loadedSessions: 3, preparingHistory: false, capture: nil,
                                             status: signIn, summary: SessionUploadSummary(status: signIn, sessions: [], requesting: []))
        XCTAssertNil(signInStages[2].action)
        // A retrying failure is still waiting to retry.
        XCTAssertEqual(SessionUploadState(session: row, status: status(.needsAttention, message: "Stalled.")), .blocked)
        XCTAssertFalse(StatusViewModel(status: status(.needsAttention, message: "Stalled.")).canRetryUploads)
    }

    /// A transient failure that has lasted waits out its backoff on its own,
    /// and Retry tries again at once; a rate limit only waits.
    func testALastingTransientFailureOffersARetryAndARateLimitDoesNot() {
        let row = Fixture.uploadSession(sessionId: "a", included: true, status: .queued)
        let transient = status(.needsAttention, message: "The upload service is temporarily unavailable.",
                               failureCode: .transient)
        XCTAssertEqual(SessionUploadState(session: row, status: transient), .blocked)
        let model = StatusViewModel(status: transient)
        XCTAssertTrue(model.canRetryUploads)
        let stages = UploadStages.make(catalogReady: true, loadedSessions: 3, preparingHistory: false, capture: nil,
                                       status: model, summary: SessionUploadSummary(status: model, sessions: [], requesting: []))
        XCTAssertEqual(stages[2].action, .retry)
        let limited = status(.needsAttention, message: "The upload service asked to slow down.", failureCode: .rateLimited)
        XCTAssertFalse(StatusViewModel(status: limited).canRetryUploads)
        XCTAssertFalse(StatusViewModel(status: status(.uploading, message: nil, failureCode: .transient)).canRetryUploads)
        // An older ai-hist uploader's hold offers Retry for after its removal.
        let held = status(.needsAttention, message: "An older ai-hist uploader is still scheduled.",
                          failureCode: .legacyUploaderActive)
        XCTAssertTrue(StatusViewModel(status: held).canRetryUploads)
    }

    func testMessageAvailabilityOnlyGoesOnlineAfterPushIsLive() {
        XCTAssertEqual(RelayPresence.off.messageStatus, "Off")
        XCTAssertEqual(RelayPresence.joining.messageStatus, "Connecting…")
        XCTAssertEqual(RelayPresence.connecting.messageStatus, "Connecting…")
        XCTAssertEqual(RelayPresence.online.messageStatus, "Relay connected")
        XCTAssertEqual(RelayPresence.offline.messageStatus, "Offline")
        XCTAssertEqual(RelayPresence.leaving.messageStatus, "Turning off…")
        XCTAssertFalse(RelayPresence.leaving.isOn)
        XCTAssertTrue(RelayPresence.joining.isOn)
    }

    func testPresenceShowsTheStateAskedForFirst() {
        XCTAssertEqual(RelayPresence.of(onRelay: false, isLive: true, pushLive: nil), .off)
        XCTAssertEqual(RelayPresence.of(onRelay: false, wanted: true, isLive: true, pushLive: nil), .joining)
        XCTAssertEqual(RelayPresence.of(onRelay: true, wanted: false, isLive: true, pushLive: true), .leaving)
        XCTAssertEqual(RelayPresence.of(onRelay: true, isLive: false, pushLive: true), .offline)
        XCTAssertEqual(RelayPresence.of(onRelay: true, isLive: true, pushLive: nil), .connecting)
        XCTAssertNil(Target(id: "session_1", kind: .claudeCloud).uploadKey)
        XCTAssertEqual(Target(id: "ses_1", kind: .opencode).uploadKey, "opencode:ses_1")
        XCTAssertEqual(Target(id: "ses_1", kind: .opencodeOpen).uploadKey, "opencode:ses_1")
        XCTAssertEqual(Target(id: "m", kind: .museOpen).uploadKey, "muse:m")
        XCTAssertEqual(Target(id: "real-gallium", kind: .devin).uploadKey, "devin:real-gallium")
        XCTAssertNil(Target(id: "p", kind: .pi).uploadKey)
        XCTAssertEqual(SessionKind.opencode.label, "OpenCode")
        XCTAssertEqual(Target(id: "x", kind: .claudeTerminal).uploadKey, "claude:x")
        XCTAssertEqual(Target(id: "g", kind: .grok).uploadKey, "grok:g")
        XCTAssertEqual(Target(id: "g", kind: .grokOpen).uploadKey, "grok:g")
    }

    func testFiltersMatchWhatTheRowShows() {
        let entry = Fixture.uploadSession(source: "claude", sessionId: "x1", title: "Ship it", cwd: "/src/app/")
        XCTAssertEqual(entry.displayProject, "app")
        XCTAssertEqual(entry.displayTitle, "Ship it")
        XCTAssertTrue(SessionFilter.matches(entry, query: "APP"))
    }

}

final class LegacyMigrationTests: XCTestCase {
    func testConversationsMoveIntoTheAppDataDirectoryOnce() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("migration-\(UUID())")
        defer { try? FileManager.default.removeItem(at: root) }
        let legacy = LegacyMigration.legacyConversations(supportDirectory: root)
        let destination = root.appendingPathComponent("com.agentrelay.desktop/Conversations")
        try FileManager.default.createDirectory(at: legacy, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
        try Data("old".utf8).write(to: legacy.appendingPathComponent("a.json"))
        try Data("stale".utf8).write(to: legacy.appendingPathComponent("b.json"))
        try Data("core".utf8).write(to: destination.appendingPathComponent("b.json"))

        try LegacyMigration.moveConversations(from: legacy, to: destination)

        XCTAssertEqual(try String(contentsOf: destination.appendingPathComponent("a.json")), "old")
        XCTAssertEqual(try String(contentsOf: destination.appendingPathComponent("b.json")), "core")
        XCTAssertFalse(FileManager.default.fileExists(atPath: legacy.deletingLastPathComponent().path))
        // Nothing left to move: a second run changes nothing.
        try LegacyMigration.moveConversations(from: legacy, to: destination)
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: destination.path).sorted(), ["a.json", "b.json"])
    }

    func testOnlyAnOptOutAndTheContractorFlagReachTheCore() throws {
        let suite = "migration-\(UUID())"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        XCTAssertNil(LegacyMigration.settings(in: defaults))
        defaults.set(true, forKey: LegacyMigration.analyticsEnabledKey)
        XCTAssertNil(LegacyMigration.settings(in: defaults))
        defaults.set(false, forKey: LegacyMigration.analyticsEnabledKey)
        defaults.set(true, forKey: LegacyMigration.contractorAccessKey)
        XCTAssertEqual(LegacyMigration.settings(in: defaults), SettingsChange(analyticsEnabled: false, contractorAccess: true))
    }
}
