import AVFoundation
import Speech

extension VoiceCandidate.Quality {
    init(_ quality: AVSpeechSynthesisVoiceQuality) {
        switch quality {
        case .premium: self = .premium
        case .enhanced: self = .enhanced
        default: self = .standard
        }
    }
}

/// The call's audio: one `AVAudioEngine` with voice processing (the echo
/// cancellation FaceTime uses) for the microphone, on-device speech
/// recognition, and speech synthesis rendered into the same engine so echo
/// cancellation removes the agent's voice from the microphone.
///
/// Control methods are called on the main thread; callbacks arrive there too.
final class VoiceCallAudio {
    enum Failure: LocalizedError {
        case recognizerUnavailable(String)

        var errorDescription: String? {
            switch self {
            case .recognizerUnavailable(let locale):
                return "On-device speech recognition isn't available for \(locale) on this device."
            }
        }
    }

    /// Input level in dBFS and the duration of the buffer it describes.
    var onLevel: ((Float, TimeInterval) -> Void)?
    var onPartial: ((String) -> Void)?
    /// The final transcript of an utterance (empty when nothing was understood).
    var onFinal: ((String) -> Void)?
    /// Called once everything queued by `speak` or `play` has played.
    var onSpeechFinished: (() -> Void)?

    /// False when voice processing could not start; the call then ignores the
    /// microphone while the agent speaks.
    private(set) var echoCancelling = false
    private(set) var isSpeaking = false

    private let engine = AVAudioEngine()
    private let player = AVAudioPlayerNode()
    private let synthesizer = AVSpeechSynthesizer()
    private let recognizer: SFSpeechRecognizer?
    private let playbackFormat = AVAudioFormat(standardFormatWithSampleRate: 24_000, channels: 1)!
    private let lock = NSLock()
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var task: SFSpeechRecognitionTask?
    private var preRoll: [AVAudioPCMBuffer] = []
    private var preRollFrames: AVAudioFrameCount = 0
    private var converters: [String: AVAudioConverter] = [:]
    private var speechGeneration = 0
    private var pendingUtterances = 0
    private var finalTimer: Timer?
    private var muted = false
    /// Identifies the latest `speak` batch, so an earlier batch's end marker
    /// cannot finish playback that is still going.
    private var playbackToken = 0
    private var latestPartial = ""

    init(locale: Locale = .current) {
        recognizer = SFSpeechRecognizer(locale: locale) ?? SFSpeechRecognizer()
    }

    static func requestSpeechAuthorization() async -> Bool {
        await withCheckedContinuation { continuation in
            SFSpeechRecognizer.requestAuthorization { continuation.resume(returning: $0 == .authorized) }
        }
    }

    func start() throws {
        guard let recognizer, recognizer.isAvailable, recognizer.supportsOnDeviceRecognition else {
            throw Failure.recognizerUnavailable(recognizer?.locale.identifier ?? Locale.current.identifier)
        }
        let input = engine.inputNode
        do {
            try input.setVoiceProcessingEnabled(true)
            echoCancelling = true
        } catch {
            echoCancelling = false
        }
        engine.attach(player)
        engine.connect(player, to: engine.mainMixerNode, format: playbackFormat)
        try startEngine()
    }

    /// Restarts after a configuration change (a new route such as AirPods),
    /// which stops the engine and may change the input format.
    func restart() throws {
        stopSpeaking()
        cancelUtterance()
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        lock.lock()
        preRoll = []
        preRollFrames = 0
        lock.unlock()
        try startEngine()
    }

    private func startEngine() throws {
        let input = engine.inputNode
        input.installTap(onBus: 0, bufferSize: 1024, format: input.outputFormat(forBus: 0)) { [weak self] buffer, _ in
            self?.capture(buffer)
        }
        engine.prepare()
        try engine.start()
        player.play()
    }

    func stop() {
        stopSpeaking()
        cancelUtterance()
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        if echoCancelling { try? engine.inputNode.setVoiceProcessingEnabled(false) }
    }

    /// Pauses the engine for an interruption (a phone call, Siri) and resumes it.
    func pause() {
        stopSpeaking()
        cancelUtterance()
        engine.pause()
    }

    func resume() throws {
        try engine.start()
        player.play()
    }

    // MARK: Recognition

