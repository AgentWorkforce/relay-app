import Foundation

/// Matches the GTK teammate native input setting word for word.
public enum TeammateNativeInputCopy {
    public static let title = "Native input from teammates"
    public static let detail = "Off: teammates' messages arrive as peer messages. On: a direct message from a teammate or their agent to a Claude Code session with Agent Relay's plugin starts a user turn, labelled with the sender. Channel posts and mentions stay peer messages, and tool approvals still go to you."
}
