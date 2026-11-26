import Foundation

/// The JSON the bundled helper's `session-open plan|open|release` exchanges
/// (`desktop-core/src/session_open/contract.rs` is the source of truth).
/// Every answer field is decoded leniently and every enum keeps the string a
/// newer helper sent (`.unknown`), so an older app never fails on a newer
/// helper. Words the person sees come from `title`, `message`, `question`
/// and `strings` (`SessionOpenStrings.swift`), which
/// `desktop-core/src/session_open/wording.rs` shares with GTK.
public enum SessionOpen {
    /// One look at, or one click on, a session's Open button. The helper
    /// refuses unknown fields, so only these are encoded.
    public struct Request: Encodable, Equatable, Sendable {
        public var source: String
        public var sessionId: String
        public var appDataDir: String
        public var directory: String?
        public var useDefaultTerminal = false
        /// The person's "Open with: Terminal" for this provider
        /// (`sessionOpening.<source>`).
        public var preferTerminal = false
        /// One of the contract's `TERMINAL_IDS`; nil defers to the system's
        /// default terminal.
        public var defaultTerminal: String?
        public var confirmed = false
        public var launchGone = false
        public var takeOver = false
        public var selected: String?

        public init(source: String, sessionId: String, appDataDir: String, directory: String? = nil,
                    useDefaultTerminal: Bool = false, preferTerminal: Bool = false,
                    defaultTerminal: String? = nil, confirmed: Bool = false,
                    launchGone: Bool = false, takeOver: Bool = false, selected: String? = nil) {
            self.source = source
            self.sessionId = sessionId
            self.appDataDir = appDataDir
            self.directory = directory
            self.useDefaultTerminal = useDefaultTerminal
            self.preferTerminal = preferTerminal
            self.defaultTerminal = defaultTerminal
            self.confirmed = confirmed
            self.launchGone = launchGone
            self.takeOver = takeOver
            self.selected = selected
        }

        private enum CodingKeys: String, CodingKey {
            case source, directory, confirmed, selected
            case sessionId = "session_id", appDataDir = "app_data_dir"
            case useDefaultTerminal = "use_default_terminal", preferTerminal = "prefer_terminal"
            case defaultTerminal = "default_terminal"
            case launchGone = "launch_gone", takeOver = "take_over"
        }

        public func encode(to encoder: Encoder) throws {
            var c = encoder.container(keyedBy: CodingKeys.self)
            try c.encode(source, forKey: .source)
            try c.encode(sessionId, forKey: .sessionId)
            try c.encode(appDataDir, forKey: .appDataDir)
            try c.encodeIfPresent(directory, forKey: .directory)
            try c.encode(useDefaultTerminal, forKey: .useDefaultTerminal)
            try c.encode(preferTerminal, forKey: .preferTerminal)
            try c.encodeIfPresent(defaultTerminal, forKey: .defaultTerminal)
            try c.encode(confirmed, forKey: .confirmed)
            try c.encode(launchGone, forKey: .launchGone)
            try c.encode(takeOver, forKey: .takeOver)
            try c.encodeIfPresent(selected, forKey: .selected)
        }
    }

    /// What the Open button would do now.
    public enum Action: Equatable, Sendable, Decodable {
        case focus, showApplication, openLink, resume, takeOver, attach, choose, confirm, unavailable
        case unknown(String)

        public init(raw: String) {
            switch raw {
            case "focus": self = .focus
            case "show_application": self = .showApplication
            case "open_link": self = .openLink
            case "resume": self = .resume
            case "take_over": self = .takeOver
            case "attach": self = .attach
            case "choose": self = .choose
            case "confirm": self = .confirm
            case "unavailable": self = .unavailable
            default: self = .unknown(raw)
            }
        }

        public init(from decoder: Decoder) throws { self.init(raw: try decoder.singleValueContainer().decode(String.self)) }
    }

    public enum State: Equatable, Sendable, Decodable {
        case live, liveMultiple, liveNotLocatable, liveHeadless, stopped, archived, notFound, launchPending
        case unknownState, invalidId, unsupported
        /// A state this build does not know.
        case unknown(String)

        public init(raw: String) {
            switch raw {
            case "live": self = .live
            case "live_multiple": self = .liveMultiple
            case "live_not_locatable": self = .liveNotLocatable
            case "live_headless": self = .liveHeadless
            case "stopped": self = .stopped
            case "archived": self = .archived
            case "not_found": self = .notFound
            case "launch_pending": self = .launchPending
            case "unknown": self = .unknownState
            case "invalid_id": self = .invalidId
            case "unsupported": self = .unsupported
            default: self = .unknown(raw)
            }
        }

        public init(from decoder: Decoder) throws { self.init(raw: try decoder.singleValueContainer().decode(String.self)) }
    }

    public enum Precision: Equatable, Sendable, Decodable {
        case exactConversation, rightWindow, appOnly
        case unknown(String)

        public init(raw: String) {
            switch raw {
            case "exact_conversation": self = .exactConversation
            case "right_window": self = .rightWindow
            case "app_only": self = .appOnly
            default: self = .unknown(raw)
            }
        }

        public init(from decoder: Decoder) throws { self.init(raw: try decoder.singleValueContainer().decode(String.self)) }
    }

    public enum Outcome: Equatable, Sendable, Decodable {
        case verified, dispatchedUnverified, refused, failed
        case unknown(String)

