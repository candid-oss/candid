import AIProviders
import Carbon
import Foundation
import Testing
@testable import Candid

/// A scripted recognizer: available or not, and what it answers.
private final class FakeEngine: SpeechEngine, @unchecked Sendable {
    let kind: VoiceEngineKind
    let isAvailable: Bool
    let answer: Result<String, Error>
    private(set) var calls = 0
    init(_ kind: VoiceEngineKind, available: Bool = true, answer: Result<String, Error> = .success("你好")) {
        self.kind = kind; self.isAvailable = available; self.answer = answer
    }
    func available() async -> Bool { isAvailable }
    func transcribe(_ clip: VoiceClip) async throws -> String { calls += 1; return try answer.get() }
}

private let clip = VoiceClip(samples: Array(repeating: 0.1, count: 16_000))

@Test func voiceEngineOrderPutsTheChoiceFirstThenBestToLastResort() {
    #expect(VoiceEngines.order(.automatic) == [.local, .cloud, .system])
    #expect(VoiceEngines.order(.local) == [.local, .cloud, .system])
    #expect(VoiceEngines.order(.cloud) == [.cloud, .local, .system])
    #expect(VoiceEngines.order(.system) == [.system, .local, .cloud])
}

@Test func voicePickTakesTheFirstAvailableEngine() async {
    let engines: [any SpeechEngine] = [FakeEngine(.local, available: false), FakeEngine(.cloud), FakeEngine(.system)]
    #expect(await VoiceEngines.pick(.automatic, from: engines)?.kind == .cloud)
    #expect(await VoiceEngines.pick(.system, from: engines)?.kind == .system)
    // The chosen engine is unavailable: fall to the next one in order.
    #expect(await VoiceEngines.pick(.local, from: engines)?.kind == .cloud)
    let none: [any SpeechEngine] = [FakeEngine(.local, available: false), FakeEngine(.system, available: false)]
    #expect(await VoiceEngines.pick(.automatic, from: none) == nil)
}

@Test func voiceTranscriptionFallsBackWhenAnEngineFailsOrIsUnavailable() async throws {
    let local = FakeEngine(.local, answer: .failure(VoiceInputError.failed("500")))
    let cloud = FakeEngine(.cloud, available: false)
    let system = FakeEngine(.system, answer: .success("  明天下午三点开会  "))
    let outcome = try await VoiceEngines.transcribe(clip, choice: .automatic, engines: [local, cloud, system])
    #expect(outcome.text == "明天下午三点开会")
    #expect(outcome.engine == .system)
    #expect(Set(outcome.skipped.keys) == [.local, .cloud])
    #expect(local.calls == 1 && cloud.calls == 0 && system.calls == 1)
}

@Test func voiceTranscriptionSkipsAnEngineAlreadyTriedLive() async throws {
    let system = FakeEngine(.system)
    let local = FakeEngine(.local, answer: .success("本机"))
    let outcome = try await VoiceEngines.transcribe(clip, choice: .system, engines: [system, local], skipping: [.system])
    #expect(outcome.engine == .local)
    #expect(system.calls == 0)
}

@Test func voiceSilenceDoesNotFallBackAndNothingAvailableIsExplained() async {
    let local = FakeEngine(.local, answer: .success("   "))
    let cloud = FakeEngine(.cloud)
    await #expect(throws: VoiceInputError.nothingHeard) {
        try await VoiceEngines.transcribe(clip, choice: .automatic, engines: [local, cloud])
    }
    #expect(cloud.calls == 0)
    do {
        _ = try await VoiceEngines.transcribe(clip, choice: .automatic, engines: [FakeEngine(.local, available: false)])
        Issue.record("expected unavailable")
    } catch let error as VoiceInputError {
        guard case .unavailable = error else { Issue.record("expected unavailable, got \(error)"); return }
    } catch { Issue.record("unexpected \(error)") }
    do {
        _ = try await VoiceEngines.transcribe(clip, choice: .cloud, engines: [FakeEngine(.cloud, answer: .failure(VoiceInputError.failed("401")))])
        Issue.record("expected failure")
    } catch let error as VoiceInputError {
        guard case .failed(let why) = error else { Issue.record("expected failed, got \(error)"); return }
        #expect(why.contains("401"))
    } catch { Issue.record("unexpected \(error)") }
}

@Test func cloudVoiceTargetReusesAnExistingSourceReadOnly() throws {
    let openai = AIProvider(providerID: "openai", name: "OpenAI", api: .openaiChat, baseURL: "https://api.example.test/v1/", secretRef: "ref-openai", destination: .cloud)
    let anthropic = AIProvider(providerID: "claude", name: "Claude", api: .anthropicMessages, baseURL: "https://claude.example.test", destination: .cloud)
    var disabled = AIProvider(providerID: "off", name: "Off", api: .openaiChat, baseURL: "https://off.example.test/v1", destination: .cloud)
    disabled.enabled = false
    let local = AIProvider(providerID: "candid-local-voice", name: "Voice", api: .other, baseURL: "http://127.0.0.1:8767/v1", destination: .onDevice)
    let config = ProviderConfig(providers: [openai, anthropic, disabled, local], models: [])
    let target = try #require(CloudVoiceTarget.resolve(providerID: "openai", model: " whisper-1 ", config: config))
    #expect(target.url.absoluteString == "https://api.example.test/v1/audio/transcriptions")
    #expect(target.model == "whisper-1")
    #expect(target.secretRef == "ref-openai")
    #expect(CloudVoiceTarget.resolve(providerID: "claude", model: "whisper-1", config: config) == nil)
    #expect(CloudVoiceTarget.resolve(providerID: "off", model: "whisper-1", config: config) == nil)
    #expect(CloudVoiceTarget.resolve(providerID: "openai", model: "", config: config) == nil)
    #expect(CloudVoiceTarget.resolve(providerID: "missing", model: "whisper-1", config: config) == nil)
    #expect(CloudVoiceTarget.candidates(config).map(\.providerID) == ["openai"])
}

