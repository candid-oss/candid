import AIProviders
import CandidLocalization
import AppKit
import DesktopCore
import DesktopPlatform
import Foundation
import Observation

@MainActor @Observable
final class DesktopRecognitionSession {
    private(set) var ocrPolicy: DesktopRecognitionPolicy
    private(set) var visionPolicy: DesktopRecognitionPolicy
    /// Models, prompts and keys live in 设置 → 模型 (roles 「图像理解」 and 「文字识别」); this session only decides
    /// when to recognise and keeps the per-request output ceiling.
    var maximumTokens: Int { didSet { defaults.set(maximumTokens, forKey: "desktop.vision.maximumTokens") } }
    var useOCRReference: Bool { didSet { defaults.set(useOCRReference, forKey: "desktop.vision.useOCRReference") } }

    private(set) var activeKind: DesktopRecognitionKind?
    private(set) var activeScreenshotID: String?
    private(set) var resultRevision = 0
    private(set) var manualFeedback: (id: String, message: String)?
    private(set) var status: [DesktopRecognitionKind: String] = [:]
    /// Why automatic recognition waits on the core's power gate (`waiting_power`, `waiting_thermal`,
    /// `waiting_manual`), per kind; nil when it may run.
    private(set) var powerWait: [DesktopRecognitionKind: String] = [:]

    @ObservationIgnored private let defaults: UserDefaults
    @ObservationIgnored private let ocr = DesktopOCR()
    @ObservationIgnored private var automaticEnabled = false
    @ObservationIgnored private var powerObserver: NSObjectProtocol?
    @ObservationIgnored private var store: DesktopStore?
    /// Screenshots whose original could not be read this session; automatic passes skip them.
    @ObservationIgnored private var unreadable: Set<String> = []
    @ObservationIgnored private var timer: Task<Void, Never>?
    @ObservationIgnored private var job: Task<Void, Never>?
    @ObservationIgnored private var retryAfter: [DesktopRecognitionKind: Date] = [:]
    @ObservationIgnored private var lastAutomaticKind: DesktopRecognitionKind = .vision
    @ObservationIgnored private var configObserver: NSObjectProtocol?
    /// System OCR of uncompressed frames, by screenshot ID: run at capture, before the original is stored as WebP.
    @ObservationIgnored private var frameOCR: [String: Task<FrameOCR, any Error>] = [:]
    @ObservationIgnored private var frameOCRTail: Task<FrameOCR, any Error>?

