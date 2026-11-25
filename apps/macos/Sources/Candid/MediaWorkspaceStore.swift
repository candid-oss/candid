import Foundation
import CandidLocalization
import Observation
import AVFoundation
import CryptoKit

@MainActor @Observable
final class MediaWorkspaceStore {
    var query = ""
    var selectedRecordingID: UUID?
    var selectedID: UUID?
    private(set) var records: [MediaLibraryRecord] = []
    private(set) var draft: MediaDraft?
    private(set) var connectionError: String?
    var error: String?
    private(set) var busy = false
    private(set) var recordingStartedAt: Date? {
        didSet { followRecording() }
    }
    /// Recent input loudness while recording (0…1, oldest first), sampled
    /// ~12×/s. Only in-window views read it; never the menu bar label.
    private(set) var recordingLevels: [Float] = []
    /// True after a few seconds without audible input, so the user knows
    /// nothing is being picked up before they finish a long recording.
    private(set) var microphoneQuiet = false
    static let levelHistory = 28
    /// Elapsed recording time for the menu bar, refreshed once a second. The
    /// menu bar label must not host Text(_:style: .timer) or a TimelineView:
    /// on macOS 27 every frame they redraw leaks the rendered label (measured
    /// ~200 MB/s), so it gets a plain string instead.
    private(set) var recordingClock: String?
    @ObservationIgnored private var clockTask: Task<Void, Never>?
    private var selectedDetail: MediaLibraryRecord?

    let profile: URL
    let capture = MediaCapture()
    var recordingInput: RecordingInput = .microphone { didSet { saveAudioPreferences() } }
    private(set) var activeRecordingInput: RecordingInput = .microphone
    var callRecordingReminders: Bool = true { didSet { saveAudioPreferences() } }
    var autoStopCallRecording = true { didSet { saveAudioPreferences() } }
    private(set) var callRecordingTarget: CallAudioProcess?
    /// Parts are written progressively (fragmented AAC), so their length only bounds one upload and one quick
    /// transcription; ten minutes matches the whole-recording speaker windows. The user never sees parts.
    var recordingSegmentSeconds = 600
    private(set) var savedRecordingSegments = 0
    var recordingFinishing = false
    var recordingNotice: String?
    var recordingWarning: String?
    var recordingInventory = RecordingInventory()
    var recordingSyncing = false
    var recordingScanning = false
    var recordingSyncAgain = false
    var recordingRecoveryLoaded = false
    var recordingJournal: RecordingJournal { RecordingJournal(profile: profile) }
    @ObservationIgnored private var previewTasks: [URL: Task<URL, Error>] = [:]
    private var refreshID = UUID()
    private var detailRequest = UUID()
    private var draftManifest: URL { profile.appendingPathComponent("pending.json") }
    private var draftDirectory: URL { profile.appendingPathComponent("Pending", isDirectory: true) }

    /// `--mock` only: synthetic records, nothing is read from or sent to a library.
    let isDemo: Bool

    init(profile: URL, demoRecords: [MediaLibraryRecord]? = nil) {
        self.profile = profile
        isDemo = demoRecords != nil
        if let demoRecords { records = demoRecords; return }
        if let data = try? Data(contentsOf: profile.appendingPathComponent("audio-preferences.json")),
           let preferences = try? JSONDecoder().decode(AudioPreferences.self, from: data) {
            recordingInput = preferences.input; callRecordingReminders = preferences.reminders
            autoStopCallRecording = preferences.autoStop ?? true
        }
        if let data = try? Data(contentsOf: draftManifest),
           let saved = try? JSONDecoder().decode(MediaDraft.self, from: data),
           saved.storedName == (saved.storedName as NSString).lastPathComponent,
           FileManager.default.fileExists(atPath: file(for: saved).path) {
            draft = saved
        } else if FileManager.default.fileExists(atPath: draftManifest.path) {
            error = L("待入库素材暂时无法读取。原件保留在资料库的 Pending 文件夹。")
        }
    }

