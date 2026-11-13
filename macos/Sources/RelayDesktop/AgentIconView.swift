import AppKit
import RelayDesktopCore
import SwiftUI

/// Bundled agent artwork, shared by onboarding and the session monitor.
struct AgentIconView: View {
    var source: String
    var size: CGFloat = 36
    /// The app the session runs in, when not the provider's own
    /// (`conductor`): its icon is badged on the provider's.
    var host: String? = nil
    var artworkColor: Color? = nil
    var avatar: SessionAvatar? = nil

    static func name(for source: String) -> String {
        switch source.lowercased() {
        case "capy": return "Capy"
        case "dot": return "Dot"
        case "conductor": return "Conductor"
        case "paseo": return "Paseo"
        case "replicas": return "Replicas"
        case "claude-cloud": return "Claude cloud"
        case "codex": return "Codex"
        case "copilot": return "GitHub Copilot"
        case "claude": return "Claude"
        case "claude-code", "claude_code", "claudecode": return "Claude Code"
        case "grok", "xai": return "Grok"
        case "grok-bot": return "Grok Bot"
        case "cursor": return "Cursor"
        case "cursor-cloud", "cursor_cloud": return "Cursor Cloud"
        case "opencode", "opencode-open", "open-code", "open_code": return "OpenCode"
        case "openai", "open-ai", "open_ai": return "OpenAI"
        case "muse", "muse-open": return "Muse Code"
        case "muse-assistant": return "Muse"
        case "devin": return "Devin"
        case "openclaw": return "OpenClaw"
        case "hermes": return "Hermes"
        case "droid", "factory": return "Factory Droid"
        case "pi": return "Pi"
        case "omp": return "Oh My Pi"
        case "amp": return "Amp"
        default: return source.capitalized
        }
    }

    /// The solid tile a mark sits on in session lists: the provider's
    /// primary colour behind its white mark. Providers whose artwork
    /// already carries its own tile (Codex, Grok Bot) and the Claude Code
    /// crab, whose orange is the mark itself, have none.
    enum BrandShape: Equatable {
        case circle
        case roundedSquare
    }

    struct Brand: Equatable {
        var red: Double
        var green: Double
        var blue: Double
        var shape: BrandShape

        var background: Color { Color(red: red, green: green, blue: blue) }
        var foreground: Color { .white }
    }

    static func brand(for source: String) -> Brand? {
        switch source.lowercased() {
        case "muse", "muse-open":
            return Brand(red: 0.25, green: 0.57, blue: 1.0, shape: .circle)
        case "grok", "xai":
            return Brand(red: 0, green: 0, blue: 0, shape: .circle)
        case "claude":
            return Brand(red: 217.0 / 255, green: 119.0 / 255, blue: 87.0 / 255, shape: .roundedSquare)
        default:
            return nil
        }
    }

    private static let images: [String: NSImage] = {
        let resources = Bundle.main.url(forResource: "RelayDesktop_RelayDesktop", withExtension: "bundle")
            .flatMap { Bundle(url: $0) } ?? .module
        var images: [String: NSImage] = [:]
        // amp.svg: Amp's official logo (ampcode.com/logo-light.svg, from its press
        // kit), square-framed in currentColor; see assets/agent-icons-LICENSE.txt.
        let templateImages = Set(["copilot", "devin", "grok", "cursor", "opencode", "openai", "muse", "pi", "replicas", "amp"])
        for name in ["copilot", "capy", "codex", "claude", "devin", "grok", "cursor", "opencode", "openai", "claudecode", "muse", "openclaw", "hermes", "pi", "replicas", "amp"] {
            if let url = resources.url(forResource: name, withExtension: "svg"),
               let image = NSImage(contentsOf: url) {
                image.isTemplate = templateImages.contains(name)
                images[name] = image
            }
        }
        if let url = resources.url(forResource: "grokbot", withExtension: "png"),
           let image = NSImage(contentsOf: url) {
            images["grokbot"] = image
        }
        for name in ["conductor", "paseo"] {
            if let url = resources.url(forResource: name, withExtension: "png"),
               let image = NSImage(contentsOf: url) {
                images["host:\(name)"] = image
                images[name] = image
            }
        }
        return images
    }()

    static func artwork(for source: String) -> NSImage? {
        let named = name(for: source)
        let key: String
        switch named {
        case "GitHub Copilot": key = "copilot"
        case "Cursor Cloud": key = "cursor"
        case "Claude cloud": key = "claude"
        case "Dot": key = "openai"
        case "Muse Code": key = "muse"
        default: key = named.lowercased().replacingOccurrences(of: " ", with: "")
        }
        return images[key]
    }

    var body: some View {
        Group {
            if let avatar, let view = SessionAvatarView(avatar: avatar) {
                view
            } else if let image = Self.artwork(for: source) {
                if let brand = Self.brand(for: source), artworkColor == nil {
                    ZStack {
                        Group {
                            if brand.shape == .circle {
                                Circle().fill(brand.background)
                                    .overlay {
                                        if brand.red == 0, brand.green == 0, brand.blue == 0 {
                                            Circle().strokeBorder(Color.primary.opacity(0.14), lineWidth: 1)
                                        }
                                    }
                            } else {
                                RoundedRectangle(cornerRadius: size * 0.24).fill(brand.background)
                            }
                        }
                        .zoomFrame(width: size, height: size)
                        Image(nsImage: image).renderingMode(.template).resizable().scaledToFit()
                            .foregroundStyle(brand.foreground)
                            .zoomFrame(width: size * 0.64, height: size * 0.64)
                    }
                } else if let artworkColor {
                    Image(nsImage: image).resizable().scaledToFit()
                        .foregroundStyle(artworkColor)
                } else {
                    Image(nsImage: image).resizable().scaledToFit()
                }
            } else {
                Image(systemName: "terminal").resizable().scaledToFit().zoomPadding(4)
                    .foregroundStyle(.secondary)
            }
        }
        .zoomFrame(width: size, height: size)
        // Top-trailing: the agent list puts its presence dot bottom-trailing.
        .overlay(alignment: .topTrailing) {
            if SessionHost.name(host) != nil, let badge = host.flatMap({ Self.images["host:\($0.lowercased())"] }) {
                let side = max(9, (size * 0.55).rounded())
                Image(nsImage: badge).resizable().scaledToFit()
                    .zoomFrame(width: side, height: side)
                    .clipShape(RoundedRectangle(cornerRadius: side * 0.22))
                    .offset(x: side * 0.3, y: -side * 0.3)
            }
        }
        .accessibilityLabel(SessionHost.label(provider: Self.name(for: source), host: host))
        .help(SessionHost.label(provider: Self.name(for: source), host: host))
    }
}
