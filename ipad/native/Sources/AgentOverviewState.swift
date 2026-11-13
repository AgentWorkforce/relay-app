import Foundation

/// What the Agent overview says when it has no generated brief to show.
///
/// A relay registration publishes its native session ID (`metadata.session_id`),
/// the same ID Cloud History files the session under. Cloud only knows the
/// session once the agent's machine has shared it with History, so a 404 for
/// the session means "not in History", not a fault: the overview says so and
/// offers no Generate, which would be refused the same way.
enum AgentOverviewState {
    /// Whether a Cloud failure for a session's analysis means Cloud History
    /// has no copy of the session. Other 404 codes (a missing workspace, say)
    /// stay errors.
    static func isMissingFromHistory(status: Int?, code: String) -> Bool {
        status == 404 && ["", "not_found", "session_not_found"].contains(code.lowercased())
    }

    static let missingFromHistory =
        "This agent's session isn't in Cloud History, so it can't be summarized yet. " +
        "It can be once the agent's machine shares the session with History."

    /// The line under the objective and original request when no brief exists.
    static func hint(hasSessionID: Bool, hasObjective: Bool, canLoadSummaries: Bool,
                     loading: Bool, missingFromHistory: Bool) -> String {
        if !hasSessionID {
            return hasObjective ? "No Cloud session summary is linked to this chat."
                                : "This agent has not published a session summary."
        }
        guard canLoadSummaries else { return "Cloud access needs approval. Enable summaries once in Settings." }
        if missingFromHistory { return Self.missingFromHistory }
        if loading { return "Checking for a saved summary…" }
        return hasObjective ? "Generate a Cloud summary for more detail."
                            : "Generate a short summary to see what this agent is working on."
    }

    /// Whether the overview offers Generate or Regenerate.
    static func offersGenerate(hasSessionID: Bool, canLoadSummaries: Bool, missingFromHistory: Bool) -> Bool {
        hasSessionID && canLoadSummaries && !missingFromHistory
    }
}
