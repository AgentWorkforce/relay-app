import Foundation

/// Wording for teammate context: guidance Agent Relay adds to the start of
/// every session it reaches (Claude Code, Codex, Copilot, Devin) to check
/// teammates' session summaries first. Off by default; the first-run
/// provider chooser and Settings turn it on. The Linux app's
/// `TEAMMATE_CONTEXT_*` constants match these word for word.
public enum TeammateContextCopy {
    public static let title = "Have agents check what teammates are working on"
    public static let detail = "At the start of each Claude Code, Codex, Copilot or Devin session, Agent Relay tells the agent to read teammates' session summaries before starting work."
    public static let turnOnHelp = "Tell new agent sessions to check teammates' work first"
    public static let turnOffHelp = "Stop telling new agent sessions to check teammates' work"
    public static let failed = "Couldn’t change teammate context. Try again."
}