    /// Starts recognizing an utterance, including the moment just before speech
    /// was detected so the first syllable is not lost.
    func beginUtterance() {
        guard let recognizer else { return }
        cancelUtterance()
        let request = SFSpeechAudioBufferRecognitionRequest()
        request.requiresOnDeviceRecognition = true
        request.shouldReportPartialResults = true
        request.addsPunctuation = true
        request.taskHint = .dictation
        latestPartial = ""
        lock.lock()
        self.request = request
        preRoll.forEach(request.append)
        preRoll = []
        preRollFrames = 0
        lock.unlock()
        task = recognizer.recognitionTask(with: request) { [weak self] result, error in
            let text = result?.bestTranscription.formattedString ?? ""
            let isFinal = result?.isFinal ?? false
            DispatchQueue.main.async {
                guard let self, self.request === request else { return }
                if !text.isEmpty { self.latestPartial = text }
                if isFinal || error != nil {
                    self.finish(request)
                } else if !text.isEmpty {
                    self.onPartial?(text)
                }
            }
        }
    }

    /// Ends the utterance and reports its transcript once recognition settles.
    func endUtterance() {
        guard let request else {
            // Nothing was being recognized (it was cancelled); still report an
            // empty result so the caller leaves its listening state.
            DispatchQueue.main.async { [weak self] in self?.onFinal?("") }
            return
        }
        request.endAudio()
        finalTimer?.invalidate()
        // A final result normally follows within a few hundred milliseconds;
        // fall back to the latest partial if it does not.
        finalTimer = Timer.scheduledTimer(withTimeInterval: 1.5, repeats: false) { [weak self] _ in
            self?.finish(request)
        }
    }

    func cancelUtterance() {
        finalTimer?.invalidate()
        finalTimer = nil
        task?.cancel()
        task = nil
        lock.lock()
        request = nil
        lock.unlock()
    }

    private func finish(_ finished: SFSpeechAudioBufferRecognitionRequest) {
        guard request === finished else { return }
        let text = latestPartial
        cancelUtterance()
        onFinal?(text)
    }

    /// While muted, microphone audio is neither recognized nor kept for pre-roll.
    func setMuted(_ muted: Bool) {
        lock.lock()
        self.muted = muted
        preRoll = []
        preRollFrames = 0
        lock.unlock()
    }

    /// Runs on the audio thread.
    private func capture(_ buffer: AVAudioPCMBuffer) {
        let duration = Double(buffer.frameLength) / buffer.format.sampleRate
        var level: Float = -160
        if let channel = buffer.floatChannelData?[0] {
            level = VoiceActivityDetector.level(of: UnsafeBufferPointer(start: channel, count: Int(buffer.frameLength)))
        }
        lock.lock()
        if muted {
            // Drop the audio.
        } else if let request {
            request.append(buffer)
        } else if let copy = buffer.copy() as? AVAudioPCMBuffer {
            // Keep about 300 ms before speech is detected.
            preRoll.append(copy)
            preRollFrames += copy.frameLength
            while let first = preRoll.first, preRollFrames - first.frameLength >= AVAudioFrameCount(0.3 * buffer.format.sampleRate) {
                preRollFrames -= first.frameLength
                preRoll.removeFirst()
            }
        }
        lock.unlock()
        DispatchQueue.main.async { [weak self] in self?.onLevel?(level, duration) }
    }

    // MARK: Playback

    /// The best installed voice for the device's language: premium, then
    /// enhanced, then the default compact voice. Read on each reply, not
    /// cached, so a voice downloaded in Settings is used on the next one.
    static var preferredVoice: AVSpeechSynthesisVoice? {
        let voices = AVSpeechSynthesisVoice.speechVoices()
        let candidates = voices.map { voice in
            VoiceCandidate(identifier: voice.identifier, language: voice.language,
                           quality: VoiceCandidate.Quality(voice.quality),
                           novelty: voice.voiceTraits.contains(.isNoveltyVoice)
                               || voice.voiceTraits.contains(.isPersonalVoice))
        }
        let language = AVSpeechSynthesisVoice.currentLanguageCode()
        guard let best = VoicePicker.best(candidates, for: language) else {
            return AVSpeechSynthesisVoice(language: language)
        }
        return AVSpeechSynthesisVoice(identifier: best.identifier)
    }

