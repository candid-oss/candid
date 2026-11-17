import AVFoundation
import CandidLocalization
import CryptoKit
import Darwin
import Foundation

/// Mac-local outbox, independent of the timeline database and device capture contract.
/// Every .wav has a durable intent before its first sample. A receipt is written only
/// after the existing media service confirms the stable segment ID.
struct RecordingSession: Codable, Sendable {
    var version = 1
    let id: UUID
    let startedAt: Date
    let sampleRate: Int
    let segmentSeconds: Int
    var input: RecordingInput?
    var state = "recording"
}

struct RecordingSegment: Codable, Identifiable, Sendable, Equatable {
    var version = 1
    let id: UUID
    let recordingID: UUID
    let index: Int
    let sampleStart: Int64
    let sampleRate: Int
    let occurredAt: Date
    let filename: String
    var input: RecordingInput?
    var sampleCount: Int64 = 0
    var sha256: String?
    var byteCount: Int = 0
    var endReason = "segment_limit"
    var recovered = false

    var stem: String { String(format: "%06d", index) }
}

struct RecordingInventory: Sendable {
    var segments: [RecordingSegment] = []
    var pending: [RecordingSegment] = []
    var warnings: [String] = []
}

enum RecordingJournalError: LocalizedError {
    case invalid, overflow, inputStopped
    var errorDescription: String? {
        switch self {
        case .invalid: L("录音分段未通过完整性校验，原件已保留。")
        case .overflow: L("录音保存跟不上采样，已停止麦克风。已保存分段保留。")
        case .inputStopped: L("麦克风已停止提供声音，录音已结束。已保存分段保留。")
        }
    }
}

struct RecordingJournal: Sendable {
    let root: URL
    init(profile: URL) { root = profile.appendingPathComponent("Recordings", isDirectory: true) }
    func directory(_ id: UUID) -> URL { root.appendingPathComponent(id.uuidString, isDirectory: true) }
    func file(_ segment: RecordingSegment, _ suffix: String) -> URL {
        directory(segment.recordingID).appendingPathComponent(segment.stem + suffix)
    }

    func begin(sampleRate: Int, seconds: Int, startedAt: Date, input: RecordingInput = .microphone) throws -> RecordingSession {
        guard (8_000...192_000).contains(sampleRate), [30, 60].contains(seconds) else { throw RecordingJournalError.invalid }
        let session = RecordingSession(id: UUID(), startedAt: startedAt, sampleRate: sampleRate, segmentSeconds: seconds, input: input)
        try Self.ensureDirectory(directory(session.id))
        try save(session)
        return session
    }

    func save(_ session: RecordingSession) throws {
        try Self.persist(session, at: directory(session.id).appendingPathComponent("session.json"))
    }
    func save(_ segment: RecordingSegment) throws { try Self.persist(segment, at: file(segment, ".json")) }
    func acknowledge(_ segment: RecordingSegment) throws {
        // A crash before the receipt retries the same ID. A crash after it needs no upload.
        try Self.persist(segment.id, at: file(segment, ".receipt"))
        try? FileManager.default.removeItem(at: file(segment, ".wav"))
    }

    /// Called off the UI thread, with the active writer excluded. Never opens a microphone.
    func recover(excluding activeID: UUID? = nil) throws -> RecordingInventory {
        var result = RecordingInventory()
        guard FileManager.default.fileExists(atPath: root.path) else { return result }
        for directory in try FileManager.default.contentsOfDirectory(at: root, includingPropertiesForKeys: [.isDirectoryKey]) {
            guard UUID(uuidString: directory.lastPathComponent) != nil else { continue }
            do {
                var session = try JSONDecoder().decode(RecordingSession.self, from: Data(contentsOf: directory.appendingPathComponent("session.json")))
                guard session.version == 1, self.directory(session.id).standardizedFileURL.path == directory.standardizedFileURL.path,
                      (8_000...192_000).contains(session.sampleRate), [30, 60].contains(session.segmentSeconds) else { throw RecordingJournalError.invalid }
                let interrupted = session.id != activeID && session.state == "recording"
                let wasRecovered = interrupted || session.state == "recovered"
                let manifests = try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
                    .filter { $0.pathExtension == "json" && $0.lastPathComponent != "session.json" }.sorted { $0.lastPathComponent < $1.lastPathComponent }
                var expectedStart: Int64 = 0
                var previousIndex = -1
                for path in manifests {
                  do {
                    var segment = try JSONDecoder().decode(RecordingSegment.self, from: Data(contentsOf: path))
                    let original = segment
                    guard segment.version == 1, segment.recordingID == session.id, segment.index > previousIndex,
                          segment.sampleStart >= expectedStart, segment.sampleRate == session.sampleRate,
                          path.standardizedFileURL.path == file(segment, ".json").standardizedFileURL.path else { throw RecordingJournalError.invalid }
                    if session.id == activeID && segment.sha256 == nil { break }
                    if FileManager.default.fileExists(atPath: file(segment, ".receipt").path) {
                        guard try JSONDecoder().decode(UUID.self, from: Data(contentsOf: file(segment, ".receipt"))) == segment.id,
                              segment.sampleCount > 0, segment.sha256 != nil else { throw RecordingJournalError.invalid }
                        try? FileManager.default.removeItem(at: file(segment, ".wav"))
                    } else if FileManager.default.fileExists(atPath: file(segment, ".wav").path) {
                        let validated = try Self.validate(file(segment, ".wav"), sampleRate: segment.sampleRate)
                        guard validated.frames <= Int64(session.sampleRate * session.segmentSeconds),
                              segment.sha256 == nil || (segment.sha256 == validated.hash && segment.sampleCount == validated.frames && segment.byteCount == validated.bytes)
                        else { throw RecordingJournalError.invalid }
                        if segment.sha256 == nil {
                            // Media rename completed but the sealed manifest did not.
                            segment.sampleCount = validated.frames; segment.sha256 = validated.hash; segment.byteCount = validated.bytes
                            segment.recovered = true
                        }
                    } else {
                        // Only the unfinished tail may be absent. Preserve its .part for inspection.
                        if session.id != activeID {
                            result.warnings.append(L("一段录音未封存或原件缺失；其余已保存分段可继续使用。"))
                        }
                        continue
                    }
                    if wasRecovered { segment.recovered = true }
                    if session.id != activeID && segment != original { try save(segment) }
                    if segment.sampleStart > expectedStart || segment.index > previousIndex + 1 {
                        result.warnings.append(L("录音存在缺段，已保留原始时间位置。"))
                    }
                    expectedStart = segment.sampleStart + segment.sampleCount
                    previousIndex = segment.index
                    result.segments.append(segment)
                    if !FileManager.default.fileExists(atPath: file(segment, ".receipt").path) { result.pending.append(segment) }
                  } catch {
                    result.warnings.append(L("一段录音未通过校验，原件保留；其余分段继续恢复。"))
                  }
                }
                if interrupted { session.state = "recovered"; try save(session) }
            } catch {
                result.warnings.append(L("一份录音暂时无法恢复，原件保留在 Recordings 文件夹。%@", error.localizedDescription))
            }
        }
        result.segments.sort { $0.occurredAt < $1.occurredAt }
        result.pending.sort { $0.occurredAt < $1.occurredAt }
        return result
    }

