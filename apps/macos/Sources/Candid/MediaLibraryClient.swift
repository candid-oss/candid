import Foundation
import CandidLocalization
import CandidShared
import CandidClient

struct MediaLibraryRecord: Decodable, Identifiable {
    enum Kind: String, Decodable { case audio, image }
    enum Source: String, Decodable {
        case `import`, macMicrophone = "mac_microphone", macSystemAudio = "mac_system_audio", macRegion = "mac_region"

        var label: String {
            switch self {
            case .import: L("文件导入")
            case .macMicrophone: L("Mac 麦克风")
            case .macSystemAudio: L("系统声音与麦克风")
            case .macRegion: L("Mac 框选截图")
            }
        }
    }
    enum Processing: String, Decodable {
        case idle, queued, processing, ready, failed
        var label: String {
            switch self {
            case .idle: L("未识别")
            case .queued: L("等待识别")
            case .processing: L("正在识别")
            case .ready: L("已识别")
            case .failed: L("识别失败")
            }
        }
        var isActive: Bool { self == .queued || self == .processing }
    }
    enum Mode: String, CaseIterable, Identifiable {
        case quick, conversation
        var id: Self { self }
        var label: String { self == .quick ? L("单人转写") : L("多人分段与声纹") }
    }

    struct Segment: Decodable {
        let startSeconds: Double
        let endSeconds: Double
        let text: String
        let speaker: String?
        let identity: String?
        /// The person shown for this line: recognised automatically or named by you; nil while anonymous.
        var speakerName: String? = nil
        var speakerDisplayLabel: String? = nil
        var shownSpeaker: String? { speakerName ?? speakerDisplayLabel ?? speaker }
    }

    /// One speaker of a recording (agent-platform-v1 §2.6): who it is, how sure the match is, how much they said.
    struct Speaker: Decodable, Identifiable, Equatable {
        enum Status: String, Decodable { case auto, maybe, confirmed, anonymous }
        let key: String
        var matchReason: String? = nil
        var label: String? = nil
        let status: Status
        let name: String?
        let personId: String?
        var voiceId: String? = nil
        let score: Double?
        var margin: Double? = nil
        var threshold: Double? = nil
        let segments: Int
        let seconds: Double
        var samplesSaved: Bool? = nil
        var dismissed: Bool? = nil
        var id: String { key }
        /// Cosine similarity shown as a percentage; it is not a probability.
        var percent: Int? { score.map { Int((max(0, min(1, $0)) * 100).rounded()) } }
    }

    struct RecordingPart: Decodable {
        let id: UUID
        let index: Int
        let sampleStart: Int64
        let sampleCount: Int64
        let sampleRate: Int
    }
    struct Gap: Decodable {
        let sampleStart: Int64
        let sampleCount: Int64
        let sampleRate: Int
        var startSeconds: Double { Double(sampleStart) / Double(sampleRate) }
        var endSeconds: Double { Double(sampleStart + sampleCount) / Double(sampleRate) }
    }
    var processing: RecordingProcessing? = nil
    var aiProcessing: [String: RecordingProcessing]? = nil
    struct AIDraft: Decodable { let text: String }
    var aiDrafts: [String: AIDraft]? = nil
    var title: String? = nil
    var notes: String? = nil
    var speakers: [Speaker]? = nil
    var speakerPolicy: RecordingSpeakerPolicy? = nil
    struct SpeakerGrouping: Decodable { let speakerCount: Int; let unresolvedSegments: Int }
    var speakerGrouping: SpeakerGrouping? = nil
    var speakerProcessing: Processing? = nil
    var recordingParts: [RecordingPart]? = nil
    var gaps: [Gap]? = nil
    var recordingState: String? = nil
    var durationSeconds: Double? = nil
    var contentRevision: String? = nil
    var displayTitle: String { title.flatMap { $0.isEmpty ? nil : $0 } ?? filename }
    let id: UUID
    let filename: String
    let kind: Kind
    let source: Source
    let occurredAt: Date?
    let importedAt: Date
    let processingStatus: Processing
    let segments: [Segment]
    let error: String?
    let processingMode: String?
    var displayedDate: Date { occurredAt ?? importedAt }
}

