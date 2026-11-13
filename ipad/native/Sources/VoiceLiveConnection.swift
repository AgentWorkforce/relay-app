import AVFoundation
import Foundation
import WebRTC

/// The WebRTC connection to a GPT-Live session: the microphone and the
/// assistant's voice on media tracks, and JSON events on the `oai-events` data
/// channel. libwebrtc runs its own echo-cancelling audio unit.
///
/// Callbacks arrive on the main thread.
final class VoiceLiveConnection: NSObject {
    enum Failure: LocalizedError {
        case setup(String)
        var errorDescription: String? {
            switch self { case .setup(let detail): return detail }
        }
    }

    var onEvent: ((VoiceLiveEvent) -> Void)?
    /// The GPT-Live session id, for usage reports.
    var sessionID = ""
    /// The latest billed seconds GPT-Live reported for this session.
    var usageSeconds: Double = 0
    /// The connection failed or the event channel closed before `session.closed`.
    var onDisconnect: (() -> Void)?

    private static let factory: RTCPeerConnectionFactory = {
        RTCInitializeSSL()
        return RTCPeerConnectionFactory()
    }()

    private static var manualAudio = false
    private static var systemAudioActive = false

    /// CallKit activated the call's audio session.
    static func audioSessionActivated(_ session: AVAudioSession) {
        manualAudio = true
        systemAudioActive = true
        #if os(iOS)
        let rtc = RTCAudioSession.sharedInstance()
        rtc.audioSessionDidActivate(session)
        rtc.useManualAudio = true
        rtc.isAudioEnabled = true
        #endif
    }

    /// CallKit deactivated the call's audio session.
    static func audioSessionDeactivated() {
        systemAudioActive = false
        #if os(iOS)
        let rtc = RTCAudioSession.sharedInstance()
        rtc.isAudioEnabled = false
        rtc.audioSessionDidDeactivate(AVAudioSession.sharedInstance())
        #endif
    }

    /// CallKit was turned off during the call and the app activated the audio
    /// session itself: WebRTC runs its audio as it does without CallKit, for
    /// this call and the ones after it.
    static func audioSessionActivatedByApp(_ session: AVAudioSession) {
        manualAudio = false
        systemAudioActive = false
        #if os(iOS)
        let rtc = RTCAudioSession.sharedInstance()
        rtc.audioSessionDidActivate(session)
        rtc.isAudioEnabled = true
        rtc.useManualAudio = false
        #endif
    }

    private var peer: RTCPeerConnection?
    private var channel: RTCDataChannel?
    private var microphone: RTCAudioTrack?
    private var gathering: CheckedContinuation<Void, Never>?
    private var closing = false
    private var finished = false

