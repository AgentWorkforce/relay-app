/// Asked on quitting while Keep agents connected after quitting is off: the
/// core stops with the app. The Linux and Windows app asks in the same words.
public enum QuitConfirmation {
    public static let title = "Quit and take your agents offline?"
    public static let detail = "Keep agents connected after quitting is off, so your agents stop receiving messages and uploads stop until you open Agent Relay again."
    public static let quitAnyway = "Quit Anyway"
    public static let keepAgentsConnected = "Keep Agents Connected"
}
