import Foundation
@testable import RelayDesktopCore

/// A helper that is never started: answers come from `respond`, and every
/// call is recorded (verb and the decoded request object).
final class FakeSessionHelper: @unchecked Sendable {
    struct Call {
        let arguments: [String]
        let request: [String: Any]
        var verb: String { arguments.last ?? "" }
    }

    private let lock = NSLock()
    private var recorded: [Call] = []
    var respond: @Sendable (Call) async throws -> SessionOpenEngine.Output

    init(respond: @escaping @Sendable (Call) async throws -> SessionOpenEngine.Output) {
        self.respond = respond
    }

    /// Always answers `plan` with `plan`, and `open` with `open` (or a
    /// verified result over the same plan).
    convenience init(plan: [String: Any], open: [String: Any]? = nil) {
        let planData = try! JSONSerialization.data(withJSONObject: plan)
        let openData = try! JSONSerialization.data(withJSONObject: open ?? [
            "outcome": "verified", "precision": "exact_conversation", "reason_code": NSNull(),
            "message": "Opened this session.", "plan": plan,
        ])
        self.init { call in
            SessionOpenEngine.Output(status: 0, stdout: call.verb == "open" ? openData : planData)
        }
    }

    var calls: [Call] { lock.withLock { recorded } }
    func calls(_ verb: String) -> [Call] { calls.filter { $0.verb == verb } }

    func engine(planTimeout: TimeInterval = 5, openTimeout: TimeInterval = 5) -> SessionOpenEngine {
        SessionOpenEngine(runner: { arguments, input, _ in
            let request = (try? JSONSerialization.jsonObject(with: input)) as? [String: Any] ?? [:]
            let call = Call(arguments: arguments, request: request)
            self.lock.withLock { self.recorded.append(call) }
            return try await self.respond(call)
        }, planTimeout: planTimeout, openTimeout: openTimeout)
    }

    static func plan(action: String, title: String, enabled: Bool = true, state: String = "stopped",
                     reasonCode: String? = nil, message: String? = nil, canResumeDefault: Bool = false,
                     pendingLaunch: Bool = false, choices: [[String: String]] = [], destination: String = "none",
                     question: String? = nil) -> [String: Any] {
        ["state": state, "action": action, "title": title, "enabled": enabled,
         "reason_code": reasonCode ?? NSNull(), "message": message ?? NSNull(), "precision": "right_window",
         "destination": destination, "question": question ?? NSNull(),
         "can_resume_default": canResumeDefault, "pending_launch": pendingLaunch, "choices": choices, "elsewhere": []]
    }
}
