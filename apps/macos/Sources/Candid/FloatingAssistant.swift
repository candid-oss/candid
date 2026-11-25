import AIProviders
import AppKit
import CandidLocalization
import Observation
import SwiftUI

extension Notification.Name {
    /// The voice shortcut went down (`true`) or up (`false`).
    static let candidVoiceHotkey = Notification.Name("Candid.voiceHotkey")
    /// Open the AI page on the chat (from the floating ball's 「在 AI 页继续」).
    static let candidOpenAIChat = Notification.Name("Candid.openAIChat")
}

// MARK: Status

/// What the ball's color says.
enum FloatingBallState: Equatable {
    case recording
    case recordingAudio
    case paused
    case problem(String)
    var problem: String? { if case .problem(let text) = self { text } else { nil } }
}

@MainActor enum FloatingStatus {
    /// Problems from elsewhere (the onboarding health check, once merged): each returns lines to show, empty when fine.
    static var healthChecks: [@MainActor () -> [String]] = []

    static func problems(session: DesktopSession, media: MediaWorkspaceStore, core: TimelineService.Phase) -> [String] {
        // Drafts waiting to be saved are not problems; errors, lost permissions and a full disk are.
        var lines = StatusAlerts.current(session: session, media: media).filter { $0.id != "draft" }.map(\.text)
        if case .failed(let why) = core { lines.append(L("核心没连上：%@", why)) }
        for check in healthChecks { lines += check() }
        return lines
    }

    static func state(recording: Bool, recordingAudio: Bool, problems: [String]) -> FloatingBallState {
        if let first = problems.first { return .problem(first) }
        if recordingAudio { return .recordingAudio }
        return recording ? .recording : .paused
    }
}

// MARK: Voice

/// Hold-to-talk: opens the microphone, shows the level, transcribes on release with the chosen engine (falling
/// back down the order), and hands the words to whoever asked.
@MainActor @Observable final class VoiceInputController {
    enum Phase: Equatable { case idle, listening, transcribing, failed(String) }
    private(set) var phase = Phase.idle
    /// Recent levels for the waveform, oldest first.
    private(set) var levels: [Float] = Array(repeating: 0, count: 24)
    /// Live words from the system recognizer while speaking.
    private(set) var partial = ""
    /// The engine 「自动」 or the user's choice resolves to right now.
    private(set) var resolved: VoiceEngineKind?
    private(set) var lastEngine: VoiceEngineKind?

    private let recorder = VoiceRecorder()
    private var live: LiveTranscription?
    private var liveKind: VoiceEngineKind?
    private var started: Date?
    private var task: Task<Void, Never>?
    private let defaults: UserDefaults
    let engines: [any SpeechEngine]

    init(defaults: UserDefaults = .standard, engines: [any SpeechEngine]? = nil) {
        self.defaults = defaults
        self.engines = engines ?? Self.standardEngines(defaults: defaults)
    }

    static func standardEngines(defaults: UserDefaults) -> [any SpeechEngine] {
        let secrets = AIRuntime.shared.secrets
        return [
            LocalVoiceEngine(),
            CloudVoiceEngine(target: { @MainActor in
                CloudVoiceTarget.resolve(providerID: FloatingPreferences.cloudProvider(defaults), model: FloatingPreferences.cloudModel(defaults), config: AIRuntime.shared.config())
            }, secret: { try secrets.secretWithoutPrompt(for: $0) }),
            SystemVoiceEngine()
        ]
    }

    var choice: VoiceEngineChoice { FloatingPreferences.engine(defaults) }
    var isActive: Bool { phase == .listening || phase == .transcribing }

    func refreshResolved() async { resolved = await VoiceEngines.pick(choice, from: engines)?.kind }