    static func validate(_ url: URL, sampleRate: Int) throws -> (frames: Int64, hash: String, bytes: Int) {
        let data = try Data(contentsOf: url)
        guard data.count > 44, (data.count - 44) % 2 == 0 else { throw RecordingJournalError.invalid }
        let frames = Int64((data.count - 44) / 2)
        guard data.prefix(44) == wavHeader(frames: frames, rate: sampleRate) else { throw RecordingJournalError.invalid }
        let audio = try AVAudioFile(forReading: url)
        guard audio.length == frames, audio.processingFormat.channelCount == 1,
              audio.processingFormat.sampleRate == Double(sampleRate),
              let buffer = AVAudioPCMBuffer(pcmFormat: audio.processingFormat, frameCapacity: 8192)
        else { throw RecordingJournalError.invalid }
        var decoded: Int64 = 0
        while decoded < frames {
            try audio.read(into: buffer, frameCount: AVAudioFrameCount(min(8192, frames - decoded)))
            guard buffer.frameLength > 0 else { throw RecordingJournalError.invalid }
            decoded += Int64(buffer.frameLength)
        }
        return (frames, SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined(), data.count)
    }

    static func wavHeader(frames: Int64, rate: Int) -> Data {
        var header = Data()
        func ascii(_ text: String) { header.append(contentsOf: text.utf8) }
        func u16(_ value: UInt16) { var value = value.littleEndian; withUnsafeBytes(of: &value) { header.append(contentsOf: $0) } }
        func u32(_ value: UInt32) { var value = value.littleEndian; withUnsafeBytes(of: &value) { header.append(contentsOf: $0) } }
        ascii("RIFF"); u32(UInt32(frames * 2 + 36)); ascii("WAVEfmt "); u32(16)
        u16(1); u16(1); u32(UInt32(rate)); u32(UInt32(rate * 2)); u16(2); u16(16)
        ascii("data"); u32(UInt32(frames * 2))
        return header
    }

    private static func ensureDirectory(_ url: URL) throws {
        if FileManager.default.fileExists(atPath: url.path) { return }
        let parent = url.deletingLastPathComponent()
        try ensureDirectory(parent)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: false,
                                                attributes: [.posixPermissions: 0o700])
        try syncDirectory(parent)
        try syncDirectory(url)
    }

    static func persist<T: Encodable>(_ value: T, at url: URL) throws {
        let temp = url.appendingPathExtension("tmp")
        let data = try JSONEncoder().encode(value)
        guard FileManager.default.createFile(atPath: temp.path, contents: nil, attributes: [.posixPermissions: 0o600]) else {
            throw CocoaError(.fileWriteUnknown)
        }
        let handle = try FileHandle(forWritingTo: temp)
        do {
            try handle.write(contentsOf: data); try sync(handle); try handle.close()
            guard Darwin.rename(temp.path, url.path) == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
            try syncDirectory(url.deletingLastPathComponent())
        } catch { try? handle.close(); throw error }
    }
    static func sync(_ handle: FileHandle) throws {
        try handle.synchronize()
        guard fcntl(handle.fileDescriptor, F_FULLFSYNC) == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    }
    static func syncDirectory(_ url: URL) throws {
        let fd = Darwin.open(url.path, O_RDONLY)
        guard fd >= 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
        defer { Darwin.close(fd) }
        guard fsync(fd) == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    }
}
