import Foundation
import Darwin

/// Runs the bundled helper's `session-open` command as this app's own child,
/// so macOS attributes the Automation (Apple Events) permission the helper
/// uses to the app. Requests and answers are never logged: they name
/// sessions and folders.
public final class SessionOpenEngine: Sendable {
    public enum Verb: String, Sendable { case plan, open, release }

    /// What a helper run produced: its exit status and its stdout.
    public struct Output: Equatable, Sendable {
        public var status: Int32
        public var stdout: Data
        public init(status: Int32, stdout: Data) {
            self.status = status
            self.stdout = stdout
        }
    }

    /// Runs the helper with `arguments`, `input` on stdin; throws
    /// `Failure.timedOut` after `timeout` seconds.
    public typealias Runner = @Sendable (_ arguments: [String], _ input: Data, _ timeout: TimeInterval) async throws -> Output

    /// No answer could be had. Every sentence is one of the helper's fixed
    /// words (`SessionOpen.Strings`), never this app's own.
    public enum Failure: Error, Equatable, LocalizedError {
        /// This copy of the app has no helper (`swift run`, a broken bundle).
        case unavailable
        case timedOut(Verb)
        /// Exit 2: the helper refused the command or request.
        case rejected
        /// Exit 1 or another status: no answer was written.
        case noAnswer(Verb, Int32)
        case malformedAnswer(Verb)
        /// The helper could not be started.
        case launchFailed(Verb)
        case alreadyOpening

        /// The sentence for this failure in `strings`: after an `open`
        /// that may have run, that its outcome is unknown.
        public func sentence(_ strings: SessionOpen.Strings) -> String {
            switch self {
            case .unavailable: return strings.helperMissing
            case .rejected: return strings.rejected
            case .alreadyOpening: return strings.alreadyOpening
            case .launchFailed(.open): return strings.openNotStarted
            case .timedOut(.open), .noAnswer(.open, _), .malformedAnswer(.open): return strings.openUnknown
            case .timedOut, .noAnswer, .malformedAnswer, .launchFailed: return strings.planFailed
            }
        }

        public var errorDescription: String? { sentence(.fallback) }
    }

    /// `plan` (and `release`) is one passive look; a few seconds is generous.
    public static let planTimeout: TimeInterval = 8
    /// `open` covers a launch and its verification (15 s and more).
    public static let openTimeout: TimeInterval = 60

    private let runner: Runner?
    private let planTimeout: TimeInterval
    private let openTimeout: TimeInterval
    private let inFlight = Locked<Set<String>>([])

    /// `runner` nil: no helper, every call fails `.unavailable`.
    public init(runner: Runner?, planTimeout: TimeInterval = SessionOpenEngine.planTimeout,
                openTimeout: TimeInterval = SessionOpenEngine.openTimeout) {
        self.runner = runner
        self.planTimeout = planTimeout
        self.openTimeout = openTimeout
    }

    /// The engine for the helper at `executable`, or an unavailable one.
    public convenience init(executable: URL?) {
        self.init(runner: executable.map { Self.processRunner(executable: $0) })
    }

    public var isAvailable: Bool { runner != nil }

    public func plan(_ request: SessionOpen.Request) async throws -> SessionOpen.Plan {
        try await run(.plan, request, timeout: planTimeout)
    }

    /// One click. A second open of the same session while one runs is
    /// refused here, before the helper (which refuses it too) is started.
    public func open(_ request: SessionOpen.Request) async throws -> SessionOpen.OpenResult {
        let key = request.source + ":" + request.sessionId
        guard inFlight.withLock({ $0.insert(key).inserted }) else { throw Failure.alreadyOpening }
        defer { inFlight.withLock { _ = $0.remove(key) } }
        return try await run(.open, request, timeout: openTimeout)
    }

    /// The person said the earlier launch was closed: the helper releases
    /// the session's launch reservation and answers a fresh plan. Nothing
    /// is opened. Shares `open`'s one-at-a-time guard (here and in the
    /// helper), so a release never runs beside an open of the session.
    public func release(_ request: SessionOpen.Request) async throws -> SessionOpen.Plan {
        let key = request.source + ":" + request.sessionId
        guard inFlight.withLock({ $0.insert(key).inserted }) else { throw Failure.alreadyOpening }
        defer { inFlight.withLock { _ = $0.remove(key) } }
        return try await run(.release, request, timeout: planTimeout)
    }

    public func isOpening(source: String, sessionId: String) -> Bool {
        inFlight.withLock { $0.contains(source + ":" + sessionId) }
    }