    private struct AudioPreferences: Codable { let input: RecordingInput; let reminders: Bool; var autoStop: Bool? }
    private func saveAudioPreferences() {
        guard !isDemo else { return }
        do {
            try FileManager.default.createDirectory(at: profile, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            try JSONEncoder().encode(AudioPreferences(input: recordingInput, reminders: callRecordingReminders, autoStop: autoStopCallRecording))
                .write(to: profile.appendingPathComponent("audio-preferences.json"), options: .atomic)
        } catch { self.error = L("录音设置未保存。") + error.localizedDescription }
    }

    static func defaultProfile() -> URL {
        URL.applicationSupportDirectory.appending(path: "Candid/MediaWorkspace", directoryHint: .isDirectory)
    }

    var hasActiveJobs: Bool { records.contains { $0.processingStatus.isActive || $0.speakerProcessing?.isActive == true || $0.processing?.isActive == true || $0.aiProcessing?.values.contains(where: \.isActive) == true } }
    var selectedDetailKey: String? {
        guard let row = records.first(where: { $0.id == selectedID }) else { return nil }
        return "\(row.id):\(row.processingStatus.rawValue):\(row.processing?.revisionKey ?? ""):\(row.aiProcessing?.map { $0.key + $0.value.revisionKey }.sorted().joined() ?? ""):\(row.processingMode ?? ""):\(row.contentRevision ?? ""):\(row.title ?? ""):\(row.notes?.hashValue ?? 0)"
    }
    var hasSelectedDetails: Bool {
        guard let row = records.first(where: { $0.id == selectedID }) else { return false }
        return selectedDetail?.id == row.id && selectedDetail?.processingStatus == row.processingStatus
            && selectedDetail?.processing == row.processing && selectedDetail?.aiProcessing == row.aiProcessing
            && selectedDetail?.processingMode == row.processingMode && selectedDetail?.contentRevision == row.contentRevision && selectedDetail?.title == row.title && selectedDetail?.notes == row.notes
    }
    var selected: MediaLibraryRecord? {
        guard let row = records.first(where: { $0.id == selectedID }) else { return nil }
        return hasSelectedDetails ? selectedDetail : row
    }
    var isRecording: Bool { recordingStartedAt != nil }

    nonisolated static func clockText(elapsed: TimeInterval) -> String {
        let seconds = max(0, Int(elapsed))
        let h = seconds / 3600, m = seconds / 60 % 60, s = seconds % 60
        return h > 0 ? String(format: "%d:%02d:%02d", h, m, s) : String(format: "%d:%02d", m, s)
    }

    private func followRecording() {
        clockTask?.cancel(); clockTask = nil
        guard let started = recordingStartedAt else {
            recordingClock = nil; recordingLevels = []; microphoneQuiet = false
            return
        }
        recordingClock = Self.clockText(elapsed: Date().timeIntervalSince(started))
        recordingLevels = Array(repeating: 0, count: Self.levelHistory)
        microphoneQuiet = false
        clockTask = Task { [weak self] in
            var lastHeard = Date()
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(80))
                guard !Task.isCancelled, let self, let started = self.recordingStartedAt else { return }
                if let failure = self.capture.recordingFailure() {
                    Task { await self.stopRecording(failure: failure) }
                    return
                }
                if let status = self.capture.recordingStatus, status.savedSegments != self.savedRecordingSegments {
                    self.savedRecordingSegments = status.savedSegments
                    self.recordingNotice = L("%@ · 自动保存中", self.activeRecordingInput.label)
                    Task { await self.syncRecordings() }
                }
                let now = Date()
                if let level = self.capture.level() {
                    self.recordingLevels = Array((self.recordingLevels + [level]).suffix(Self.levelHistory))
                    if level > Self.audibleLevel { lastHeard = now }
                }
                let quiet = now.timeIntervalSince(lastHeard) > 4
                if self.microphoneQuiet != quiet { self.microphoneQuiet = quiet }
                let text = Self.clockText(elapsed: now.timeIntervalSince(started))
                if self.recordingClock != text { self.recordingClock = text }
            }
        }
    }

    /// Roughly -44 dBFS: above a quiet room, below soft speech.
    nonisolated static let audibleLevel: Float = 0.015
    var canCapture: Bool { !isDemo && !isRecording && !busy && !recordingFinishing && !recordingScanning && draft == nil }
    var draftFile: URL? { draft.map { file(for: $0) } }

    func refresh() async {
        guard !isDemo else { return }
        if !recordingRecoveryLoaded { await syncRecordings() }
        let requestedQuery = query
        let request = UUID()
        refreshID = request
        do {
            let items = try await client().list(query: requestedQuery)
            guard !Task.isCancelled, refreshID == request, query == requestedQuery else { return }
            records = items
            if selected == nil { selectedID = items.first?.id }
            connectionError = nil
        } catch {
            guard !Task.isCancelled, refreshID == request else { return }
            connectionError = error.localizedDescription
        }
    }

    func loadDetails() async {
        guard !isDemo, let id = selectedID, !hasSelectedDetails else { return }
        let request = UUID()
        detailRequest = request
        do {
            let item = try await client().item(id)
            guard !Task.isCancelled, detailRequest == request, selectedID == id else { return }
            replace(item)
        } catch {
            guard !Task.isCancelled, detailRequest == request, selectedID == id else { return }
            self.error = error.localizedDescription
        }
    }

    func importFile(_ url: URL) async {
        guard canCapture else { error = MediaWorkspaceError.busyDraft.localizedDescription; return }
        busy = true; error = nil
        defer { busy = false }
        do {
            try await stage(url, source: .import, occurredAt: nil, needsReview: false)
            try await submitDraft()
        } catch { self.error = error.localizedDescription }
    }

    func startRecording(input: RecordingInput? = nil, call: CallAudioProcess? = nil) async {
        guard canCapture else { return }
        busy = true; error = nil
        defer { busy = false }
        do {
            let chosen = input ?? recordingInput
            let started = try await capture.startRecording(journal: recordingJournal, seconds: recordingSegmentSeconds, input: chosen)
            activeRecordingInput = chosen
            var inferredCall: CallAudioProcess?
            if #available(macOS 15, *), call == nil, chosen == .systemAndMicrophone, autoStopCallRecording,
               let processes = await Task.detached(priority: .utility, operation: { CallAudioActivity.read() }).value,
               Set(processes.map(\.family)).count == 1 { inferredCall = processes.first }
            var associatedCall = call ?? inferredCall
            if #available(macOS 15, *), let associated = associatedCall { associatedCall?.callWindowID = CallAudioActivity.callWindow(for: associated) }
            callRecordingTarget = associatedCall
            savedRecordingSegments = 0
            recordingNotice = L("%@ · 自动保存中 · 点按停止", activeRecordingInput.label)
            recordingStartedAt = started
        } catch { self.error = error.localizedDescription }
    }

    func stopRecording(failure: String? = nil, automatic: Bool = false) async {
        guard isRecording, !recordingFinishing else { return }
        // Clear the capture indicator immediately; IO completion has its own status.
        recordingFinishing = true; recordingStartedAt = nil; callRecordingTarget = nil
        recordingNotice = L("音频采集已停止 · 正在保存最后一段")
        do {
            try await capture.stopRecording(reason: failure == nil ? "stopped" : "input_fault")
            recordingNotice = automatic ? L("通话已结束 · 录音已自动保存") : L("录音已停止 · 已保存")
            if let failure { error = failure }
        } catch {
            self.error = L("录音已停止。已封存分段保留，最后一段可能未保存。") + error.localizedDescription
            recordingNotice = L("录音已停止 · 请检查保存结果")
        }
        recordingFinishing = false
        await syncRecordings()
        await refresh()
    }

    func captureRegion() async {
        guard canCapture else { return }
        busy = true; error = nil
        defer { busy = false }
        do {
            guard let file = try await capture.captureRegion() else { return }
            try await stage(file, source: .macRegion, occurredAt: Date(), needsReview: true)
            try? FileManager.default.removeItem(at: file)
        } catch { self.error = error.localizedDescription }
    }

    func saveDraft() async {
        guard !busy, draft != nil else { return }
        busy = true; error = nil
        defer { busy = false }
        do { try await submitDraft() }
        catch { self.error = error.localizedDescription }
    }

    func discardDraft() {
        guard !busy, let draft else { return }
        do { try clear(draft) }
        catch { self.error = error.localizedDescription }
    }

    func edit(_ record: MediaLibraryRecord, title: String, notes: String) async {
        guard !isDemo, !busy else { return }
        busy = true; error = nil
        defer { busy = false }
        do { replace(try await client().edit(record.id, title: title, notes: notes)) }
        catch { self.error = error.localizedDescription }
    }

    func recognize(_ id: UUID, mode: MediaLibraryRecord.Mode) async {
        guard !isDemo else { error = L("合成样例不能重新识别。"); return }
        guard !busy else { return }
        busy = true; error = nil
        defer { busy = false }
        do {
            let updated = try await client().recognize(id, mode: mode)
            replace(updated)
            await refresh()
        } catch { self.error = error.localizedDescription }
    }

    func cancelProcessing(_ record: MediaLibraryRecord) async {
        guard !isDemo else { return }
        do { replace(try await client().cancelProcessing(record.id)); await refresh() }
        catch { self.error = error.localizedDescription }
    }

    /// 「现在处理」: every waiting job of this recording (and its parts) runs once, past the power gate only.
    func runNow(_ record: MediaLibraryRecord) async {
        guard !isDemo, let channel = TimelineService.shared.channel else { return }
        do {
            _ = try await channel.send("POST", "/local/jobs", body: JSONSerialization.data(withJSONObject: ["action": "run_now", "input_ref": record.id.uuidString.lowercased()])).checked()
            await refresh()
        } catch { self.error = error.localizedDescription }
    }

    func runAI(_ record: MediaLibraryRecord, step: String) async {
        guard !isDemo, let channel = TimelineService.shared.channel else { return }
        do {
            try await AIRuntime.shared.syncCore()
            _ = try await channel.send("POST", "/local/workflows", body: JSONSerialization.data(withJSONObject: ["action":"enqueue", "id":record.id.uuidString.lowercased(), "step":step])).checked()
            await refresh()
        } catch { self.error = error.localizedDescription }
    }

    func delete(_ record: MediaLibraryRecord) async {
        guard !isDemo else { error = L("合成样例不能删除。"); return }
        guard !busy else { return }
        busy = true; error = nil
        defer { busy = false }
        do {
            try await client().delete(record.id)
            records.removeAll { $0.id == record.id }
            await refresh()
        } catch { self.error = error.localizedDescription }
    }

    func preview(_ record: MediaLibraryRecord, directory: URL) async throws -> URL {
        // Ask the core every time so deleted/revoked originals cannot be served by a stale preview.
        let data = try await client().content(record.id)
        guard let value = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw MediaWorkspaceError.malformed }
        if let path = value["path"] as? String { return URL(fileURLWithPath: path) }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let url = directory.appendingPathComponent(record.id.uuidString + ".playlist.json")
        try data.write(to: url, options: .atomic)
        return url
    }

    private func client() throws -> MediaLibraryClient { try MediaLibraryClient(profile: profile) }
    private func file(for draft: MediaDraft) -> URL { draftDirectory.appendingPathComponent(draft.storedName) }

    private func submitDraft() async throws {
        guard let draft else { return }
        let item = try await client().upload(draft, file: file(for: draft))
        try clear(draft)
        query = ""
        replace(item)
        selectedID = item.id
        await refresh()
    }

    /// People offered by 「这是谁？」 and the speaker being saved right now (labelling can take a while: the voice
    /// service analyses a short window to keep samples).
    private(set) var speakerPeople: [SpeakerPerson] = []
    private(set) var labelingSpeaker: String?
    private(set) var speakerNote: String?

    func loadSpeakerPeople() async {
        guard !isDemo else { return }
        if let people = try? await client().speakerPeople() { speakerPeople = people }
    }

    func label(_ record: MediaLibraryRecord, speaker: String, as choice: SpeakerChoice) async {
        guard !isDemo else { error = L("合成样例不能标注说话人。"); return }
        guard labelingSpeaker == nil else { return }
        labelingSpeaker = speaker; speakerNote = nil; error = nil
        defer { labelingSpeaker = nil }
        do {
            let (updated, note) = try await client().labelSpeaker(record.id, speaker: speaker, as: choice)
            replace(updated)
            if let note { speakerNote = L("名字已记下；声纹样本这次没存上（%@），下次确认时会再试。", note) }
            await loadSpeakerPeople()
        } catch { self.error = error.localizedDescription }
    }

    func replace(_ record: MediaLibraryRecord) {
        if record.id == selectedID { selectedDetail = record }
        if let index = records.firstIndex(where: { $0.id == record.id }) { records[index] = record }
        else { records.insert(record, at: 0) }
    }

    private func clear(_ draft: MediaDraft) throws {
        try FileManager.default.removeItem(at: draftManifest)
        self.draft = nil
        try? FileManager.default.removeItem(at: file(for: draft))
    }

    private func stage(_ url: URL, source: MediaLibraryRecord.Source, occurredAt: Date?, needsReview: Bool) async throws {
        let suffix = url.pathExtension.lowercased()
        let kind: String
        if ["wav", "m4a", "mp4", "mp3", "aac", "flac", "ogg", "opus"].contains(suffix) { kind = "audio" }
        else if ["png", "jpg", "jpeg", "webp"].contains(suffix) { kind = "image" }
        else { throw MediaWorkspaceError.unsupported }
        let id = UUID()
        let filename: String
        switch source {
        case .import: filename = url.lastPathComponent
        case .macMicrophone, .macSystemAudio, .macRegion:
            let formatter = DateFormatter()
            formatter.locale = Locale(identifier: "en_US_POSIX")
            formatter.dateFormat = "yyyy-MM-dd HH-mm-ss"
            let title = source == .macRegion ? L("框选截图") : L("Mac 录音")
            filename = title + " " + formatter.string(from: occurredAt ?? Date()) + "." + suffix
        }
        let item = MediaDraft(id: id, filename: filename, storedName: id.uuidString + "." + suffix,
                             kind: kind, source: source.rawValue, occurredAt: occurredAt, needsReview: needsReview)
        let destination = file(for: item)
        let directory = draftDirectory
        let manifest = draftManifest
        let persisted = try await Task.detached(priority: .userInitiated) {
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            let values = try url.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey])
            guard values.isRegularFile == true, let size = values.fileSize, size > 0 else {
                throw MediaWorkspaceError.unsupported
            }
            guard size <= (kind == "audio" ? 512 : 64) * 1024 * 1024 else { throw MediaWorkspaceError.tooLarge }
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true,
                                                    attributes: [.posixPermissions: 0o700])
            try FileManager.default.copyItem(at: url, to: destination)
            do {
                var capturedAt = item.occurredAt
                if capturedAt == nil, item.kind == "audio",
                   let tag = try? await AVURLAsset(url: destination).load(.creationDate),
                   let date = try? await tag.load(.dateValue), date <= Date() {
                    capturedAt = date
                }
                let saved = MediaDraft(id: item.id, filename: item.filename, storedName: item.storedName,
                                       kind: item.kind, source: item.source, occurredAt: capturedAt,
                                       needsReview: item.needsReview)
                try JSONEncoder().encode(saved).write(to: manifest, options: .atomic)
                return saved
            } catch {
                try? FileManager.default.removeItem(at: destination)
                throw error
            }
        }.value
        draft = persisted
    }
}
