import DesktopCore
import CandidLocalization
import Foundation
import Observation
import CandidClient

/// How the desktop capture reaches the timeline core: its local channel and whether capture writes directly yet.
@MainActor protocol DesktopCoreAccess: AnyObject {
    var channel: LocalChannel? { get }
    /// The core finished the switch from the old ledger (timeline-v1 §9.1); desktop batches are accepted.
    var captureDirect: Bool { get }
}

/// The desktop capture's storage (ADR-024: only the core opens the database). Writes go into the durable outbox
/// first and count as saved there; a sender hands them to the core's ingest in order once the core runs and the
/// cutover is done. Reads use the core's `app_*` tools and return the timeline types the views draw; originals are
/// opened by path, never sent as bytes.
@MainActor @Observable final class DesktopStore {
    static let sourceName = L("Mac 桌面")

    /// Entries waiting for the core.
    private(set) var pending = 0
    /// The last delivery problem, while entries wait.
    private(set) var deliveryProblem: String?

    @ObservationIgnored let outbox: DesktopOutbox
    @ObservationIgnored let preferencesFile: DesktopPreferencesFile
    @ObservationIgnored private weak var core: (any DesktopCoreAccess)?
    @ObservationIgnored private var sender: Task<Void, Never>?
    @ObservationIgnored private var sourceID: String?
    @ObservationIgnored private var imagePaths: [String: URL] = [:]
    @ObservationIgnored private var evidencePaths: [String: URL] = [:]
    /// Called after the core took a batch, so the timeline can reload.
    @ObservationIgnored var onDelivered: (() -> Void)?
    @ObservationIgnored var retryDelay: (Int) -> Duration = { .milliseconds(min(30_000, 250 * (1 << min($0, 7)))) }

    init(directory: URL, core: any DesktopCoreAccess, limits: DesktopOutbox.Limits = .init()) throws {
        outbox = try DesktopOutbox(directory: directory.appendingPathComponent("outbox", isDirectory: true), limits: limits)
        preferencesFile = DesktopPreferencesFile(directory: directory)
        self.core = core
    }

    // MARK: Preferences

    /// The saved settings; on the first launch after the ledger, the ones the ledger kept (asked from the core,
    /// which opens the ledger read-only), waiting up to `wait` for the core to answer.
    func loadPreferences(wait: Duration = .seconds(5)) async throws -> DesktopPreferences {
        if preferencesFile.exists { return try preferencesFile.load() }
        let deadline = ContinuousClock.now.advanced(by: wait)
        while ContinuousClock.now < deadline {
            if let channel = core?.channel, (try? await channel.health(timeout: 1)) != nil {
                if let data = try? await channel.legacyDesktopPreferences(), let legacy = try? DesktopPreferencesFile.decodeLegacy(data) {
                    try preferencesFile.save(legacy)
                    return legacy
                }
                break
            }
            try? await Task.sleep(for: .milliseconds(100))
        }
        return DesktopPreferences()
    }

    func savePreferences(_ preferences: DesktopPreferences) throws { try preferencesFile.save(preferences) }

    // MARK: Writes

    /// Saves a record (durable once this returns); with a gate, only while `epoch` is current.
    func append(_ record: DesktopRecord, gate: CaptureCommitGate? = nil, epoch: UInt64 = 0) async throws {
        try DesktopEventMapping.validate(record)
        guard record.kind != .screenshot else { throw DesktopStoreError.invalidRecord }
        let (event, files) = try DesktopEventMapping.event(for: record)
        try await outbox.append(.ingest([event]), files: files.mapValues { ($0, "json") }, gate: gate, epoch: epoch)
        await queued()
    }

    /// Saves a screenshot with its original (WebP, or JPEG/PNG); returns the record with its image hash.
    func storeScreenshot(record original: DesktopRecord, data: Data, gate: CaptureCommitGate? = nil, epoch: UInt64 = 0) async throws -> DesktopRecord {
        guard !data.isEmpty, data.count <= DesktopEventMapping.maximumImageBytes else { throw DesktopStoreError.imageTooLarge }
        guard let format = DesktopImageFormat(data) else { throw DesktopStoreError.invalidRecord }
        var record = original
        record.imageHash = DesktopEventMapping.sha256(data); record.kind = .screenshot
        try DesktopEventMapping.validate(record)
        let (event, evidence) = try DesktopEventMapping.event(for: record, imageFormat: format)
        var files = evidence.mapValues { (data: $0, ext: "json") }
        files[record.imageHash!] = (data, format.fileExtension)
        try await outbox.append(.ingest([event]), files: files, gate: gate, epoch: epoch)
        await queued()
        return record
    }