    /// Microphone on. The best engine is chosen now; the system one also writes live words.
    func begin() async {
        guard phase != .listening, phase != .transcribing else { return }
        task?.cancel()
        partial = ""; levels = Array(repeating: 0, count: levels.count)
        phase = .listening
        started = Date()
        guard await VoiceRecorder.authorize() else { phase = .failed(VoiceInputError.microphonePermission.localizedDescription); return }
        guard phase == .listening else { return } // released while the permission prompt was up
        let engine = await VoiceEngines.pick(choice, from: engines)
        resolved = engine?.kind
        live = nil; liveKind = nil
        if let engine = engine as? LiveSpeechEngine {
            if await SystemVoiceEngine.authorize() {
                live = try? engine.startLive(onPartial: { [weak self] text in Task { @MainActor in self?.partial = text } })
                liveKind = live == nil ? nil : engine.kind
            }
        }
        do {
            try recorder.start(live: live, onLevel: { [weak self] level in Task { @MainActor in self?.push(level) } })
        } catch {
            live?.cancel(); live = nil
            phase = .failed(error.localizedDescription)
        }
    }

    /// Microphone off; returns the words, or nil (and an explained `.failed`) when there are none.
    func end() async -> String? {
        guard phase == .listening else { return nil }
        let clip = recorder.stop()
        let held = Date().timeIntervalSince(started ?? Date())
        phase = .transcribing
        defer { if phase == .transcribing { phase = .idle } }
        guard held >= 0.4, clip.seconds >= 0.3 else { live?.cancel(); live = nil; phase = .failed(VoiceInputError.tooShort.localizedDescription); return nil }
        var tried: Set<VoiceEngineKind> = []
        if let live, let kind = liveKind {
            self.live = nil
            tried.insert(kind)
            if let text = try? await live.finish(), !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                lastEngine = kind
                return text.trimmingCharacters(in: .whitespacesAndNewlines)
            }
        }
        do {
            let outcome = try await VoiceEngines.transcribe(clip, choice: choice, engines: engines, skipping: tried)
            lastEngine = outcome.engine
            return outcome.text
        } catch {
            phase = .failed(error.localizedDescription)
            return nil
        }
    }

    func cancel() {
        guard phase == .listening else { return }
        _ = recorder.stop()
        live?.cancel(); live = nil
        phase = .idle
    }

    func dismissError() { if case .failed = phase { phase = .idle } }

    private func push(_ level: Float) {
        guard phase == .listening else { return }
        levels.removeFirst()
        levels.append(level)
    }
}

// MARK: The assistant behind the ball

/// The floating ball's model: the shared AI conversation (the same `AIPageModel` the AI page shows), the input
/// field, voice, and what the ball shows. One per app.
@MainActor @Observable final class FloatingAssistant {
    private static var instance: FloatingAssistant?
    let session: DesktopSession
    let media: MediaWorkspaceStore
    let ai: AIPageModel
    let voice = VoiceInputController()
    let note = QuickNote()

    var expanded = false
    var showingSettings = false
    var draft = ""
    /// The draft came from voice: offer 记一笔 and 帮我写 next to 发送.
    var draftFromVoice = false
    /// Bumped to move typing focus into the input field.
    private(set) var focusRequest = 0
    /// The question asked from the ball, so its answer can be shown under it.
    private(set) var askedAt: UUID?
    private(set) var notice: String?
    private var noticeTask: Task<Void, Never>?

    /// The one AI page model, shared with the AI page so both show the same conversation.
    static func shared(session: DesktopSession, media: MediaWorkspaceStore) -> FloatingAssistant {
        if let instance { return instance }
        let made = FloatingAssistant(session: session, media: media, ai: makeModel(session: session))
        instance = made
        return made
    }
    static var current: FloatingAssistant? { instance }

    /// Same scratch folders the AI page uses for demo and isolated launches.
    static func makeModel(session: DesktopSession) -> AIPageModel {
        let isolated = TimelineService.isolated(isDemo: session.isDemo)
        let scratch = URL(fileURLWithPath: session.dataLocation, isDirectory: true).appendingPathComponent("AI", isDirectory: true)
        return AIPageModel(agent: isolated ? PiAgent(directory: scratch.appendingPathComponent("pi", isDirectory: true)) : nil,
                           store: isolated ? AIConversationStore(directory: scratch.appendingPathComponent("conversations", isDirectory: true)) : .standard)
    }

