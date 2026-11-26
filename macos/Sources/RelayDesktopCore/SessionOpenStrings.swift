import Foundation

extension SessionOpen {
    /// The fixed words around the Open button (`contract::Strings` in
    /// `desktop-core/src/session_open/contract.rs`). Every answer carries
    /// them; `fallback` is the copy used when no answer could be had (the
    /// helper is missing, timed out or died). `fallback` must equal
    /// `shared/session-open-strings.json`, the output of
    /// `agent-relay-probe session-open strings`: `SessionOpenStringsTests`
    /// fails when they differ. Never edit one without the other.
    public struct Strings: Equatable, Sendable, Codable {
        public var openSession: String
        public var checking: String
        public var opening: String
        public var resumeDefault: String
        public var resolveLaunch: String
        public var launchQuestion: String
        public var launchDetail: String
        public var launchClosed: String
        public var launchWait: String
        public var cancel: String
        public var takeOverDetail: String
        public var helperMissing: String
        public var rejected: String
        public var planFailed: String
        public var openUnknown: String
        public var openNotStarted: String
        public var alreadyOpening: String

        public static let fallback = Strings(
            openSession: "Open session",
            checking: "Checking session location…",
            opening: "Opening session…",
            resumeDefault: "Resume in default terminal",
            resolveLaunch: "Resolve earlier launch…",
            launchQuestion: "Did you close or stop the earlier launch?",
            launchDetail: "The earlier terminal or application may still be starting the provider. Close or stop that launch before allowing another resume request.",
            launchClosed: "I closed or stopped it",
            launchWait: "Keep waiting",
            cancel: "Cancel",
            takeOverDetail: "Agent Relay will stop the background process running this session, check that it exited, then resume the session here.",
            helperMissing: "This copy of Agent Relay is missing its session helper, so it cannot open sessions. Reinstall Agent Relay.",
            rejected: "The session helper could not read this request. Update Agent Relay.",
            planFailed: "Couldn’t check this session’s location. Try again.",
            openUnknown: "Agent Relay did not get an answer, so it is unknown whether the session opened. Check before trying again.",
            openNotStarted: "Agent Relay could not start opening this session. No new session was started.",
            alreadyOpening: "This session is already being opened.")

        public init(openSession: String, checking: String, opening: String, resumeDefault: String,
                    resolveLaunch: String, launchQuestion: String, launchDetail: String, launchClosed: String,
                    launchWait: String, cancel: String, takeOverDetail: String, helperMissing: String,
                    rejected: String, planFailed: String, openUnknown: String, openNotStarted: String,
                    alreadyOpening: String) {
            self.openSession = openSession
            self.checking = checking
            self.opening = opening
            self.resumeDefault = resumeDefault
            self.resolveLaunch = resolveLaunch
            self.launchQuestion = launchQuestion
            self.launchDetail = launchDetail
            self.launchClosed = launchClosed
            self.launchWait = launchWait
            self.cancel = cancel
            self.takeOverDetail = takeOverDetail
            self.helperMissing = helperMissing
            self.rejected = rejected
            self.planFailed = planFailed
            self.openUnknown = openUnknown
            self.openNotStarted = openNotStarted
            self.alreadyOpening = alreadyOpening
        }

        enum CodingKeys: String, CodingKey, CaseIterable {
            case openSession = "open_session", checking, opening, resumeDefault = "resume_default"
            case resolveLaunch = "resolve_launch", launchQuestion = "launch_question"
            case launchDetail = "launch_detail", launchClosed = "launch_closed", launchWait = "launch_wait"
            case cancel, takeOverDetail = "take_over_detail", helperMissing = "helper_missing", rejected
            case planFailed = "plan_failed", openUnknown = "open_unknown"
            case openNotStarted = "open_not_started", alreadyOpening = "already_opening"
        }

        /// Each word a newer or older helper left out is this build's own.
        public init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            let f = Strings.fallback
            func word(_ key: CodingKeys, _ fallback: String) -> String {
                (try? c.decodeIfPresent(String.self, forKey: key)) ?? fallback
            }
            self.init(openSession: word(.openSession, f.openSession), checking: word(.checking, f.checking),
                      opening: word(.opening, f.opening), resumeDefault: word(.resumeDefault, f.resumeDefault),
                      resolveLaunch: word(.resolveLaunch, f.resolveLaunch),
                      launchQuestion: word(.launchQuestion, f.launchQuestion),
                      launchDetail: word(.launchDetail, f.launchDetail),
                      launchClosed: word(.launchClosed, f.launchClosed), launchWait: word(.launchWait, f.launchWait),
                      cancel: word(.cancel, f.cancel), takeOverDetail: word(.takeOverDetail, f.takeOverDetail),
                      helperMissing: word(.helperMissing, f.helperMissing), rejected: word(.rejected, f.rejected),
                      planFailed: word(.planFailed, f.planFailed), openUnknown: word(.openUnknown, f.openUnknown),
                      openNotStarted: word(.openNotStarted, f.openNotStarted),
                      alreadyOpening: word(.alreadyOpening, f.alreadyOpening))
        }
    }
}