    /// Deletes an activity: records of it still waiting here are dropped, the rest is deleted in the core.
    func deleteActivity(_ anchor: DesktopRecord) async throws {
        if let activity = anchor.activityID { await outbox.dropActivity(runID: anchor.runID, activityID: activity) }
        try await outbox.append(.deleteActivity(anchor.id))
        await queued()
    }

    /// Deletes desktop records older than `cutoff` (retention), in order with everything queued before.
    func deleteBefore(_ cutoff: Date) async throws {
        try await outbox.append(.retention(beforeMs: DesktopEventMapping.milliseconds(cutoff)))
        await queued()
    }

    private func queued() async {
        pending = await outbox.count
        startSender()
    }

    // MARK: Sender

    /// Starts delivering (idempotent). Call again when the core becomes ready.
    func startSender() {
        guard sender == nil else { return }
        sender = Task { [weak self] in
            var failures = 0
            while !Task.isCancelled, let self {
                let entries = await self.outbox.peek(64)
                if entries.isEmpty {
                    self.pending = 0; self.deliveryProblem = nil
                    self.sender = nil
                    return
                }
                do {
                    let (delivered, note) = try await self.deliver(entries)
                    await self.outbox.remove(delivered)
                    self.pending = await self.outbox.count
                    self.deliveryProblem = note
                    failures = 0
                    self.onDelivered?()
                } catch {
                    failures += 1
                    self.deliveryProblem = Self.describe(error)
                    try? await Task.sleep(for: self.retryDelay(failures))
                }
            }
        }
    }

    func stopSender() { sender?.cancel(); sender = nil }

    /// The core (re)started, finished the cutover or reported new data: deliver what waits and refresh.
    func coreChanged() {
        startSender()
        onDelivered?()
    }

    private static func describe(_ error: Error) -> String {
        if case CoreError.service(_, "cutover_pending", _) = error { return "等待时间线库完成切换。" }
        return error.localizedDescription
    }

    private func deliver(_ entries: [DesktopOutbox.Entry]) async throws -> ([UInt64], String?) {
        guard let core, let channel = core.channel else { throw DesktopStoreError.unavailable(L("核心进程尚未就绪")) }
        guard core.captureDirect else { throw CoreError.service(status: 409, code: "cutover_pending", message: "") }
        let source = try await desktopSource(channel)
        return try await DesktopDelivery.deliver(entries, channel: channel, sourceID: source)
    }

    private func desktopSource(_ channel: LocalChannel) async throws -> String {
        if let sourceID { return sourceID }
        let id = try await DesktopDelivery.registerSource(channel)
        sourceID = id
        return id
    }

    // MARK: Reads

    private func channel() throws -> LocalChannel {
        guard let channel = core?.channel else { throw DesktopStoreError.unavailable(L("核心进程尚未就绪")) }
        return channel
    }

    private func remember(_ a: AppActivity) {
        for r in [a.anchor, a.latest] + a.observations + a.screenshots { remember(r) }
    }

    private func remember(_ r: AppRecord) {
        if let hash = r.image_hash, let path = r.image_path { imagePaths[hash] = URL(fileURLWithPath: path) }
        if let path = r.ax_evidence_path { evidencePaths[r.id] = URL(fileURLWithPath: path) }
        if imagePaths.count > 4096 { imagePaths.removeAll() }
        if evidencePaths.count > 4096 { evidencePaths.removeAll() }
    }

    /// Activities that overlap `[start, end)`, newest first, with the library's counts. The core answers at most
    /// `limit` per call (newest first); a busy day is read in further pages ending where the previous one did, so its
    /// morning is not cut off. `maxPages` bounds the work for a runaway day.
    func activities(from start: Date, to end: Date, query: String, limit: Int = 1000, maxPages: Int = 8) async throws -> ([DesktopTimelineEvent], DesktopStatistics) {
        var arguments: [String: CoreJSON] = ["start_ms": .int(DesktopEventMapping.milliseconds(start)), "end_ms": .int(DesktopEventMapping.milliseconds(end)), "limit": .int(Int64(limit))]
        if !query.isEmpty { arguments["q"] = .string(query) }
        var answer: AppActivities = try await channel().app("app_activities", arguments)
        var collected = answer.activities, seen = Set(collected.map(\.anchor.id)), pages = 1
        while answer.activities.count >= limit, pages < maxPages, let oldestEnd = answer.activities.map(\.end_ms).min() {
            // Activities not returned yet all end at or before the oldest end returned, so they start before it too.
            arguments["end_ms"] = .int(oldestEnd + 1)
            answer = try await channel().app("app_activities", arguments)
            let fresh = answer.activities.filter { seen.insert($0.anchor.id).inserted }
            pages += 1
            if fresh.isEmpty { break }
            collected += fresh
        }
        collected.forEach(remember)
        // Mapping a whole day is real work; it runs off the main thread so the interface keeps answering.
        let activities = collected
        let events = await Task.detached(priority: .userInitiated) { activities.map(DesktopEventMapping.event) }.value
        return (events, Self.statistics(answer.stats))
    }