struct SpeakerPerson: Decodable, Identifiable, Equatable {
    let personId: String
    let name: String
    var voiceId: String? = nil
    var id: String { personId }
}

enum SpeakerChoice: Equatable {
    case person(SpeakerPerson), voice(String, name: String), new(String), anonymous
}

struct MediaDraft: Codable, Identifiable {
    let id: UUID
    let filename: String
    let storedName: String
    let kind: String
    let source: String
    let occurredAt: Date?
    let needsReview: Bool
    var recording: RecordingUpload? = nil
}

struct RecordingUpload: Codable {
    let recordingID: UUID
    let segmentIndex: Int
    let sampleStart: Int64
    let sampleCount: Int64
    let sampleRate: Int
    let startedAt: Date
}

enum MediaWorkspaceError: LocalizedError {
    case configuration
    case unsupported
    case busyDraft
    case rejected(Int, String?)
    case malformed
    case tooLarge

    var errorDescription: String? {
        switch self {
        case .configuration: L("资料库尚未连接，请连接资料库后重试。")
        case .unsupported: L("请选择支持的录音或图片文件。")
        case .busyDraft: L("请先保存或丢弃当前待入库素材。")
        case .rejected(401, _): L("资料库授权失效，请重新运行启动脚本。")
        case .rejected(413, _), .tooLarge: L("文件超过资料库的大小限制。")
        case .rejected(_, let message): message ?? L("资料库未能完成操作。")
        case .malformed: L("资料库返回了无法读取的内容。")
        }
    }
}

