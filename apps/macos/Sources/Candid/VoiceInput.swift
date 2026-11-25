import AIProviders
import AVFoundation
import CandidLocalization
import Foundation
import Speech

// Voice input for the floating ball: hold the ball (or the voice shortcut), speak, let go. The words land in the
// ball's input field for the user to edit; nothing is sent until they press Return. The microphone is open only
// while held, through its own audio engine, so an ongoing Mac recording keeps its own input. The clip lives in
// memory only and is dropped once it has been transcribed.

/// One of the three recognizers, in the order 「自动」 tries them.
enum VoiceEngineKind: String, CaseIterable, Codable, Sendable {
    /// Qwen3-ASR in the optional voice service on this Mac (`services/voice`).
    case local
    /// Any OpenAI-compatible `/v1/audio/transcriptions`, using an existing model source's address and key.
    case cloud
    /// macOS speech recognition: the last resort, with live words while speaking.
    case system

    var title: String {
        switch self { case .local: L("本机 Qwen3-ASR"); case .cloud: L("云端转写"); case .system: L("macOS 系统识别") }
    }
}

/// What the user picked in the ball's settings.
enum VoiceEngineChoice: String, CaseIterable, Sendable {
    case automatic, local, cloud, system
    var title: String {
        switch self { case .automatic: L("自动（用当前可用的最好的）"); case .local: VoiceEngineKind.local.title; case .cloud: VoiceEngineKind.cloud.title; case .system: VoiceEngineKind.system.title }
    }
}

/// Mono 16 kHz samples, -1…1.
struct VoiceClip: Sendable, Equatable {
    static let sampleRate = 16_000
    var samples: [Float]
    var seconds: Double { Double(samples.count) / Double(Self.sampleRate) }

    /// 16-bit PCM WAV, what every transcription endpoint accepts.
    var wav: Data {
        var data = Data()
        func u32(_ v: UInt32) { withUnsafeBytes(of: v.littleEndian) { data.append(contentsOf: $0) } }
        func u16(_ v: UInt16) { withUnsafeBytes(of: v.littleEndian) { data.append(contentsOf: $0) } }
        let bytes = UInt32(samples.count * 2)
        data.append(contentsOf: Array("RIFF".utf8)); u32(36 + bytes); data.append(contentsOf: Array("WAVE".utf8))
        data.append(contentsOf: Array("fmt ".utf8)); u32(16); u16(1); u16(1); u32(UInt32(Self.sampleRate)); u32(UInt32(Self.sampleRate * 2)); u16(2); u16(16)
        data.append(contentsOf: Array("data".utf8)); u32(bytes)
        var pcm = [Int16](repeating: 0, count: samples.count)
        for (i, s) in samples.enumerated() { pcm[i] = Int16(max(-1, min(1, s)) * Float(Int16.max)) }
        pcm.withUnsafeBytes { data.append(contentsOf: $0) }
        return data
    }
}

enum VoiceInputError: LocalizedError, Equatable {
    case unavailable(String)
    case microphonePermission
    case speechPermission
    case tooShort
    case nothingHeard
    case failed(String)

    var errorDescription: String? {
        switch self {
        case .unavailable(let why): why
        case .microphonePermission: L("需要麦克风授权：在系统设置的「隐私与安全性 › 麦克风」里允许 Candid")
        case .speechPermission: L("需要语音识别授权：在系统设置的「隐私与安全性 › 语音识别」里允许 Candid")
        case .tooShort: L("按住久一点再说")
        case .nothingHeard: L("没听清，再说一次")
        case .failed(let why): why
        }
    }
}

/// A recognizer. `available()` is cheap (a health check, a config read); `transcribe` gets the whole clip.
protocol SpeechEngine: AnyObject, Sendable {
    var kind: VoiceEngineKind { get }
    func available() async -> Bool
    func transcribe(_ clip: VoiceClip) async throws -> String
}

/// A recognizer that can also show words while the user is still speaking.
protocol LiveSpeechEngine: SpeechEngine {
    func startLive(onPartial: @escaping @Sendable (String) -> Void) throws -> LiveTranscription
}

