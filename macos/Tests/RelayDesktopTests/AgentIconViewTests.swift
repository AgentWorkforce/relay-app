import XCTest
import AppKit
import QuartzCore
import RelayDesktopCore
import SwiftUI
@testable import RelayDesktop

final class AgentIconViewTests: XCTestCase {
    func testNativeProfileAvatarsIncludeAnimationAndKeepUnknownArtworkOnFallback() throws {
        for shape in ["house", "blob"] {
            let frames = try XCTUnwrap(SessionAvatarView.frames(shape: shape))
            XCTAssertEqual(frames.count, 32)
            XCTAssertTrue(frames.allSatisfy(\.isTemplate))
            XCTAssertNotEqual(frames[0].tiffRepresentation, frames[12].tiffRepresentation)
        }
        XCTAssertNil(SessionAvatarView(avatar: SessionAvatar(color: "blue", shape: "unknown")))
        XCTAssertNil(SessionAvatarView.inlineImage("https://example.com/avatar.gif"))
    }

    @MainActor
    func testInlineAvatarRendersItsAlreadyResolvedImage() throws {
        let artwork = try XCTUnwrap(AgentIconView.artwork(for: "grok-bot"))
        let bitmap = try XCTUnwrap(NSBitmapImageRep(data: XCTUnwrap(artwork.tiffRepresentation)))
        let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
        let avatar = SessionAvatar(imageDataUrl: "data:image/png;base64," + png.base64EncodedString())
        let view = try XCTUnwrap(SessionAvatarView(avatar: avatar))
        let resolved = try XCTUnwrap(view.image)
        let host = NSHostingView(rootView: view)
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 24, height: 24),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        defer { window.close() }
        host.layoutSubtreeIfNeeded()
        func raster(in parent: NSView) -> NSImageView? {
            if let image = parent as? NSImageView { return image }
            return parent.subviews.lazy.compactMap { raster(in: $0) }.first
        }
        XCTAssertTrue(try XCTUnwrap(raster(in: host)).image === resolved)
        host.rootView = view
        host.layoutSubtreeIfNeeded()
        XCTAssertTrue(try XCTUnwrap(raster(in: host)).image === resolved)
    }

    @MainActor
    func testAnimatedAvatarPlaysItsFramesInCoreAnimation() throws {
        let frames = try XCTUnwrap(SessionAvatarView.frames(shape: "blob"))
        let view = AvatarFramesView(frame: NSRect(x: 0, y: 0, width: 24, height: 24))
        view.configure(frames: frames, color: .systemBlue, animates: true)
        XCTAssertTrue(view.layer?.mask === view.frameMask)
        let animation = try XCTUnwrap(view.frameMask.animation(forKey: AvatarFramesView.animationKey) as? CAKeyframeAnimation)
        XCTAssertEqual(animation.keyPath, "contents")
        XCTAssertEqual(animation.values?.count, 32)
        XCTAssertEqual(animation.duration, 4, accuracy: 0.001)
        XCTAssertEqual(animation.calculationMode, .discrete)
        XCTAssertEqual(animation.repeatCount, .infinity)
        XCTAssertFalse(animation.isRemovedOnCompletion)
        XCTAssertNotNil(view.layer?.backgroundColor)

        // Unchanged frames keep the running animation rather than restarting it.
        view.configure(frames: frames, color: .systemBlue, animates: true)
        XCTAssertTrue(view.frameMask.animation(forKey: AvatarFramesView.animationKey) === animation)
    }

    @MainActor
    func testReduceMotionShowsTheFirstAvatarFrameStill() throws {
        let frames = try XCTUnwrap(SessionAvatarView.frames(shape: "house"))
        let view = AvatarFramesView(frame: NSRect(x: 0, y: 0, width: 24, height: 24))
        view.configure(frames: frames, color: .systemBlue, animates: false)
        XCTAssertNil(view.frameMask.animation(forKey: AvatarFramesView.animationKey))
        XCTAssertTrue((view.frameMask.contents as AnyObject?) === frames[0].cgImage(forProposedRect: nil, context: nil, hints: nil))

        view.configure(frames: frames, color: .systemBlue, animates: true)
        XCTAssertNotNil(view.frameMask.animation(forKey: AvatarFramesView.animationKey))
        view.configure(frames: frames, color: .systemBlue, animates: false)
        XCTAssertNil(view.frameMask.animation(forKey: AvatarFramesView.animationKey))
    }

    @MainActor
    func testAnimatedAvatarRendersAsALayerBackedView() throws {
        let view = try XCTUnwrap(SessionAvatarView(avatar: SessionAvatar(color: "green", shape: "blob")))
        let host = NSHostingView(rootView: view.frame(width: 24, height: 24))
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 24, height: 24),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        defer { window.close() }
        host.layoutSubtreeIfNeeded()
        func frames(in parent: NSView) -> AvatarFramesView? {
            if let view = parent as? AvatarFramesView { return view }
            return parent.subviews.lazy.compactMap { frames(in: $0) }.first
        }
        let avatar = try XCTUnwrap(frames(in: host))
        XCTAssertEqual(avatar.frameMask.frame, avatar.bounds)
        let tint = try XCTUnwrap(avatar.layer?.backgroundColor.flatMap { NSColor(cgColor: $0)?.usingColorSpace(.sRGB) })
        XCTAssertEqual(tint.greenComponent, 0xC9 / 255, accuracy: 0.01)
    }

    func testGrokBotUsesItsOwnAppArtwork() throws {
        XCTAssertEqual(AgentIconView.name(for: "grok-bot"), "Grok Bot")
        XCTAssertEqual(AgentIconView.name(for: "Grok-Bot"), "Grok Bot")
        let artwork = try XCTUnwrap(AgentIconView.artwork(for: "grok-bot"))
        XCTAssertFalse(artwork.isTemplate)
        XCTAssertGreaterThan(artwork.size.width, 0)
        XCTAssertFalse(artwork === AgentIconView.artwork(for: "grok"))
    }

    func testFactoryDroidIsNamedForBothSourceIds() {
        XCTAssertEqual(AgentIconView.name(for: "droid"), "Factory Droid")
        XCTAssertEqual(AgentIconView.name(for: "factory"), "Factory Droid")
    }

    /// Dot and the Muse assistant draw their bundled marks, not the
    /// terminal fallback: builds before 10060af0 and 5f53a408 (stable
    /// v2026.11.2 and v2026.11.3) had no mapping for either core id.
    func testDotAndMuseAssistantResolveToTheirBundledMarks() throws {
        XCTAssertEqual(AgentIconView.name(for: "dot"), "Dot")
        XCTAssertEqual(AgentIconView.name(for: "muse-assistant"), "Muse")
        let dot = try XCTUnwrap(AgentIconView.artwork(for: "dot"))
        XCTAssertTrue(dot === AgentIconView.artwork(for: "openai"))
        let muse = try XCTUnwrap(AgentIconView.artwork(for: "muse-assistant"))
        XCTAssertTrue(muse === AgentIconView.artwork(for: "muse"))
        XCTAssertGreaterThan(muse.size.width, 0)
        XCTAssertNotNil(AgentIconView.artwork(for: "copilot"))
    }

    /// Amp draws its bundled logo as a template mark, not the terminal fallback.
    func testAmpResolvesToItsBundledMark() throws {
        XCTAssertEqual(AgentIconView.name(for: "amp"), "Amp")
        let amp = try XCTUnwrap(AgentIconView.artwork(for: "amp"))
        XCTAssertTrue(amp.isTemplate)
        XCTAssertGreaterThan(amp.size.width, 0)
    }

    func testACapyThreadIsNamedCapy() {
        XCTAssertEqual(AgentIconView.name(for: "capy"), "Capy")
        XCTAssertEqual(AgentIconView.name(for: "Capy"), "Capy")
        XCTAssertEqual(AgentIconView.name(for: "cursor-cloud"), "Cursor Cloud")
    }

    func testBrandedTilesUseTheProvidersPrimaryColour() {
        XCTAssertEqual(AgentIconView.brand(for: "muse"),
                       AgentIconView.Brand(red: 0.25, green: 0.57, blue: 1.0, shape: .circle))
        XCTAssertEqual(AgentIconView.brand(for: "muse-open"), AgentIconView.brand(for: "muse"))
        XCTAssertEqual(AgentIconView.brand(for: "grok"),
                       AgentIconView.Brand(red: 0, green: 0, blue: 0, shape: .circle))
        XCTAssertEqual(AgentIconView.brand(for: "xai"), AgentIconView.brand(for: "grok"))
        XCTAssertEqual(AgentIconView.brand(for: "claude"),
                       AgentIconView.Brand(red: 217.0 / 255, green: 119.0 / 255, blue: 87.0 / 255, shape: .roundedSquare))
    }

    func testUnbrandedArtworkKeepsItsOwnTile() {
        XCTAssertNil(AgentIconView.brand(for: "codex"))
        XCTAssertNil(AgentIconView.brand(for: "grok-bot"))
        XCTAssertNil(AgentIconView.brand(for: "claude-code"))
        XCTAssertNil(AgentIconView.brand(for: "cursor"))
    }

    func testClaudeCodeInTheTerminalUsesTheCrabNotTheClaudeMark() throws {
        XCTAssertEqual(AgentIconView.name(for: "claude-code"), "Claude Code")
        let crab = try XCTUnwrap(AgentIconView.artwork(for: "claude-code"))
        let claude = try XCTUnwrap(AgentIconView.artwork(for: "claude"))
        XCTAssertFalse(crab === claude)
        XCTAssertFalse(crab.isTemplate)
    }

    func testPiUsesItsOwnMark() throws {
        XCTAssertEqual(AgentIconView.name(for: "pi"), "Pi")
        let artwork = try XCTUnwrap(AgentIconView.artwork(for: "pi"))
        XCTAssertTrue(artwork.isTemplate)
        XCTAssertFalse(artwork === AgentIconView.artwork(for: "codex"))
    }

    func testReplicasUsesItsOfficialTemplateMark() throws {
        let artwork = try XCTUnwrap(AgentIconView.artwork(for: "replicas"))
        XCTAssertTrue(artwork.isTemplate)
        XCTAssertGreaterThan(artwork.size.width, 0)
    }
}
