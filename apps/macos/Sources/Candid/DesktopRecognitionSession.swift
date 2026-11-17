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
    private(set) var powerBlocks: [DesktopRecognitionKind: String] = [:]

    @ObservationIgnored private let defaults: UserDefaults
    @ObservationIgnored private let ocr = DesktopOCR()
    @ObservationIgnored private var powerGate = DesktopRecognitionPowerGate()
    @ObservationIgnored private var powerObservation: DesktopRecognitionPowerObservation?
    @ObservationIgnored private var store: DesktopStore?
    /// Screenshots whose original could not be read this session; automatic passes skip them.
    @ObservationIgnored private var unreadable: Set<String> = []
    @ObservationIgnored private var timer: Task<Void, Never>?
    @ObservationIgnored private var job: Task<Void, Never>?
    @ObservationIgnored private var retryAfter: [DesktopRecognitionKind: Date] = [:]
    @ObservationIgnored private var batches: [DesktopRecognitionKind: Batch] = [:]
    @ObservationIgnored private var lastAutomaticKind: DesktopRecognitionKind = .vision
    @ObservationIgnored private var configObserver: NSObjectProtocol?

    private struct Batch {
        var next: Date
        var remaining = 0
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
        if automaticEnabled {
            powerObservation = DesktopRecognitionPowerObservation { [weak self] reset in
                if reset { self?.powerGate.reset() }
                self?.refreshPower()
                self?.wake()
            }
        }
        refreshPower()
        if automaticEnabled { wake() }
    }

    func stop() {
        store = nil
        timer?.cancel()
        job?.cancel()
        powerObservation = nil
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
        if updated.mode != previous.mode || updated.batchIntervalMinutes != previous.batchIntervalMinutes {
            setBatch(Batch(next: Date().addingTimeInterval(updated.batchInterval)), for: kind)
        }
        if activeKind == kind { job?.cancel() }
        retryAfter[kind] = nil
        status[kind] = nil
        refreshPower()
        wake()
    }

    func refreshPower() {
        let power = DesktopRecognitionPowerSource.read()
        let uptime = ProcessInfo.processInfo.systemUptime
        for kind in DesktopRecognitionKind.allCases {
            powerBlocks[kind] = powerGate.blockReason(policy: policy(kind), power: power, uptime: uptime)
        }
        if let activeKind, powerBlocks[activeKind] != nil { job?.cancel() }
    }

    func wake() {
        timer?.cancel()
        guard store != nil, powerObservation != nil, job == nil,
              DesktopRecognitionKind.allCases.contains(where: { policy($0).mode != .manual }) else { return }
        timer = Task { [weak self] in await self?.evaluate() }
    }

    func results(for screenshot: DesktopRecord) async throws -> [DesktopRecognitionKind: DesktopRecognitionResult] {
        try await store?.recognitionResults(for: screenshot.id) ?? [:]
    }

    func runManually(_ kind: DesktopRecognitionKind, screenshot: DesktopRecord, promptOverride: String? = nil, focusMainPane: Bool? = nil) {
        refreshPower()
        guard job == nil else {
            manualFeedback = (screenshot.id, DesktopRecognitionError.busy.localizedDescription)
            return
        }
        if let reason = powerBlocks[kind] {
            manualFeedback = (screenshot.id, reason)
            return
        }
        manualFeedback = nil
        start(kind, screenshot: screenshot, automatic: false, promptOverride: promptOverride, focusMainPane: focusMainPane)
    }

    func runBatchNow(_ kind: DesktopRecognitionKind) {
        guard policy(kind).mode == .batch, powerObservation != nil, job == nil else { return }
        retryAfter[kind] = nil
        setBatch(Batch(next: Date()), for: kind)
        wake()
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
        refreshPower()
        let now = Date()
        let kinds: [DesktopRecognitionKind] = lastAutomaticKind == .ocr ? [.vision, .ocr] : [.ocr, .vision]
        for kind in kinds {
            let policy = policy(kind)
            guard policy.mode != .manual else { continue }
            if let reason = powerBlocks[kind] {
                status[kind] = reason
                if let wait = powerGate.remainingPluggedWait(policy: policy, uptime: ProcessInfo.processInfo.systemUptime) {
                    schedule(now.addingTimeInterval(wait))
                }
                continue
            }
            if kind == .vision, !visionConfigured {
                status[kind] = DesktopRecognitionError.missingModel.localizedDescription
                continue
            }
            if let retry = retryAfter[kind], retry > now { schedule(retry); continue }
            var batch = batches[kind] ?? Batch(next: defaults.object(forKey: batchKey(kind)) as? Date ?? now)
            if policy.mode == .batch && batch.remaining == 0 && batch.next > now {
                status[kind] = L("下一批约在 ") + batch.next.formatted(date: .omitted, time: .shortened)
                schedule(batch.next)
                continue
            }
            do {
                let record = try await store.pendingRecognition(kind, since: policy.earliestAutomaticDate, skip: unreadable)
                guard !Task.isCancelled, job == nil else { return }
                if policy.mode == .batch {
                    if batch.remaining == 0 { batch = Batch(next: now.addingTimeInterval(policy.batchInterval), remaining: policy.batchSize) }
                    batch.remaining = record == nil ? 0 : batch.remaining - 1
                    setBatch(batch, for: kind)
                }
                guard let record else {
                    status[kind] = L("没有待处理的截图")
                    if policy.mode == .batch { schedule(batch.next) }
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
                let (jpeg, evidence) = try await store.recognitionInput(screenshot)
                refreshPower()
                try Task.checkCancellation()
                let result: DesktopRecognitionResult
                switch kind {
                case .ocr:
                    if automatic, let previous = try await store.existingOCR(forImageHash: hash), previous.ocrLines != nil { result = previous }
                    else if AIRuntime.shared.model(for: "ocr") != nil { result = try await modelOCR(jpeg) }
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
                    result = try await modelVision(jpeg, prompt: requestPrompt, ocrReference: reference, windowContext: context,
                                                   focusMainPane: focusMainPane, axEvidence: evidence ?? screenshot.context?.axEvidence)
                }
                refreshPower()
                try Task.checkCancellation()
                try await store.saveRecognition(result, kind: kind, screenshot: screenshot)
                resultRevision &+= 1
                status[kind] = L("已完成")
            } catch {
                if Task.isCancelled {
                    status[kind] = powerBlocks[kind] ?? L("已取消")
                    if !automatic, let reason = powerBlocks[kind] { manualFeedback = (screenshot.id, reason) }
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

    private func setBatch(_ batch: Batch, for kind: DesktopRecognitionKind) {
        if batches[kind]?.next != batch.next { defaults.set(batch.next, forKey: batchKey(kind)) }
        batches[kind] = batch
    }

    private func batchKey(_ kind: DesktopRecognitionKind) -> String { "desktop.recognition.next.\(kind.rawValue)" }

    private static func readPolicy(_ kind: DesktopRecognitionKind, from defaults: UserDefaults) -> DesktopRecognitionPolicy {
        guard let data = defaults.data(forKey: "desktop.recognition.\(kind.rawValue)"),
              let policy = try? JSONDecoder().decode(DesktopRecognitionPolicy.self, from: data) else { return DesktopRecognitionPolicy() }
        return policy
    }
}