        public init(raw: String) {
            switch raw {
            case "verified": self = .verified
            case "dispatched_unverified": self = .dispatchedUnverified
            case "refused": self = .refused
            case "failed": self = .failed
            default: self = .unknown(raw)
            }
        }

        public init(from decoder: Decoder) throws { self.init(raw: try decoder.singleValueContainer().decode(String.self)) }
    }

    /// Where a click goes: the terminal icon is shown exactly for `.terminal`.
    public enum Destination: Equatable, Sendable, Decodable {
        case terminal, desktopApp, none
        case unknown(String)

        public init(raw: String) {
            switch raw {
            case "terminal": self = .terminal
            case "desktop_app": self = .desktopApp
            case "none": self = .none
            default: self = .unknown(raw)
            }
        }

        public init(from decoder: Decoder) throws { self.init(raw: try decoder.singleValueContainer().decode(String.self)) }
    }

    /// One live place a `choose` answer offers.
    public struct Choice: Equatable, Sendable, Decodable, Identifiable {
        public let id: String
        public let label: String
        public init(id: String, label: String) {
            self.id = id
            self.label = label
        }
    }

    /// The Open button's state.
    public struct Plan: Equatable, Sendable, Decodable {
        public var state: State
        public var action: Action
        public var title: String
        public var enabled: Bool
        public var reasonCode: String?
        public var message: String?
        public var precision: Precision?
        public var destination: Destination
        /// For `confirm` and `take_over`: what to ask first.
        public var question: String?
        public var canResumeDefault: Bool
        public var pendingLaunch: Bool
        public var choices: [Choice]
        public var elsewhere: [String]
        /// The fixed words; `.fallback` when an older helper sent none.
        public var strings: Strings

        public init(state: State = .stopped, action: Action, title: String, enabled: Bool = true,
                    reasonCode: String? = nil, message: String? = nil, precision: Precision? = nil,
                    destination: Destination = .none, question: String? = nil,
                    canResumeDefault: Bool = false, pendingLaunch: Bool = false, choices: [Choice] = [],
                    elsewhere: [String] = [], strings: Strings = .fallback) {
            self.state = state
            self.action = action
            self.title = title
            self.enabled = enabled
            self.reasonCode = reasonCode
            self.message = message
            self.precision = precision
            self.destination = destination
            self.question = question
            self.canResumeDefault = canResumeDefault
            self.pendingLaunch = pendingLaunch
            self.choices = choices
            self.elsewhere = elsewhere
            self.strings = strings
        }

        private enum CodingKeys: String, CodingKey {
            case state, action, title, enabled, message, precision, choices, elsewhere, destination, question, strings
            case reasonCode = "reason_code", canResumeDefault = "can_resume_default", pendingLaunch = "pending_launch"
        }

        public init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            state = try c.decodeIfPresent(State.self, forKey: .state) ?? .unknown("")
            action = try c.decodeIfPresent(Action.self, forKey: .action) ?? .unknown("")
            title = try c.decodeIfPresent(String.self, forKey: .title) ?? ""
            enabled = try c.decodeIfPresent(Bool.self, forKey: .enabled) ?? false
            reasonCode = try c.decodeIfPresent(String.self, forKey: .reasonCode)
            message = try c.decodeIfPresent(String.self, forKey: .message)
            precision = try c.decodeIfPresent(Precision.self, forKey: .precision)
            destination = try c.decodeIfPresent(Destination.self, forKey: .destination) ?? .none
            question = try c.decodeIfPresent(String.self, forKey: .question)
            canResumeDefault = try c.decodeIfPresent(Bool.self, forKey: .canResumeDefault) ?? false
            pendingLaunch = try c.decodeIfPresent(Bool.self, forKey: .pendingLaunch) ?? false
            choices = try c.decodeIfPresent([Choice].self, forKey: .choices) ?? []
            elsewhere = try c.decodeIfPresent([String].self, forKey: .elsewhere) ?? []
            strings = (try? c.decodeIfPresent(Strings.self, forKey: .strings)) ?? .fallback
        }

        /// Whether the button can be pressed: an action this build knows
        /// that the helper enabled. An action a newer helper names is shown
        /// with its words but never sent.
        public var actionable: Bool {
            guard enabled else { return false }
            if case .unknown = action { return false }
            return action != .unavailable
        }
    }

    /// What one click did.
    public struct OpenResult: Equatable, Sendable, Decodable {
        public var outcome: Outcome
        public var precision: Precision?
        public var reasonCode: String?
        public var message: String
        public var plan: Plan?

        public init(outcome: Outcome, precision: Precision? = nil, reasonCode: String? = nil, message: String, plan: Plan?) {
            self.outcome = outcome
            self.precision = precision
            self.reasonCode = reasonCode
            self.message = message
            self.plan = plan
        }

        private enum CodingKeys: String, CodingKey {
            case outcome, precision, message, plan
            case reasonCode = "reason_code"
        }

        public init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            outcome = try c.decodeIfPresent(Outcome.self, forKey: .outcome) ?? .unknown("")
            precision = try c.decodeIfPresent(Precision.self, forKey: .precision)
            reasonCode = try c.decodeIfPresent(String.self, forKey: .reasonCode)
            message = try c.decodeIfPresent(String.self, forKey: .message) ?? ""
            plan = try c.decodeIfPresent(Plan.self, forKey: .plan)
        }
    }
}

