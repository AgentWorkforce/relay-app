import Foundation

/// Wording for the teammate-context hook: a Claude Code SessionStart hook
/// that tells each session to check teammates' session summaries before it
/// starts. On by default; the first-run provider chooser and Settings turn
/// it off. The Linux app's `TEAMMATE_CONTEXT_*` constants match these word
/// for word.
public enum TeammateContextCopy {
    public static let title = "Have agents check what teammates are working on"
    public static let detail = "Adds a session-start hook so Claude Code reads teammates' session summaries before starting work."
    public static let turnOnHelp = "Add the teammate-context hook to Claude Code"
    public static let turnOffHelp = "Remove the teammate-context hook from Claude Code"
    public static let failed = "Couldn’t change the teammate-context hook. Try again."
}