    init(session: DesktopSession, media: MediaWorkspaceStore, ai: AIPageModel) {
        self.session = session; self.media = media; self.ai = ai
    }

    // MARK: What the ball shows

    var problems: [String] { FloatingStatus.problems(session: session, media: media, core: TimelineService.shared.phase) }
    var state: FloatingBallState { FloatingStatus.state(recording: session.isRecording, recordingAudio: media.isRecording, problems: problems) }
    /// Proposals the App can carry out, waiting for a yes.
    var proposals: [AgendaProposal] { ai.waiting.filter(\.executable) }
    /// Dates and reminders found in conversations plus proposals: the badge number.
    var pendingCount: Int { ai.items.count + proposals.count }

    /// The question asked from the ball and its answer (nil until something was asked here).
    var exchange: (question: AIChatMessage, answer: AIChatMessage?)? {
        guard let askedAt, let i = ai.conversation.messages.firstIndex(where: { $0.id == askedAt }) else { return nil }
        let answer = ai.conversation.messages[(i + 1)...].first { $0.role != .user }
        return (ai.conversation.messages[i], answer)
    }

    func refresh() async { await ai.refreshCore() }

    // MARK: Actions

    func toggleRecording() {
        if session.isRecording { session.pauseRecording() } else { session.startRecording() }
    }

    /// 问 Candid: through the AI page's conversation (Pi, `agent_chat`), saved and recorded like any other turn.
    func ask() async {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !ai.turnRunning else { return }
        ai.draft = text
        draft = ""; draftFromVoice = false
        await ai.send()
        askedAt = ai.conversation.messages.last(where: { $0.role == .user && $0.text == text })?.id
    }

    func stopAnswer() { ai.stopTurn() }

    /// 记一笔 with the draft as the note.
    func saveNote() async {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        note.target = nil
        note.text = text
        if await note.save() { draft = ""; draftFromVoice = false; flash(L("已记下")) } else { flash(note.error ?? L("笔记未保存，可重试")) }
    }

    /// 口述给帮我写: the draft becomes the instruction.
    func compose() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        draft = ""; draftFromVoice = false
        expanded = false
        FloatingControl.shared.closePanel()
        ComposePanel.shared.show(instruction: text.isEmpty ? nil : text)
    }

    func continueInAI() {
        expanded = false
        FloatingControl.shared.closePanel()
        NotificationCenter.default.post(name: .candidOpenAIChat, object: "chat")
    }

    /// 「全部」 under 待确认: the AI page's 「今天」.
    func openToday() {
        expanded = false
        FloatingControl.shared.closePanel()
        NotificationCenter.default.post(name: .candidOpenAIChat, object: "today")
    }

    func requestFocus() { focusRequest += 1 }

    func quick(_ action: QuickAction) {
        FloatingControl.shared.closePanel()
        NotificationCenter.default.post(name: .candidQuickAction, object: action)
    }

    // MARK: Voice

    var voiceEnabled: Bool { FloatingPreferences.voiceEnabled() }

    func beginVoice() {
        guard voiceEnabled else { return }
        FloatingControl.shared.openPanel()
        Task { await voice.begin() }
    }

    func endVoice() {
        Task {
            guard let text = await voice.end() else { return }
            draft = draft.isEmpty ? text : draft + " " + text
            draftFromVoice = true
            FloatingControl.shared.openPanel(focus: true)
        }
    }

    private func flash(_ text: String) {
        notice = text
        noticeTask?.cancel()
        noticeTask = Task { try? await Task.sleep(for: .seconds(2.5)); if !Task.isCancelled { notice = nil } }
    }
}