/// Fed with microphone buffers while held; `finish()` returns the final words.
protocol LiveTranscription: AnyObject, Sendable {
    func append(_ buffer: AVAudioPCMBuffer)
    func finish() async throws -> String
    func cancel()
}

// MARK: Choosing an engine

enum VoiceEngines {
    /// The order to try: the user's choice first, then the rest from best to last resort.
    static func order(_ choice: VoiceEngineChoice) -> [VoiceEngineKind] {
        let all = VoiceEngineKind.allCases
        guard let first = VoiceEngineKind(rawValue: choice.rawValue) else { return all }
        return [first] + all.filter { $0 != first }
    }

    /// The first available engine in `order(choice)`.
    static func pick(_ choice: VoiceEngineChoice, from engines: [any SpeechEngine]) async -> (any SpeechEngine)? {
        for kind in order(choice) {
            if let engine = engines.first(where: { $0.kind == kind }), await engine.available() { return engine }
        }
        return nil
    }

    struct Outcome: Sendable, Equatable {
        var text: String
        var engine: VoiceEngineKind
        /// Engines tried before it and why they failed.
        var skipped: [VoiceEngineKind: String] = [:]
    }

    /// Transcribes with the first engine that is available and succeeds, falling back down the order on failure.
    /// `tried` skips engines already used for this clip (the live engine that just came back empty).
    static func transcribe(_ clip: VoiceClip, choice: VoiceEngineChoice, engines: [any SpeechEngine],
                           skipping tried: Set<VoiceEngineKind> = []) async throws -> Outcome {
        var skipped: [VoiceEngineKind: String] = [:]
        for kind in order(choice) where !tried.contains(kind) {
            guard let engine = engines.first(where: { $0.kind == kind }) else { continue }
            guard await engine.available() else { skipped[kind] = L("不可用"); continue }
            do {
                let text = try await engine.transcribe(clip).trimmingCharacters(in: .whitespacesAndNewlines)
                if text.isEmpty { throw VoiceInputError.nothingHeard }
                return Outcome(text: text, engine: kind, skipped: skipped)
            } catch VoiceInputError.nothingHeard {
                // Silence is not a broken engine: another one will not hear more.
                throw VoiceInputError.nothingHeard
            } catch {
                skipped[kind] = error.localizedDescription
            }
        }
        if let reason = skipped.values.first(where: { $0 != L("不可用") }) { throw VoiceInputError.failed(L("语音没识别出来：%@", reason)) }
        throw VoiceInputError.unavailable(L("没有可用的语音识别：安装本机语音组件、在悬浮球设置里选一个云端来源，或允许 macOS 语音识别"))
    }
}

// MARK: Local Qwen3-ASR

/// `services/voice` on 127.0.0.1:8767. Available when its health check says ready.
final class LocalVoiceEngine: SpeechEngine, @unchecked Sendable {
    let kind = VoiceEngineKind.local
    let base: URL
    private let session: URLSession
    init(base: URL = URL(string: "http://127.0.0.1:8767")!, session: URLSession = .shared) { self.base = base; self.session = session }

    func available() async -> Bool {
        var request = URLRequest(url: base.appendingPathComponent("health"))
        request.timeoutInterval = 1.5
        guard let (data, response) = try? await session.data(for: request), (response as? HTTPURLResponse)?.statusCode == 200,
              let body = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return false }
        return body["status"] as? String == "ready"
    }

    func transcribe(_ clip: VoiceClip) async throws -> String {
        // The service refuses a `model` other than its own, so none is sent.
        try await OpenAITranscription.send(clip, url: base.appendingPathComponent("v1/audio/transcriptions"), fields: [:], headers: [:], session: session)
    }
}

// MARK: Cloud (OpenAI-compatible)

/// Where the cloud engine sends audio: a model source the user already set up, read only.
struct CloudVoiceTarget: Sendable, Equatable {
    var url: URL
    var model: String
    var headers: [String: String]
    var secretRef: String?
    var destination: Destination