    /// How much was recorded on each of `days` (the App's local days), keyed by day start. Days without records
    /// are absent. One aggregate query in the core (`app_day_counts`).
    func dayAmounts(_ days: [DateInterval]) async throws -> [Date: DayAmount] {
        struct Answer: Decodable { struct Day: Decodable { var start_ms: Int64; var screen_minutes: Int; var media: Int; var ai: Int }; var days: [Day] }
        guard !days.isEmpty else { return [:] }
        let ranges: [CoreJSON] = days.map { .array([.int(DesktopEventMapping.milliseconds($0.start)), .int(DesktopEventMapping.milliseconds($0.end))]) }
        let answer: Answer = try await channel().app("app_day_counts", ["days": .array(ranges)])
        let starts = Dictionary(days.map { (DesktopEventMapping.milliseconds($0.start), $0.start) }, uniquingKeysWith: { a, _ in a })
        var result: [Date: DayAmount] = [:]
        for day in answer.days {
            guard let start = starts[day.start_ms] else { continue }
            result[start] = DayAmount(screenMinutes: day.screen_minutes, media: day.media, ai: day.ai)
        }
        return result
    }

    /// The library's desktop counts alone (an empty window of the activity read).
    func statistics() async throws -> DesktopStatistics {
        let answer: AppActivities = try await channel().app("app_activities", ["start_ms": .int(0), "end_ms": .int(1), "limit": .int(1)])
        return Self.statistics(answer.stats)
    }

    static func statistics(_ s: AppStats) -> DesktopStatistics {
        DesktopStatistics(recordCount: s.record_count, screenshotCount: s.screenshot_count, storageBytes: s.storage_bytes,
                          freedBytes: s.freed_bytes ?? 0, freedScreenshots: s.freed_screenshots ?? 0)
    }

    /// Over the storage limit: the oldest screenshots (over 7 days old) keep only their text until the library's
    /// originals use at most `targetBytes`.
    func freeScreenshotSpace(targetBytes: Int64) async throws -> ScreenshotSpaceReport {
        try await channel().freeScreenshotSpace(targetBytes: max(0, targetBytes))
    }

    /// One activity with a page of observations (`before`/`after`/`around` an observation, or the newest page).
    func activity(id: String, before: String? = nil, after: String? = nil, around: String? = nil, limit: Int = 120) async throws -> DesktopTimelineEvent? {
        var arguments: [String: CoreJSON] = ["id": .string(id), "limit": .int(Int64(limit))]
        if let before { arguments["before"] = .string(before) }
        if let after { arguments["after"] = .string(after) }
        if let around { arguments["around"] = .string(around) }
        struct Answer: Decodable { var activity: AppActivity? }
        let answer: Answer = try await channel().app("app_activity", arguments)
        guard let activity = answer.activity else { return nil }
        remember(activity)
        return DesktopEventMapping.event(activity)
    }

    /// The original screenshot's file, if the core has it.
    func imageURL(hash: String) async -> URL? {
        if let url = imagePaths[hash], FileManager.default.fileExists(atPath: url.path) { return url }
        struct Answer: Decodable { var paths: [String: String] }
        guard let answer: Answer = try? await channel().app("app_media", ["sha256": .array([.string(hash)])]),
              let path = answer.paths[hash] else { return nil }
        let url = URL(fileURLWithPath: path)
        imagePaths[hash] = url
        return url
    }

    func retentionCount(before cutoff: Date) async throws -> Int {
        struct Answer: Decodable { var records: Int }
        let answer: Answer = try await channel().app("app_retention_count", ["before_ms": .int(DesktopEventMapping.milliseconds(cutoff))])
        return answer.records
    }

    // MARK: Recognition

    private static func method(_ kind: DesktopRecognitionKind) -> String { kind == .ocr ? "ocr" : "vlm" }

    private static func result(_ r: AppRecognition) -> DesktopRecognitionResult {
        if let path = r.result_path, let data = DesktopEventMapping.derivedJSON(at: URL(fileURLWithPath: path)),
           let full = try? JSONDecoder().decode(DesktopRecognitionResult.self, from: data) { return full }
        return DesktopRecognitionResult(source: r.model, text: r.text, completedAt: Date(timeIntervalSince1970: Double(r.completed_ms) / 1000))
    }

    func recognitionResults(for id: String) async throws -> [DesktopRecognitionKind: DesktopRecognitionResult] {
        let info: AppRecognitionInfo = try await channel().app("app_recognition", ["screenshot_id": .string(id)])
        var out: [DesktopRecognitionKind: DesktopRecognitionResult] = [:]
        for r in info.results { out[r.method == "ocr" ? .ocr : .vision] = Self.result(r) }
        return out
    }