struct MediaLibraryClient {
    private struct Page: Decodable { let items: [MediaLibraryRecord] }
    private let channel: LocalChannel
    init(channel: LocalChannel) { self.channel = channel }
    init(profile: URL) throws {
        channel = LocalChannel(socketPath: RunDirectory.socket(in: RunDirectory.url()).path)
    }
    private func call(_ values: [String: Any]) async throws -> Data {
        let body = try JSONSerialization.data(withJSONObject: values)
        let deadline = ContinuousClock.now.advanced(by: .seconds(15))
        while true {
            try Task.checkCancellation()
            do { return try await channel.send("POST", "/local/media", body: body, timeout: 120).checked() }
            catch CoreError.unreachable where ContinuousClock.now < deadline {
                try await Task.sleep(for: .milliseconds(150))
            }
        }
    }
    func list(query: String) async throws -> [MediaLibraryRecord] {
        try decode(Page.self, from: await call(["action": "list", "q": query, "limit": 100])).items
    }
    func item(_ id: UUID) async throws -> MediaLibraryRecord {
        try decode(MediaLibraryRecord.self, from: await call(["action": "item", "id": id.uuidString.lowercased()]))
    }
    private func part(_ p: RecordingUpload) -> [String: Any] {
        ["recording_id": p.recordingID.uuidString.lowercased(), "segment_index": p.segmentIndex,
         "sample_start": p.sampleStart, "sample_count": p.sampleCount, "sample_rate": p.sampleRate,
         "recording_started_at": p.startedAt.ISO8601Format()]
    }
    func upload(_ draft: MediaDraft, file: URL) async throws -> MediaLibraryRecord {
        var body: [String: Any] = ["action": "import", "id": draft.id.uuidString.lowercased(),
            "filename": draft.filename, "kind": draft.kind, "source": draft.source, "path": file.path]
        if let at = draft.occurredAt { body["occurred_at"] = at.ISO8601Format() }
        if let p = draft.recording { body["recording"] = part(p) }
        return try decode(MediaLibraryRecord.self, from: await call(body))
    }
    func linkRecording(_ draft: MediaDraft) async throws -> MediaLibraryRecord {
        guard let p = draft.recording else { throw MediaWorkspaceError.malformed }
        return try decode(MediaLibraryRecord.self, from: await call(["action": "link", "id": draft.id.uuidString.lowercased(), "recording": part(p)]))
    }
    func edit(_ id: UUID, title: String, notes: String) async throws -> MediaLibraryRecord {
        try decode(MediaLibraryRecord.self, from: await call(["action": "edit", "id": id.uuidString.lowercased(), "title": title, "notes": notes]))
    }
    func recordingWasDeleted(_ id: UUID) async throws -> Bool {
        let data = try await call(["action": "state", "id": id.uuidString.lowercased()])
        return (try JSONSerialization.jsonObject(with: data) as? [String: Any])?["recording_state"] as? String == "deleted"
    }
    func recordingState(_ id: UUID, state: String) async throws -> MediaLibraryRecord {
        try decode(MediaLibraryRecord.self, from: await call(["action": "state", "id": id.uuidString.lowercased(), "state": state]))
    }
    func recognize(_ id: UUID, mode: MediaLibraryRecord.Mode) async throws -> MediaLibraryRecord {
        try decode(MediaLibraryRecord.self, from: await call(["action": "recognize", "id": id.uuidString.lowercased(), "mode": mode.rawValue, "request_id": UUID().uuidString.lowercased()]))
    }
    func cancelProcessing(_ id: UUID) async throws -> MediaLibraryRecord {
        try decode(MediaLibraryRecord.self, from: await call(["action": "cancel_processing", "id": id.uuidString.lowercased()]))
    }
    /// People offered by 「这是谁？」, most recently named first.
    func speakerPeople() async throws -> [SpeakerPerson] {
        struct People: Decodable { let people: [SpeakerPerson] }
        return try decode(People.self, from: await call(["action": "speaker_people"])).people
    }
    /// Names a speaker; the core also saves the speaker's clearest voice as library samples when the voice
    /// service is reachable. Returns the updated recording and, if samples could not be saved, why.
    func labelSpeaker(_ id: UUID, speaker: String, as choice: SpeakerChoice) async throws -> (MediaLibraryRecord, String?) {
        var body: [String: Any] = ["action": "label", "id": id.uuidString.lowercased(), "speaker": speaker]
        switch choice {
        case .anonymous: body["anonymous"] = true
        case let .person(person): body["person_id"] = person.personId; body["name"] = person.name; if let v = person.voiceId { body["voice_id"] = v }
        case let .voice(voiceID, name): body["voice_id"] = voiceID; body["name"] = name
        case let .new(name): body["name"] = name
        }
        let data = try await channel.send("POST", "/local/voiceprints", body: JSONSerialization.data(withJSONObject: body), timeout: 130).checked()
        let note = (try? JSONSerialization.jsonObject(with: data) as? [String: Any])?["samples_error"] as? String
        return (try decode(MediaLibraryRecord.self, from: data), note)
    }
    func delete(_ id: UUID) async throws {
        _ = try await call(["action": "delete", "id": id.uuidString.lowercased()])
    }
    /// The core returns original part paths; playback reads one file at a time without concatenating.
    func content(_ id: UUID) async throws -> Data { try await call(["action": "content", "id": id.uuidString.lowercased()]) }
    func download(_ item: MediaLibraryRecord, to destination: URL) async throws {
        let data = try await content(item.id)
        guard let answer = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw MediaWorkspaceError.malformed }
        if let path = answer["path"] as? String { try FileManager.default.copyItem(at: URL(fileURLWithPath: path), to: destination) }
        else { try data.write(to: destination, options: .atomic) }
    }
    private func decode<T: Decodable>(_ type: T.Type, from data: Data) throws -> T {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        decoder.dateDecodingStrategy = .custom { input in
            let value = try input.singleValueContainer().decode(String.self)
            let formatter = ISO8601DateFormatter()
            formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
            if let date = formatter.date(from: value) { return date }
            formatter.formatOptions = [.withInternetDateTime]
            guard let date = formatter.date(from: value) else { throw MediaWorkspaceError.malformed }
            return date
        }
        return try decoder.decode(type, from: data)
    }
}