    /// `<base>/audio/transcriptions` of the chosen source, or nil when it is gone, disabled or not OpenAI-style.
    static func resolve(providerID: String, model: String, config: ProviderConfig) -> CloudVoiceTarget? {
        let model = model.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !providerID.isEmpty, !model.isEmpty, let p = config.provider(providerID), p.enabled,
              [.openaiChat, .openaiResponses, .other].contains(p.api) else { return nil }
        var base = p.baseURL.trimmingCharacters(in: .whitespacesAndNewlines)
        while base.hasSuffix("/") { base.removeLast() }
        guard let url = URL(string: base + "/audio/transcriptions"), url.scheme?.hasPrefix("http") == true else { return nil }
        return CloudVoiceTarget(url: url, model: model, headers: p.headers, secretRef: p.secretRef, destination: p.destination)
    }

    /// Sources the cloud engine can use: enabled, OpenAI-style, not the local voice service itself.
    static func candidates(_ config: ProviderConfig) -> [AIProvider] {
        config.providers.filter { $0.enabled && [.openaiChat, .openaiResponses, .other].contains($0.api) && $0.providerID != "candid-local-voice" }
    }
}

final class CloudVoiceEngine: SpeechEngine, @unchecked Sendable {
    let kind = VoiceEngineKind.cloud
    private let target: @Sendable () async -> CloudVoiceTarget?
    private let secret: @Sendable (String) throws -> String?
    private let session: URLSession
    init(target: @escaping @Sendable () async -> CloudVoiceTarget?, secret: @escaping @Sendable (String) throws -> String?, session: URLSession = .shared) {
        self.target = target; self.secret = secret; self.session = session
    }

    func available() async -> Bool {
        guard let target = await target() else { return false }
        guard let ref = target.secretRef else { return true }
        return ((try? secret(ref)) ?? nil)?.isEmpty == false
    }

    func transcribe(_ clip: VoiceClip) async throws -> String {
        guard let target = await target() else { throw VoiceInputError.unavailable(L("没有选云端来源")) }
        var headers = target.headers
        if let ref = target.secretRef {
            guard let key = try secret(ref), !key.isEmpty else { throw VoiceInputError.unavailable(L("云端来源没有密钥")) }
            headers["Authorization"] = "Bearer " + key
        }
        return try await OpenAITranscription.send(clip, url: target.url, fields: ["model": target.model], headers: headers, session: session)
    }
}

enum OpenAITranscription {
    static func body(_ clip: VoiceClip, fields: [String: String], boundary: String) -> Data {
        var body = Data()
        for (name, value) in fields.sorted(by: { $0.key < $1.key }) {
            body.append(Data("--\(boundary)\r\nContent-Disposition: form-data; name=\"\(name)\"\r\n\r\n\(value)\r\n".utf8))
        }
        body.append(Data("--\(boundary)\r\nContent-Disposition: form-data; name=\"file\"; filename=\"voice.wav\"\r\nContent-Type: audio/wav\r\n\r\n".utf8))
        body.append(clip.wav)
        body.append(Data("\r\n--\(boundary)--\r\n".utf8))
        return body
    }

    static func send(_ clip: VoiceClip, url: URL, fields: [String: String], headers: [String: String], session: URLSession) async throws -> String {
        let boundary = "candid-" + UUID().uuidString
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.timeoutInterval = 60
        for (k, v) in headers { request.setValue(v, forHTTPHeaderField: k) }
        request.setValue("multipart/form-data; boundary=" + boundary, forHTTPHeaderField: "Content-Type")
        let (data, response) = try await session.upload(for: request, from: body(clip, fields: fields, boundary: boundary))
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard (200..<300).contains(status) else {
            let detail = String(data: data.prefix(200), encoding: .utf8) ?? ""
            throw VoiceInputError.failed(status == 429 ? L("语音服务正忙，稍后再试") : L("转写失败（%@）%@", String(status), detail.isEmpty ? "" : L("：") + detail))
        }
        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any], let text = json["text"] as? String else {
            throw VoiceInputError.failed(L("转写结果无法读取"))
        }
        return text
    }
}

