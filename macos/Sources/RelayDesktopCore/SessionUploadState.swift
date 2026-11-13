import Foundation

/// The uploader's fields a session's upload state reads. A status that
/// changes only its counters leaves every session's state as it was.
public struct UploaderGate: Equatable, Sendable {
    public let paused: Bool
    public let stage: Stage?
    public let stopped: Bool

    public init(status: UploaderStatus?) {
        paused = status?.paused == true
        stage = status?.stage
        stopped = status?.delivery.stopped == true
    }
}

/// A session row's upload state. Delivery is separate from permission to
/// upload: `shared` only proves permission.
public enum SessionUploadState: String, CaseIterable {
    case local, discovering, preparing, waiting, queued, uploading, uploaded, checking, paused, blocked, stopped, unknown

    public init(session: UploadSession, status: UploaderStatus?) {
        self.init(session: session, gate: UploaderGate(status: status))
    }

    public init(session: UploadSession, gate: UploaderGate) {
        if session.status == .uploaded { self = .uploaded; return }
        if session.status == .unknown { self = .unknown; return }
        if !session.included { self = .local; return }
        if gate.paused { self = .paused; return }
        // A problem that is not retried on its own stopped every upload.
        if gate.stage == .needsAttention && gate.stopped { self = .stopped; return }
        switch gate.stage {
        case .notRunning?, .needsAttention?, .offline?:
            self = .blocked; return
        default: break
        }
        switch session.status {
        case .uploaded: self = .uploaded
        case .uploading: self = .uploading
        case .queued: self = .queued
        case .waiting: self = .waiting
        case .shared: self = .preparing
        case .checking: self = .checking
        case .notShared, .unknown: self = .unknown
        }
    }

    public var title: String {
        switch self {
        case .discovering: return "Indexing"
        case .local: return "Not uploading"
        case .preparing: return "Preparing"
        case .waiting: return "Waiting to upload"
        case .queued: return "Queued"
        case .uploading: return "Uploading…"
        case .uploaded: return "Uploaded"
        case .checking: return "Checking…"
        case .paused: return "Paused"
        case .blocked: return "Waiting to retry"
        case .stopped: return "Upload stopped"
        case .unknown: return "Status unavailable"
        }
    }

    /// Short label for the icon. Pipeline milestones are not a measured percentage.
    public var iconTitle: String { self == .local ? "Local only" : title }

    public var symbol: String {
        switch self {
        case .discovering, .preparing: return "clock"
        case .waiting: return "hourglass"
        case .local: return "desktopcomputer"
        case .queued: return "tray.and.arrow.up"
        case .uploading: return "arrow.up.circle.fill"
        case .uploaded: return "checkmark.circle.fill"
        case .checking: return "arrow.triangle.2.circlepath"
        case .paused: return "pause.circle"
        case .blocked, .stopped: return "exclamationmark.circle"
        case .unknown: return "questionmark.circle"
        }
    }

    public var detail: String {
        switch self {
        case .discovering: return "Session discovered. Preparing it for upload selection."
        case .local: return "Not selected for upload. This does not remove any copy uploaded previously."
        case .preparing: return "Enabled for upload. Delivery has not been confirmed yet."
        case .waiting: return "Your organization uploads this session once it has been quiet for the upload window. Until then you can choose Don't upload."
        case .queued: return "Waiting for the uploader to send this session."
        case .uploading: return "The uploader is sending a batch containing this session."
        case .uploaded: return "The uploader reports this session is up to date."
        case .checking: return "Enabled for upload. Agent Relay is still confirming whether this session is up to date."
        case .paused: return "Enabled for upload. Resume uploads to continue."
        case .blocked: return "Enabled for upload, but the uploader is stopped or retrying a problem. See the upload status in the header."
        case .stopped: return "Enabled for upload, but a problem stopped uploads. Queued records are preserved; retry uploads from the upload status in the header."
        case .unknown: return "Enabled for upload, but delivery status is not available yet."
        }
    }

    /// Not yet confirmed is not waiting to upload.
    public var isPending: Bool { self != .local && self != .uploaded && self != .discovering && self != .checking }