    /// Builds the connection and returns its SDP offer, after ICE gathering
    /// completes (or a short timeout) so the offer carries its candidates.
    func makeOffer() async throws -> String {
        #if os(iOS)
        let audio = RTCAudioSessionConfiguration.webRTC()
        audio.categoryOptions = [.defaultToSpeaker, .allowBluetoothHFP, .allowBluetoothA2DP]
        RTCAudioSessionConfiguration.setWebRTC(audio)
        // With CallKit, the system activates the audio session and WebRTC
        // starts audio when told (see `audioSessionActivated`).
        let session = RTCAudioSession.sharedInstance()
        session.useManualAudio = Self.manualAudio
        if Self.manualAudio { session.isAudioEnabled = Self.systemAudioActive }
        #endif
        let configuration = RTCConfiguration()
        configuration.sdpSemantics = .unifiedPlan
        let constraints = RTCMediaConstraints(mandatoryConstraints: nil, optionalConstraints: nil)
        guard let peer = Self.factory.peerConnection(with: configuration, constraints: constraints, delegate: self) else {
            throw Failure.setup("Could not create a voice connection.")
        }
        self.peer = peer
        let track = Self.factory.audioTrack(with: Self.factory.audioSource(with: constraints), trackId: "microphone")
        microphone = track
        peer.add(track, streamIds: ["call"])
        // The event channel must exist before the offer is created.
        channel = peer.dataChannel(forLabel: "oai-events", configuration: RTCDataChannelConfiguration())
        channel?.delegate = self

        let offer: RTCSessionDescription = try await withCheckedThrowingContinuation { continuation in
            peer.offer(for: constraints) { description, error in
                if let description { continuation.resume(returning: description) }
                else { continuation.resume(throwing: error ?? Failure.setup("Could not create a voice offer.")) }
            }
        }
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            peer.setLocalDescription(offer) { error in
                if let error { continuation.resume(throwing: error) } else { continuation.resume() }
            }
        }
        if peer.iceGatheringState != .complete {
            await withCheckedContinuation { continuation in
                DispatchQueue.main.async {
                    self.gathering = continuation
                    DispatchQueue.main.asyncAfter(deadline: .now() + 2) { self.finishGathering() }
                }
            }
        }
        return peer.localDescription?.sdp ?? offer.sdp
    }

    func accept(answer: String) async throws {
        guard let peer else { throw Failure.setup("The voice connection closed.") }
        let description = RTCSessionDescription(type: .answer, sdp: answer)
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            peer.setRemoteDescription(description) { error in
                if let error { continuation.resume(throwing: error) } else { continuation.resume() }
            }
        }
    }

    @discardableResult
    func send(_ command: VoiceLiveCommand) -> Bool {
        guard let channel, channel.readyState == .open else { return false }
        return channel.sendData(RTCDataBuffer(data: command.data(), isBinary: false))
    }

    /// Muting stops sending microphone audio; GPT-Live still bills the session.
    func setMuted(_ muted: Bool) {
        microphone?.isEnabled = !muted
    }

    /// Asks GPT-Live to finish; the session ends with `session.closed`.
    func requestClose() {
        closing = true
        if !send(.close) { tearDown() }
    }

    /// Releases the connection and its audio.
    func tearDown() {
        closing = true
        finishGathering()
        channel?.delegate = nil
        channel?.close()
        channel = nil
        microphone = nil
        peer?.close()
        peer = nil
    }

    private func finishGathering() {
        let continuation = gathering
        gathering = nil
        continuation?.resume()
    }

    private func disconnected() {
        DispatchQueue.main.async {
            guard !self.closing, !self.finished else { return }
            self.finished = true
            self.onDisconnect?()
        }
    }
}

extension VoiceLiveConnection: RTCPeerConnectionDelegate {
    func peerConnection(_ peerConnection: RTCPeerConnection, didChange stateChanged: RTCSignalingState) {}
    func peerConnection(_ peerConnection: RTCPeerConnection, didAdd stream: RTCMediaStream) {}
    func peerConnection(_ peerConnection: RTCPeerConnection, didRemove stream: RTCMediaStream) {}
    func peerConnectionShouldNegotiate(_ peerConnection: RTCPeerConnection) {}
    func peerConnection(_ peerConnection: RTCPeerConnection, didGenerate candidate: RTCIceCandidate) {}
    func peerConnection(_ peerConnection: RTCPeerConnection, didRemove candidates: [RTCIceCandidate]) {}
    func peerConnection(_ peerConnection: RTCPeerConnection, didOpen dataChannel: RTCDataChannel) {}

    func peerConnection(_ peerConnection: RTCPeerConnection, didChange newState: RTCIceConnectionState) {
        if newState == .failed || newState == .closed { disconnected() }
    }

    func peerConnection(_ peerConnection: RTCPeerConnection, didChange newState: RTCIceGatheringState) {
        guard newState == .complete else { return }
        DispatchQueue.main.async { self.finishGathering() }
    }
}

extension VoiceLiveConnection: RTCDataChannelDelegate {
    func dataChannelDidChangeState(_ dataChannel: RTCDataChannel) {
        if dataChannel.readyState == .closed { disconnected() }
    }

    func dataChannel(_ dataChannel: RTCDataChannel, didReceiveMessageWith buffer: RTCDataBuffer) {
        guard !buffer.isBinary, let event = VoiceLiveEvent.parse(buffer.data) else { return }
        DispatchQueue.main.async {
            if case .closed = event { self.finished = true }
            self.onEvent?(event)
        }
    }
}