// MARK: macOS speech recognition

final class SystemVoiceEngine: LiveSpeechEngine, @unchecked Sendable {
    let kind = VoiceEngineKind.system
    private let recognizer: SFSpeechRecognizer?

    init(locale: Locale = SystemVoiceEngine.preferredLocale()) { recognizer = SFSpeechRecognizer(locale: locale) }

    /// Chinese when the user reads Chinese at all (the interface is Chinese first), otherwise the system language.
    static func preferredLocale() -> Locale {
        let zh = Locale.preferredLanguages.first { $0.hasPrefix("zh") }
        return Locale(identifier: zh.map { $0.contains("Hant") || $0.contains("TW") || $0.contains("HK") ? "zh-TW" : "zh-CN" } ?? Locale.current.identifier)
    }

    func available() async -> Bool {
        guard let recognizer, recognizer.isAvailable else { return false }
        // Not asked yet still counts: the first use asks.
        return [.authorized, .notDetermined].contains(SFSpeechRecognizer.authorizationStatus())
    }

    static func authorize() async -> Bool {
        if SFSpeechRecognizer.authorizationStatus() == .authorized { return true }
        guard SFSpeechRecognizer.authorizationStatus() == .notDetermined else { return false }
        return await withCheckedContinuation { done in SFSpeechRecognizer.requestAuthorization { done.resume(returning: $0 == .authorized) } }
    }

    func startLive(onPartial: @escaping @Sendable (String) -> Void) throws -> LiveTranscription {
        guard let recognizer, SFSpeechRecognizer.authorizationStatus() == .authorized else { throw VoiceInputError.speechPermission }
        return SystemLiveTranscription(recognizer: recognizer, onPartial: onPartial)
    }

    func transcribe(_ clip: VoiceClip) async throws -> String {
        guard await Self.authorize() else { throw VoiceInputError.speechPermission }
        guard let recognizer else { throw VoiceInputError.unavailable(L("这台 Mac 不支持语音识别")) }
        let live = SystemLiveTranscription(recognizer: recognizer, onPartial: { _ in })
        guard let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: Double(VoiceClip.sampleRate), channels: 1, interleaved: false),
              let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(max(1, clip.samples.count))) else { throw VoiceInputError.tooShort }
        buffer.frameLength = AVAudioFrameCount(clip.samples.count)
        clip.samples.withUnsafeBufferPointer { src in buffer.floatChannelData![0].update(from: src.baseAddress!, count: clip.samples.count) }
        live.append(buffer)
        return try await live.finish()
    }
}

final class SystemLiveTranscription: LiveTranscription, @unchecked Sendable {
    private let request = SFSpeechAudioBufferRecognitionRequest()
    private var task: SFSpeechRecognitionTask?
    private let lock = NSLock()
    private var latest = ""
    private var result: Result<String, Error>?
    private var waiter: CheckedContinuation<String, Error>?

    init(recognizer: SFSpeechRecognizer, onPartial: @escaping @Sendable (String) -> Void) {
        request.shouldReportPartialResults = true
        if recognizer.supportsOnDeviceRecognition { request.requiresOnDeviceRecognition = true }
        task = recognizer.recognitionTask(with: request) { [weak self] result, error in
            guard let self else { return }
            if let result {
                let text = result.bestTranscription.formattedString
                self.lock.withLock { self.latest = text }
                onPartial(text)
                if result.isFinal { self.settle(.success(text)) }
            } else if let error {
                // Ending with no speech reports an error; what was heard so far still counts.
                let heard = self.lock.withLock { self.latest }
                self.settle(heard.isEmpty ? .failure(VoiceInputError.failed(error.localizedDescription)) : .success(heard))
            }
        }
    }

    func append(_ buffer: AVAudioPCMBuffer) { request.append(buffer) }