@Test func cloudVoiceEngineNeedsATargetAndAKey() async {
    let target = CloudVoiceTarget(url: URL(string: "https://api.example.test/v1/audio/transcriptions")!, model: "whisper-1", headers: [:], secretRef: "ref", destination: .cloud)
    #expect(await CloudVoiceEngine(target: { target }, secret: { _ in "sk-test" }).available())
    #expect(await !CloudVoiceEngine(target: { target }, secret: { _ in nil }).available())
    #expect(await !CloudVoiceEngine(target: { nil }, secret: { _ in "sk-test" }).available())
}

@Test func voiceClipEncodesAWavAndTheUploadCarriesTheModel() {
    let wav = clip.wav
    #expect(wav.count == 44 + 16_000 * 2)
    #expect(String(data: wav.prefix(4), encoding: .ascii) == "RIFF")
    #expect(String(data: wav[8..<12], encoding: .ascii) == "WAVE")
    let body = String(decoding: OpenAITranscription.body(VoiceClip(samples: [0]), fields: ["model": "whisper-1"], boundary: "b"), as: UTF8.self)
    #expect(body.contains("name=\"model\"\r\n\r\nwhisper-1"))
    #expect(body.contains("filename=\"voice.wav\""))
    #expect(body.hasSuffix("--b--\r\n"))
}

@Test func voicePreferencesDefaultToAutomaticWithVoiceOn() throws {
    let name = "synthetic.voice." + UUID().uuidString
    let defaults = try #require(UserDefaults(suiteName: name))
    defer { defaults.removePersistentDomain(forName: name) }
    #expect(FloatingPreferences.voiceEnabled(defaults))
    #expect(FloatingPreferences.engine(defaults) == .automatic)
    #expect(FloatingPreferences.cloudModel(defaults) == "whisper-1")
    defaults.set("system", forKey: FloatingPreferences.engineKey)
    defaults.set(false, forKey: FloatingPreferences.voiceKey)
    #expect(FloatingPreferences.engine(defaults) == .system)
    #expect(!FloatingPreferences.voiceEnabled(defaults))
}

@MainActor @Test func floatingBallStateShowsProblemsFirst() {
    #expect(FloatingStatus.state(recording: true, recordingAudio: false, problems: []) == .recording)
    #expect(FloatingStatus.state(recording: false, recordingAudio: false, problems: []) == .paused)
    #expect(FloatingStatus.state(recording: true, recordingAudio: true, problems: []) == .recordingAudio)
    #expect(FloatingStatus.state(recording: true, recordingAudio: false, problems: ["窗口截图需要屏幕录制授权"]) == .problem("窗口截图需要屏幕录制授权"))
}

@MainActor @Test func voiceHotkeyStartsOnOptionSpaceAndCanBeTurnedOff() throws {
    let name = "synthetic.hotkeys." + UUID().uuidString
    let defaults = try #require(UserDefaults(suiteName: name))
    defer { defaults.removePersistentDomain(forName: name) }
    let model = GlobalHotkeys(defaults: defaults)
    #expect(model.voiceBinding == .init(key: 49, modifiers: UInt32(Carbon.optionKey)))
    #expect(model.voiceKeyIndex == 0)
    model.configureVoice(nil)
    #expect(GlobalHotkeys(defaults: defaults).voiceBinding == nil)
    model.configureVoice(GlobalHotkeys.voiceKeys[1].binding)
    #expect(GlobalHotkeys(defaults: defaults).voiceKeyIndex == 1)
}

/// Real Qwen3-ASR on this Mac: `CANDID_VOICE_WAV=<16 kHz mono 16-bit wav>` with the voice service running.
@Test(.enabled(if: ProcessInfo.processInfo.environment["CANDID_VOICE_WAV"] != nil))
func localVoiceEngineTranscribesARealClip() async throws {
    let data = try Data(contentsOf: URL(fileURLWithPath: ProcessInfo.processInfo.environment["CANDID_VOICE_WAV"]!))
    let pcm = data.dropFirst(44)
    let samples = stride(from: pcm.startIndex, to: pcm.endIndex - 1, by: 2).map { i in
        Float(Int16(bitPattern: UInt16(pcm[i]) | UInt16(pcm[i + 1]) << 8)) / Float(Int16.max)
    }
    let engine = LocalVoiceEngine()
    #expect(await engine.available())
    let outcome = try await VoiceEngines.transcribe(VoiceClip(samples: samples), choice: .automatic, engines: [engine])
    #expect(outcome.engine == .local)
    print("local transcription:", outcome.text)
    #expect(!outcome.text.isEmpty)
}
