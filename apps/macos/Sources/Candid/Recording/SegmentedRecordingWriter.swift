import Foundation

/// All methods run on one serial IO queue. Samples are PCM16 mono at the input
/// device's fixed rate. Segment boundaries split buffers, never restart input.
final class SegmentedRecordingWriter {
    enum Checkpoint { case beforeWrite, mediaRenamed, manifestCommitted }
    let journal: RecordingJournal
    private(set) var session: RecordingSession
    private(set) var savedSegments = 0
    private(set) var savedFrames: Int64 = 0
    private var current: RecordingSegment?
    private var handle: FileHandle?
    private var frames: Int64 = 0
    private var closed = false
    /// Test seam at real IO boundaries; production has no injected hook.
    var checkpoint: ((Checkpoint) throws -> Void)?

    init(journal: RecordingJournal, sampleRate: Int, seconds: Int, startedAt: Date = Date(), input: RecordingInput = .microphone) throws {
        self.journal = journal
        session = try journal.begin(sampleRate: sampleRate, seconds: seconds, startedAt: startedAt, input: input)
    }

    func append(_ pcm: Data) throws {
        guard !closed, pcm.count % 2 == 0 else { throw RecordingJournalError.invalid }
        var offset = 0
        while offset < pcm.count {
            if handle == nil { try openSegment() }
            let remaining = Int64(session.sampleRate * session.segmentSeconds) - frames
            let count = min(pcm.count - offset, Int(remaining * 2))
            try checkpoint?(.beforeWrite)
            try handle!.write(contentsOf: pcm.subdata(in: offset..<(offset + count)))
            offset += count; frames += Int64(count / 2)
            if frames == Int64(session.sampleRate * session.segmentSeconds) { try seal(reason: "segment_limit") }
        }
    }

    func finish(reason: String = "stopped") throws {
        guard !closed else { return }
        defer { closed = true; try? handle?.close(); handle = nil }
        try seal(reason: reason)
        session.state = reason == "stopped" ? "stopped" : "fault"
        try journal.save(session)
    }

    /// A write failure must never seal a possibly partial write as good audio.
    func abandon() {
        closed = true
        try? handle?.close(); handle = nil
        session.state = "fault"
        try? journal.save(session)
    }

    private func openSegment() throws {
        let occurredAt = session.startedAt.addingTimeInterval(Double(savedFrames) / Double(session.sampleRate))
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd HH-mm-ss"
        let filename = (session.input == .systemAndMicrophone ? "系统与麦克风录音 " : "Mac 录音 ") + formatter.string(from: occurredAt) + " · 第\(savedSegments + 1)段.wav"
        let segment = RecordingSegment(id: UUID(), recordingID: session.id, index: savedSegments,
                                       sampleStart: savedFrames, sampleRate: session.sampleRate,
                                       occurredAt: occurredAt, filename: filename, input: session.input)
        try journal.save(segment) // intent precedes media
        let url = journal.file(segment, ".part")
        guard FileManager.default.createFile(atPath: url.path, contents: RecordingJournal.wavHeader(frames: 0, rate: session.sampleRate),
                                            attributes: [.posixPermissions: 0o600]) else { throw CocoaError(.fileWriteUnknown) }
        handle = try FileHandle(forWritingTo: url)
        try handle!.seekToEnd()
        current = segment; frames = 0
    }

    private func seal(reason: String) throws {
        guard let handle, var segment = current else { return }
        guard frames > 0 else { return }
        segment.endReason = reason
        // Persist the intended ending first; orphan .wav recovery can retain it.
        try journal.save(segment)
        try handle.seek(toOffset: 0)
        try handle.write(contentsOf: RecordingJournal.wavHeader(frames: frames, rate: session.sampleRate))
        try RecordingJournal.sync(handle)
        try handle.close(); self.handle = nil
        let part = journal.file(segment, ".part"), final = journal.file(segment, ".wav")
        let validated = try RecordingJournal.validate(part, sampleRate: session.sampleRate)
        guard validated.frames == frames else { throw RecordingJournalError.invalid }
        try FileManager.default.moveItem(at: part, to: final)
        try RecordingJournal.syncDirectory(final.deletingLastPathComponent())
        try checkpoint?(.mediaRenamed)
        segment.sampleCount = frames; segment.sha256 = validated.hash; segment.byteCount = validated.bytes
        try journal.save(segment)
        savedSegments += 1; savedFrames += frames
        current = nil; frames = 0
        try checkpoint?(.manifestCommitted)
    }
}