    /// Speaks the sentences in order, rendering each through the engine.
    /// `rate` and `pitch` are multipliers for the persona's speaking style.
    func speak(_ sentences: [String], voice: AVSpeechSynthesisVoice? = nil, rate: Float = 1, pitch: Float = 1) {
        guard !sentences.isEmpty else { return }
        isSpeaking = true
        playbackToken += 1
        lock.lock()
        let generation = speechGeneration
        lock.unlock()
        pendingUtterances += sentences.count
        let voice = voice ?? Self.preferredVoice
        for sentence in sentences {
            let utterance = AVSpeechUtterance(string: sentence)
            utterance.voice = voice
            utterance.rate = min(AVSpeechUtteranceMaximumSpeechRate,
                                 max(AVSpeechUtteranceMinimumSpeechRate, AVSpeechUtteranceDefaultSpeechRate * rate))
            utterance.pitchMultiplier = min(2, max(0.5, pitch))
            synthesizer.write(utterance) { [weak self] buffer in
                guard let self, let pcm = buffer as? AVAudioPCMBuffer, self.isCurrent(generation) else { return }
                if pcm.frameLength == 0 {
                    DispatchQueue.main.async { self.utteranceRendered(generation) }
                } else if let converted = self.convert(pcm) {
                    self.player.scheduleBuffer(converted)
                }
            }
        }
    }

    /// Plays a cue tone through the engine.
    func play(_ cue: VoiceCue) {
        let samples = cue.samples(sampleRate: playbackFormat.sampleRate)
        guard let buffer = AVAudioPCMBuffer(pcmFormat: playbackFormat, frameCapacity: AVAudioFrameCount(samples.count)),
              let channel = buffer.floatChannelData?[0] else { return }
        buffer.frameLength = AVAudioFrameCount(samples.count)
        samples.withUnsafeBufferPointer { channel.update(from: $0.baseAddress!, count: samples.count) }
        player.scheduleBuffer(buffer)
    }

    /// Stops the agent's voice immediately (barge-in or "stop").
    func stopSpeaking() {
        lock.lock()
        speechGeneration += 1
        lock.unlock()
        pendingUtterances = 0
        synthesizer.stopSpeaking(at: .immediate)
        player.stop()
        if engine.isRunning { player.play() }
        isSpeaking = false
    }

    private func isCurrent(_ generation: Int) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return generation == speechGeneration
    }

    private func utteranceRendered(_ generation: Int) {
        guard isCurrent(generation) else { return }
        pendingUtterances -= 1
        guard pendingUtterances <= 0 else { return }
        // Everything is scheduled; report when the last buffer has played.
        let token = playbackToken
        guard let marker = AVAudioPCMBuffer(pcmFormat: playbackFormat, frameCapacity: 1) else { return }
        marker.frameLength = 1
        player.scheduleBuffer(marker, completionCallbackType: .dataPlayedBack) { [weak self] _ in
            DispatchQueue.main.async {
                guard let self, self.isCurrent(generation), token == self.playbackToken, self.pendingUtterances <= 0 else { return }
                self.isSpeaking = false
                self.onSpeechFinished?()
            }
        }
    }

    /// Converts a synthesizer buffer to the player's format. Runs on the
    /// synthesizer's thread.
    private func convert(_ buffer: AVAudioPCMBuffer) -> AVAudioPCMBuffer? {
        if buffer.format == playbackFormat { return buffer }
        let key = "\(buffer.format)"
        lock.lock()
        let converter = converters[key] ?? AVAudioConverter(from: buffer.format, to: playbackFormat)
        converters[key] = converter
        lock.unlock()
        guard let converter else { return nil }
        let ratio = playbackFormat.sampleRate / buffer.format.sampleRate
        let capacity = AVAudioFrameCount(Double(buffer.frameLength) * ratio) + 1024
        guard let output = AVAudioPCMBuffer(pcmFormat: playbackFormat, frameCapacity: capacity) else { return nil }
        var supplied = false
        var error: NSError?
        converter.convert(to: output, error: &error) { _, status in
            if supplied {
                status.pointee = .noDataNow
                return nil
            }
            supplied = true
            status.pointee = .haveData
            return buffer
        }
        return error == nil && output.frameLength > 0 ? output : nil
    }
}