    /// A capture-time OCR result with what it cost, for the energy ledger.
    private struct FrameOCR: Sendable {
        let result: DesktopRecognitionResult
        let start: BackgroundPower.Mark
        let end: BackgroundPower.Mark
    }

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        ocrPolicy = Self.readPolicy(.ocr, from: defaults)
        visionPolicy = Self.readPolicy(.vision, from: defaults)
        maximumTokens = defaults.object(forKey: "desktop.vision.maximumTokens") as? Int ?? DesktopVisionClient.defaultMaximumTokens
        useOCRReference = defaults.object(forKey: "desktop.vision.useOCRReference") as? Bool ?? true
        configObserver = NotificationCenter.default.addObserver(forName: ModelSettingsModel.configChanged, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.connectionChanged() }
        }
    }

    func attach(to store: DesktopStore, automaticEnabled: Bool) {
        self.store = store
        self.automaticEnabled = automaticEnabled
        if automaticEnabled {
            // The core's gate changed (power, heat, pace): look again.
            powerObserver = NotificationCenter.default.addObserver(forName: BackgroundPower.changed, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.wake() }
            }
            wake()
        }
    }

    func stop() {
        frameOCR.values.forEach { $0.cancel() }
        frameOCR = [:]; frameOCRTail = nil
        store = nil
        timer?.cancel()
        job?.cancel()
        automaticEnabled = false
        if let powerObserver { NotificationCenter.default.removeObserver(powerObserver) }
        powerObserver = nil
    }

    func policy(_ kind: DesktopRecognitionKind) -> DesktopRecognitionPolicy {
        kind == .ocr ? ocrPolicy : visionPolicy
    }

    func setPolicy(_ policy: DesktopRecognitionPolicy, for kind: DesktopRecognitionKind) {
        let previous = self.policy(kind)
        guard policy != previous else { return }
        var updated = policy
        if previous.mode == .manual && policy.mode != .manual { updated.automaticSince = Date() }
        if kind == .ocr { ocrPolicy = updated } else { visionPolicy = updated }
        if let data = try? JSONEncoder().encode(updated) { defaults.set(data, forKey: "desktop.recognition.\(kind.rawValue)") }
        if activeKind == kind { job?.cancel() }
        retryAfter[kind] = nil
        status[kind] = nil
        wake()
    }

    func wake() {
        timer?.cancel()
        guard store != nil, automaticEnabled, job == nil,
              DesktopRecognitionKind.allCases.contains(where: { policy($0).mode != .manual }) else { return }
        timer = Task { [weak self] in await self?.evaluate() }
    }

    /// Whether capture should hand over the uncompressed frame: automatic OCR of new screenshots with the system
    /// engine, allowed by the core's power gate at its last answer. Otherwise the stored WebP is recognised later.
    var wantsRawFrames: Bool {
        store != nil && ocrPolicy.mode == .immediate && powerWait[.ocr] == nil && AIRuntime.shared.model(for: "ocr") == nil
    }

    /// Starts system OCR on the uncompressed frame; the automatic pass uses it when it reaches this screenshot.
    /// Frames are recognised one at a time, in capture order.
    func recognizeRawFrame(_ frame: DesktopRawFrame, screenshotID: String) {
        guard wantsRawFrames else { return }
        // Results the automatic pass never collected (deleted screenshot, policy changed) are dropped, not hoarded.
        while frameOCR.count >= 16, let stale = frameOCR.keys.first { frameOCR.removeValue(forKey: stale)?.cancel() }
        let previous = frameOCRTail
        let task = Task.detached(priority: .utility) { () throws -> FrameOCR in
            _ = try? await previous?.value
            try Task.checkCancellation()
            let start = BackgroundPower.Mark.now()
            let result = try DesktopOCR.recognize(frame: frame.image)
            return FrameOCR(result: result, start: start, end: .now())
        }
        frameOCR[screenshotID] = task
        frameOCRTail = task
    }

    func results(for screenshot: DesktopRecord) async throws -> [DesktopRecognitionKind: DesktopRecognitionResult] {
        try await store?.recognitionResults(for: screenshot.id) ?? [:]
    }

    /// A manual run is 「现在处理」: it passes the power gate (power v1 §2.2).
    func runManually(_ kind: DesktopRecognitionKind, screenshot: DesktopRecord, promptOverride: String? = nil, focusMainPane: Bool? = nil) {
        guard job == nil else {
            manualFeedback = (screenshot.id, DesktopRecognitionError.busy.localizedDescription)
            return
        }
        manualFeedback = nil
        start(kind, screenshot: screenshot, automatic: false, promptOverride: promptOverride, focusMainPane: focusMainPane)
    }

    func cancel() {
        if let activeScreenshotID { manualFeedback = (activeScreenshotID, L("已取消识别。")) }
        if let activeKind { retryAfter[activeKind] = Date().addingTimeInterval(60) }
        job?.cancel()
    }

    /// The 「图像理解」 prompt (the user's own or the default); saving the default clears the override.
    var prompt: String {
        get { AIRuntime.shared.prompt(for: "vision") }
        set { try? AIRuntime.shared.setPrompt(newValue, for: "vision"); connectionChanged() }
    }

    /// Whether 「图像理解」 has a model; automatic vision waits until it does.
    var visionConfigured: Bool { AIRuntime.shared.model(for: "vision") != nil }

    /// Call when the model settings change so a waiting automatic pass retries promptly.
    func modelsChanged() { connectionChanged() }

    private func connectionChanged() {
        if activeKind == .vision { job?.cancel() }
        retryAfter[.vision] = nil
        status[.vision] = nil
        wake()
    }

    private func evaluate() async {
        guard let store, job == nil else { return }
        var nextWake: Date?
        func schedule(_ time: Date) { nextWake = min(nextWake ?? time, time) }
        defer {
            if !Task.isCancelled, job == nil, let nextWake {
                timer = Task { [weak self] in
                    do { try await Task.sleep(for: .seconds(max(0.1, nextWake.timeIntervalSinceNow))) } catch { return }
                    await self?.evaluate()
                }
            }
        }
        let now = Date()
        let kinds: [DesktopRecognitionKind] = lastAutomaticKind == .ocr ? [.vision, .ocr] : [.ocr, .vision]
        for kind in kinds {
            let policy = policy(kind)
            guard policy.mode != .manual else { continue }
            if kind == .vision, !visionConfigured {
                status[kind] = DesktopRecognitionError.missingModel.localizedDescription
                continue
            }
            // The core's power gate decides; a change there posts `BackgroundPower.changed`, which wakes this pass.
            let (work, role) = Self.work(kind)
            let wait = await BackgroundPower.shared.allows(work, local: work == .vision ? BackgroundPower.modelIsLocal(role: role) : nil)
            guard !Task.isCancelled, job == nil else { return }
            powerWait[kind] = wait
            if let wait {
                status[kind] = BackgroundPower.label(wait)
                continue
            }
            if let retry = retryAfter[kind], retry > now { schedule(retry); continue }
            do {
                let record = try await store.pendingRecognition(kind, since: policy.earliestAutomaticDate, skip: unreadable)
                guard !Task.isCancelled, job == nil else { return }
                guard let record else {
                    status[kind] = L("没有待处理的截图")
                    continue
                }
                lastAutomaticKind = kind
                start(kind, screenshot: record, automatic: true)
                return
            } catch {
                status[kind] = error.localizedDescription
                let retry = now.addingTimeInterval(300)
                retryAfter[kind] = retry
                schedule(retry)
            }
        }
    }

    /// Which registered app work a recognition kind is, and the model role it would use.
    private static func work(_ kind: DesktopRecognitionKind) -> (AppBackgroundWork, String) {
        if kind == .ocr { return (AIRuntime.shared.model(for: "ocr") == nil ? .ocr : .vision, "ocr") }
        return (.vision, "vision")
    }

    private func start(_ kind: DesktopRecognitionKind, screenshot: DesktopRecord, automatic: Bool, promptOverride: String? = nil, focusMainPane: Bool? = nil) {
        guard let store, let hash = screenshot.imageHash else { return }
        if manualFeedback?.id == screenshot.id { manualFeedback = nil }
        activeKind = kind
        activeScreenshotID = screenshot.id
        status[kind] = L("正在处理截图")
        let requestPrompt = kind == .vision ? (promptOverride ?? prompt) : ""
        job = Task(priority: .utility) { [self] in
            defer {
                activeKind = nil
                activeScreenshotID = nil
                job = nil
                wake()
            }
            do {
                if kind == .ocr, let early = frameOCR.removeValue(forKey: screenshot.id), let frame = try? await early.value {
                    try Task.checkCancellation()
                    try await store.saveRecognition(frame.result, kind: kind, screenshot: screenshot)
                    BackgroundPower.shared.account(.ocr, subject: screenshot.id, cpuMs: frame.end.cpuMs - frame.start.cpuMs, wallMs: frame.end.wallMs - frame.start.wallMs)
                    resultRevision &+= 1
                    status[kind] = L("已完成")
                    return
                }
                let (jpeg, evidence) = try await store.recognitionInput(screenshot) // the stored original: WebP or JPEG
                try Task.checkCancellation()
                let mark = BackgroundPower.Mark.now()
                let result: DesktopRecognitionResult
                switch kind {
                case .ocr:
                    if automatic, let previous = try await store.existingOCR(forImageHash: hash), previous.ocrLines != nil { result = previous }
                    else if AIRuntime.shared.model(for: "ocr") != nil { result = try await modelOCR(DesktopImageEncoder.jpegForModels(jpeg)) }
                    else { result = try await ocr.recognize(jpeg) }
                case .vision:
                    let reference = useOCRReference ? try await store.recognitionResults(for: screenshot.id)[.ocr] : nil
                    var context = ["app": screenshot.appName]
                    if !screenshot.windowTitle.isEmpty { context["windowTitle"] = screenshot.windowTitle }
                    if let focus = screenshot.context, focus.isProtected != true, focus.textStatus != "protected" {
                        context["focusedRole"] = focus.focusedRole
                        context["focusedLabel"] = focus.focusedLabel
                        context["selectedText"] = focus.selectedText
                    }
                    try Task.checkCancellation()
                    result = try await modelVision(DesktopImageEncoder.jpegForModels(jpeg), prompt: requestPrompt, ocrReference: reference, windowContext: context,
                                                   focusMainPane: focusMainPane, axEvidence: evidence ?? screenshot.context?.axEvidence)
                }
                try Task.checkCancellation()
                BackgroundPower.shared.account(Self.work(kind).0, subject: screenshot.id, since: mark)
                try await store.saveRecognition(result, kind: kind, screenshot: screenshot)
                resultRevision &+= 1
                status[kind] = L("已完成")
            } catch {
                if Task.isCancelled {
                    status[kind] = L("已取消")
                    return
                }
                let message = error.localizedDescription
                if !automatic { manualFeedback = (screenshot.id, message) }
                status[kind] = message
                if automatic, error as? DesktopStoreError == .missingImage || error as? DesktopStoreError == .deletedRecord {
                    unreadable.insert(screenshot.id)
                } else {
                    retryAfter[kind] = Date().addingTimeInterval(300)
                }
            }
        }
    }

    // MARK: Model roles

    /// 「图像理解」: the same evidence-rich request as before, sent through the shared client so the call is
    /// recorded and the model, key and fallbacks come from 设置 → 模型.
    private func modelVision(_ jpeg: Data, prompt: String, ocrReference: DesktopRecognitionResult?, windowContext: [String: String],
                             focusMainPane: Bool?, axEvidence: DesktopAXEvidence?) async throws -> DesktopRecognitionResult {
        let prepared = try DesktopVisionClient.prepare(jpeg, prompt: prompt, ocrReference: ocrReference, windowContext: windowContext,
                                                       focusMainPane: focusMainPane, axEvidence: axEvidence)
        let request = ChatRequest(messages: [ChatMessage(.user, prepared.instruction, images: [ChatImage(data: prepared.image)])],
                                  maxTokens: min(max(maximumTokens, 256), 8_192), temperature: 0)
        let response = try await AIRuntime.shared.client.complete(request, role: "vision", context: CallContext(feature: "vision"))
        return try DesktopVisionClient.finish(response.text, finishReason: response.finishReason,
                                              source: response.servedModel ?? "vision", prepared: prepared)
    }

    /// 「文字识别」 assigned to a model: plain transcription, no line positions (the system engine keeps those).
    private func modelOCR(_ jpeg: Data) async throws -> DesktopRecognitionResult {
        let request = ChatRequest(messages: [ChatMessage(.user, AIRuntime.shared.prompt(for: "ocr"), images: [ChatImage(data: jpeg)])],
                                  maxTokens: min(max(maximumTokens, 256), 8_192), temperature: 0)
        let response = try await AIRuntime.shared.client.complete(request, role: "ocr", context: CallContext(feature: "ocr"))
        let text = response.text.trimmingCharacters(in: .whitespacesAndNewlines)
        if response.finishReason == "length" || response.finishReason == "max_tokens" { throw DesktopRecognitionError.incompleteLayout }
        return DesktopRecognitionResult(source: response.servedModel ?? "ocr", text: text)
    }

    private static func readPolicy(_ kind: DesktopRecognitionKind, from defaults: UserDefaults) -> DesktopRecognitionPolicy {
        guard let data = defaults.data(forKey: "desktop.recognition.\(kind.rawValue)"),
              let policy = try? JSONDecoder().decode(DesktopRecognitionPolicy.self, from: data) else { return DesktopRecognitionPolicy() }
        return policy
    }
}