    /// How far this session is through preparing → queued → uploading → uploaded.
    public var pipelineProgress: Double? {
        switch self {
        case .preparing: return 0.25
        case .queued: return 0.5
        case .uploading: return 0.75
        case .uploaded: return 1
        case .local, .discovering, .waiting, .checking, .paused, .blocked, .stopped, .unknown: return nil
        }
    }

    /// When a waiting session uploads: "Uploads in N min", rounded up and
    /// never under one minute.
    public static func uploadsIn(untilMs: Int64, nowMs: Int64) -> String {
        let minutes = (max(0, untilMs - nowMs) + 59_999) / 60_000
        return "Uploads in \(max(1, minutes)) min"
    }

    /// The row's short upload label: `uploadsIn` for a waiting session whose
    /// window end is known, else `iconTitle`.
    public func label(for session: UploadSession?, now: Date = Date()) -> String {
        if self == .waiting, let until = session?.uploadHoldUntilMs {
            return Self.uploadsIn(untilMs: until, nowMs: Int64(now.timeIntervalSince1970 * 1000))
        }
        return iconTitle
    }
}

public enum SessionUploadFilter: String, CaseIterable, Identifiable {
    case all = "All", pending = "In progress", uploaded = "Uploaded", local = "Not uploading"
    public var id: String { rawValue }

    public func matches(_ state: SessionUploadState) -> Bool {
        switch self {
        case .all: return true
        case .pending: return state.isPending
        case .uploaded: return state == .uploaded
        case .local: return state == .local
        }
    }
}

/// Upload status beside what the person just asked for. A request is not
/// permission confirmed by the uploader, nor proof of delivery.
public struct SessionUploadSummary {
    public let state: DisplayState
    public let headline: String
    public let detail: String
    public let selectionSummary: String

    public init(status: StatusViewModel, sessions: [UploadSession], requesting: Set<String>) {
        let enabled = max(status.sharedSessions, sessions.filter(\.included).count)
        selectionSummary = requesting.isEmpty
            ? "\(enabled.formatted()) \(enabled == 1 ? "session" : "sessions") enabled"
            : "\(requesting.count.formatted()) \(requesting.count == 1 ? "session" : "sessions") chosen"

        switch status.state {
        case .paused, .notRunning, .offline, .needsAttention, .conflict:
            state = status.state
            headline = status.headline
            detail = (status.detail ?? "") + (requesting.isEmpty ? "" : " Your upload selection is being prepared.")
            return
        default: break
        }
        if !requesting.isEmpty {
            state = .preparing
            headline = "Preparing \(requesting.count) \(requesting.count == 1 ? "session" : "sessions")"
            detail = "Uploading starts automatically after preparation. A large history can take a few minutes; you can leave this window open to follow progress."
            return
        }
        let pending = sessions.filter { $0.included && $0.status != .uploaded && $0.status != .checking }
        let checking = sessions.filter { $0.included && $0.status == .checking }
        if status.state == .upToDate && !pending.isEmpty {
            state = .preparing
            headline = "Waiting for upload confirmation"
            detail = "Your sessions are enabled for upload. Each row will show Uploaded when the uploader confirms it is up to date."
            return
        }
        if status.state == .upToDate && !checking.isEmpty {
            state = status.state
            headline = "Confirming upload status"
            detail = "The uploader is up to date. Agent Relay is still confirming \(checking.count.formatted()) \(checking.count == 1 ? "session" : "sessions"); each row updates when checked."
            return
        }
        state = status.state
        if status.state == .upToDate && enabled > 0 {
            headline = "Uploads up to date"
            detail = "New changes to enabled sessions upload automatically."
        } else {
            headline = status.headline
            detail = status.detail ?? "Sending session data. Each row will show Uploaded when its upload is confirmed."
        }
    }
}

/// A repeating activity sweep, never a claimed delivery percentage.
public struct UploadActivityFrame {
    public let fill: Double
    public let opacity: Double

    public init(elapsed: TimeInterval, reduceMotion: Bool = false) {
        if reduceMotion {
            fill = 1
            opacity = 0.85
        } else {
            let phase = max(0, elapsed).truncatingRemainder(dividingBy: 1.8)
            fill = min(1, phase / 1.5)
            opacity = 0.85 * (phase <= 1.5 ? 1 : (1.8 - phase) / 0.3)
        }
    }
}