    func finish() async throws -> String {
        request.endAudio()
        return try await withCheckedThrowingContinuation { done in
            let ready: Result<String, Error>? = lock.withLock { if result == nil { waiter = done }; return result }
            if let ready { done.resume(with: ready) }
        }
    }

    func cancel() { task?.cancel(); settle(.failure(CancellationError())) }

    private func settle(_ outcome: Result<String, Error>) {
        let waiting: CheckedContinuation<String, Error>? = lock.withLock {
            guard result == nil else { return nil }
            result = outcome
            defer { waiter = nil }
            return waiter
        }
        waiting?.resume(with: outcome)
    }
}

// MARK: Microphone

/// Opens the microphone through its own `AVAudioEngine` only while held. Collects 16 kHz mono samples and a level
/// for the waveform; forwards the raw buffers to a live recognizer when there is one.
final class VoiceRecorder: @unchecked Sendable {
    static let maximumSeconds: Double = 60
    private var engine: AVAudioEngine?
    private let lock = NSLock()
    private var samples: [Float] = []
    private var live: LiveTranscription?
    private var onLevel: (@Sendable (Float) -> Void)?

    static func authorize() async -> Bool {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized: true
        case .notDetermined: await AVCaptureDevice.requestAccess(for: .audio)
        default: false
        }
    }

    func start(live: LiveTranscription?, onLevel: @escaping @Sendable (Float) -> Void) throws {
        stopEngine()
        lock.withLock { samples = []; self.live = live; self.onLevel = onLevel }
        let engine = AVAudioEngine()
        let input = engine.inputNode
        let format = input.outputFormat(forBus: 0)
        guard format.sampleRate > 0, format.channelCount > 0,
              let target = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: Double(VoiceClip.sampleRate), channels: 1, interleaved: false),
              let converter = AVAudioConverter(from: format, to: target) else { throw VoiceInputError.unavailable(L("没有可用的麦克风")) }
        let limit = Int(Self.maximumSeconds) * VoiceClip.sampleRate
        input.installTap(onBus: 0, bufferSize: 1024, format: format) { [weak self] buffer, _ in
            guard let self else { return }
            let capacity = AVAudioFrameCount(Double(buffer.frameLength) * target.sampleRate / format.sampleRate) + 32
            guard let out = AVAudioPCMBuffer(pcmFormat: target, frameCapacity: capacity) else { return }
            var fed = false
            _ = converter.convert(to: out, error: nil) { _, status in
                if fed { status.pointee = .noDataNow; return nil }
                fed = true; status.pointee = .haveData; return buffer
            }
            let count = Int(out.frameLength)
            guard count > 0, let channel = out.floatChannelData?[0] else { return }
            let chunk = Array(UnsafeBufferPointer(start: channel, count: count))
            var sum: Float = 0
            for s in chunk { sum += s * s }
            let level = Self.level(rms: sqrt(sum / Float(count)))
            let (live, report) = self.lock.withLock { () -> (LiveTranscription?, (@Sendable (Float) -> Void)?) in
                if self.samples.count < limit { self.samples.append(contentsOf: chunk.prefix(limit - self.samples.count)) }
                return (self.live, self.onLevel)
            }
            live?.append(out)
            report?(level)
        }
        engine.prepare()
        do { try engine.start() } catch {
            input.removeTap(onBus: 0)
            throw VoiceInputError.unavailable(L("麦克风没能打开：%@", error.localizedDescription))
        }
        self.engine = engine
    }

    /// Closes the microphone and hands back what was heard.
    func stop() -> VoiceClip {
        stopEngine()
        return lock.withLock {
            let clip = VoiceClip(samples: samples)
            samples = []; live = nil; onLevel = nil
            return clip
        }
    }

    private func stopEngine() {
        guard let engine else { return }
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        self.engine = nil
    }

    /// -50…0 dBFS onto 0…1, so speech reads clearly and room noise stays near the floor.
    static func level(rms: Float) -> Float {
        guard rms > 0 else { return 0 }
        let db = 20 * log10(rms)
        return max(0, min(1, (db + 50) / 50))
    }
}