    private func run<Answer: Decodable>(_ verb: Verb, _ request: SessionOpen.Request,
                                        timeout: TimeInterval) async throws -> Answer {
        guard let runner else { throw Failure.unavailable }
        let input = try JSONEncoder().encode(request)
        let output = try await runner(["session-open", verb.rawValue], input, timeout)
        switch output.status {
        case 0:
            do { return try JSONDecoder().decode(Answer.self, from: output.stdout) }
            catch { throw Failure.malformedAnswer(verb) }
        case 2: throw Failure.rejected
        default: throw Failure.noAnswer(verb, output.status)
        }
    }

    /// Largest answer read; an answer is a few kilobytes.
    static let maxAnswer = 1 << 20

    /// Runs `executable` off the caller's thread. stderr is discarded (it is
    /// one diagnostic line); stdout is read until the helper exits, plus a
    /// short grace for a child that inherited it.
    public static func processRunner(executable: URL) -> Runner {
        { arguments, input, timeout in
            try await withCheckedThrowingContinuation { continuation in
                DispatchQueue.global(qos: .userInitiated).async {
                    continuation.resume(with: Result {
                        try runProcess(executable: executable, arguments: arguments, input: input, timeout: timeout)
                    })
                }
            }
        }
    }

    static func runProcess(executable: URL, arguments: [String], input: Data,
                           timeout: TimeInterval) throws -> Output {
        let verb = arguments.last.flatMap(Verb.init(rawValue:)) ?? .plan
        let process = Process()
        process.executableURL = executable
        process.arguments = arguments
        let stdin = Pipe(), stdout = Pipe()
        process.standardInput = stdin
        process.standardOutput = stdout
        process.standardError = FileHandle.nullDevice
        let exited = DispatchSemaphore(value: 0)
        process.terminationHandler = { _ in exited.signal() }
        let collected = Collector(limit: maxAnswer)
        stdout.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            if data.isEmpty {
                handle.readabilityHandler = nil
                collected.finish()
            } else {
                collected.append(data)
            }
        }
        do { try process.run() } catch {
            stdout.fileHandleForReading.readabilityHandler = nil
            throw Failure.launchFailed(verb)
        }
        // The helper may exit before reading: never die of SIGPIPE.
        let writer = stdin.fileHandleForWriting
        _ = fcntl(writer.fileDescriptor, F_SETNOSIGPIPE, 1)
        DispatchQueue.global(qos: .userInitiated).async {
            try? writer.write(contentsOf: input)
            try? writer.close()
        }
        if exited.wait(timeout: .now() + timeout) == .timedOut {
            process.terminate()
            if exited.wait(timeout: .now() + 2) == .timedOut {
                kill(process.processIdentifier, SIGKILL)
                exited.wait()
            }
            stdout.fileHandleForReading.readabilityHandler = nil
            throw Failure.timedOut(verb)
        }
        collected.waitForEnd(timeout: 2)
        stdout.fileHandleForReading.readabilityHandler = nil
        guard process.terminationReason == .exit else { return Output(status: -1, stdout: Data()) }
        guard !collected.overflowed else { throw Failure.malformedAnswer(verb) }
        return Output(status: process.terminationStatus, stdout: collected.data)
    }
}

/// A value behind a lock.
final class Locked<Value>: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Value
    init(_ value: Value) { self.value = value }
    func withLock<T>(_ body: (inout Value) throws -> T) rethrows -> T {
        lock.lock()
        defer { lock.unlock() }
        return try body(&value)
    }
}

/// stdout as it arrives, bounded.
private final class Collector: @unchecked Sendable {
    private let lock = NSLock()
    private let ended = DispatchSemaphore(value: 0)
    private let limit: Int
    private var buffer = Data()
    private var tooLarge = false
    private var finished = false

    init(limit: Int) { self.limit = limit }

    func append(_ data: Data) {
        lock.lock()
        defer { lock.unlock() }
        if buffer.count + data.count > limit { tooLarge = true } else { buffer.append(data) }
    }

    func finish() {
        lock.lock()
        let first = !finished
        finished = true
        lock.unlock()
        if first { ended.signal() }
    }

    func waitForEnd(timeout: TimeInterval) { _ = ended.wait(timeout: .now() + timeout) }

    var data: Data {
        lock.lock()
        defer { lock.unlock() }
        return buffer
    }

    var overflowed: Bool {
        lock.lock()
        defer { lock.unlock() }
        return tooLarge
    }
}