    func pendingRecognition(_ kind: DesktopRecognitionKind, since: Date, skip: Set<String>) async throws -> DesktopRecord? {
        struct Answer: Decodable { var screenshot: AppRecord? }
        let since = since == .distantPast ? 0 : DesktopEventMapping.milliseconds(since)
        let answer: Answer = try await channel().app("app_pending_recognition", ["method": .string(Self.method(kind)), "since_ms": .int(since),
                                                                                   "skip": .array(skip.sorted().map(CoreJSON.string))])
        guard let shot = answer.screenshot else { return nil }
        remember(shot)
        return DesktopEventMapping.record(shot)
    }

    func existingOCR(forImageHash hash: String) async throws -> DesktopRecognitionResult? {
        struct Answer: Decodable { var result: AppRecognition? }
        let answer: Answer = try await channel().app("app_ocr", ["sha256": .string(hash)])
        return answer.result.map(Self.result)
    }

    /// The screenshot's JPEG and AX evidence for a recognizer; fails with `missingImage` when it is gone or differs.
    func recognitionInput(_ screenshot: DesktopRecord) async throws -> (jpeg: Data, evidence: DesktopAXEvidence?) {
        let info: AppRecognitionInfo
        do { info = try await channel().app("app_recognition", ["screenshot_id": .string(screenshot.id)]) }
        catch CoreError.service(404, _, _) { throw DesktopStoreError.missingImage }
        guard let hash = info.image_hash, hash == screenshot.imageHash, let path = info.image_path,
              let jpeg = try? Data(contentsOf: URL(fileURLWithPath: path)), DesktopEventMapping.sha256(jpeg) == hash else {
            throw DesktopStoreError.missingImage
        }
        let evidence = info.ax_evidence_path.flatMap { DesktopEventMapping.derivedJSON(at: URL(fileURLWithPath: $0)) }
            .flatMap { try? JSONDecoder().decode(DesktopAXEvidence.self, from: $0) }
        return (jpeg, evidence)
    }

    /// Writes a recognition run for a screenshot (its full result kept as a derived file) and supersedes the earlier
    /// runs of the same kind. Recognition can be redone, so it goes straight to the core instead of the outbox.
    func saveRecognition(_ result: DesktopRecognitionResult, kind: DesktopRecognitionKind, screenshot: DesktopRecord) async throws {
        try Task.checkCancellation()
        let channel = try channel()
        let info: AppRecognitionInfo
        do { info = try await channel.app("app_recognition", ["screenshot_id": .string(screenshot.id)]) }
        catch CoreError.service(404, _, _) { throw DesktopStoreError.deletedRecord }
        guard let hash = info.image_hash, hash == screenshot.imageHash else { throw DesktopStoreError.deletedRecord }
        let encoder = JSONEncoder(); encoder.outputFormatting = [.sortedKeys]
        let data = try encoder.encode(result)
        guard data.count <= 256 * 1024 else { throw DesktopStoreError.invalidRecord }
        let sha = DesktopEventMapping.sha256(data)
        let staging = URL(fileURLWithPath: try await channel.health().staging_dir, isDirectory: true)
        let file = staging.appendingPathComponent("app-\(UUID().uuidString).json")
        try DesktopOutbox.writeDurably(data, to: file)
        defer { try? FileManager.default.removeItem(at: file) }
        let completed = DesktopEventMapping.milliseconds(result.completedAt)
        let method = Self.method(kind)
        var event = IngestEvent(source_key: "recognition:\(screenshot.id):\(kind.rawValue)@\(completed)", kind: "image.text",
                                start_ms: DesktopEventMapping.milliseconds(screenshot.timestamp))
        event.app_bundle_id = screenshot.bundleID.isEmpty ? nil : screenshot.bundleID
        event.app_name = screenshot.appName.isEmpty ? nil : screenshot.appName
        event.title = screenshot.windowTitle.isEmpty ? nil : screenshot.windowTitle
        event.parent_key = screenshot.id
        event.payload = ["image_event_id": .string(info.screenshot_event_id), "method": .string(method), "model_id": .string(result.source),
                         "completed_ms": .int(completed), "result_ref": .string(sha)]
        if !result.text.isEmpty { event.text = [IngestText(field: method, content: result.text)] }
        event.blobs = [IngestBlob(staging_path: file.path, sha256: sha, role: "derived", mime_type: "application/json")]
        let source = try await desktopSource(channel)
        _ = try await channel.ingest(IngestBatch(source_id: source, events: [event]))
        let previous = (info.results.filter { $0.method == method && $0.source_key != event.source_key }.map(\.event_id) + info.older)
        if !previous.isEmpty { _ = try await channel.deleteDesktop(eventIDs: previous, reason: "superseded") }
    }
}
