import AVFoundation
import Foundation
import CandidLocalization

/// All methods run on one serial IO queue. Samples are PCM16 mono at the input
/// device's fixed rate. Segment boundaries split buffers, never restart input.
///
/// `wav` parts are raw PCM sealed at each boundary. `aac` parts are fragmented MP4 written as the samples arrive:
/// every second a fragment is finalized on disk, so a crash loses at most the last second and the rest is recovered
/// at the next start (`RecordingJournal.recoverTail`). AAC parts need no conversion after processing.
final class SegmentedRecordingWriter {
    enum Checkpoint { case beforeWrite, mediaRenamed, manifestCommitted }
    let journal: RecordingJournal
    private(set) var session: RecordingSession
    private(set) var savedSegments = 0
    private(set) var savedFrames: Int64 = 0
    private var current: RecordingSegment?
    private var handle: FileHandle?
    private var encoder: AACPartEncoder?
    private var frames: Int64 = 0
    private var closed = false
    /// Test seam at real IO boundaries; production has no injected hook.
    var checkpoint: ((Checkpoint) throws -> Void)?

    var codec: RecordingCodec { session.codec ?? .wav }

    init(journal: RecordingJournal, sampleRate: Int, seconds: Int, startedAt: Date = Date(), input: RecordingInput = .microphone, codec: RecordingCodec = .wav) throws {
        self.journal = journal
        session = try journal.begin(sampleRate: sampleRate, seconds: seconds, startedAt: startedAt, input: input, codec: codec)
    }

    func append(_ pcm: Data) throws {
        guard !closed, pcm.count % 2 == 0 else { throw RecordingJournalError.invalid }
        var offset = 0
        while offset < pcm.count {
            if current == nil { try openSegment() }
            let remaining = Int64(session.sampleRate * session.segmentSeconds) - frames
            let count = min(pcm.count - offset, Int(remaining * 2))
            try checkpoint?(.beforeWrite)
            let chunk = pcm.subdata(in: offset..<(offset + count))
            if let encoder { try encoder.append(chunk, at: frames) } else { try handle!.write(contentsOf: chunk) }
            offset += count; frames += Int64(count / 2)
            if frames == Int64(session.sampleRate * session.segmentSeconds) { try seal(reason: "segment_limit") }
        }
    }

    func finish(reason: String = "stopped") throws {
        guard !closed else { return }
        defer { closed = true; try? handle?.close(); handle = nil; encoder = nil }
        try seal(reason: reason)
        session.state = reason == "stopped" ? "stopped" : "fault"
        try journal.save(session)
    }

    /// A write failure must never seal a possibly partial write as good audio. An AAC part keeps what its finished
    /// fragments hold; recovery decides what is usable.
    func abandon() {
        closed = true
        try? handle?.close(); handle = nil; encoder = nil
        session.state = "fault"
        try? journal.save(session)
    }

    private func openSegment() throws {
        let occurredAt = session.startedAt.addingTimeInterval(Double(savedFrames) / Double(session.sampleRate))
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd HH-mm-ss"
        var filename = (session.input == .systemAndMicrophone ? L("系统与麦克风录音 ") : L("Mac 录音 ")) + formatter.string(from: occurredAt) + L(" · 第%@段.wav", savedSegments + 1)
        if codec == .aac, filename.hasSuffix(".wav") { filename = String(filename.dropLast(4)) + ".m4a" }
        var segment = RecordingSegment(id: UUID(), recordingID: session.id, index: savedSegments,
                                       sampleStart: savedFrames, sampleRate: session.sampleRate,
                                       occurredAt: occurredAt, filename: filename, input: session.input)
        segment.codec = session.codec
        try journal.save(segment) // intent precedes media
        let url = journal.file(segment, ".part")
        if codec == .aac {
            try? FileManager.default.removeItem(at: url)
            encoder = try AACPartEncoder(url: url, sampleRate: session.sampleRate)
        } else {
            guard FileManager.default.createFile(atPath: url.path, contents: RecordingJournal.wavHeader(frames: 0, rate: session.sampleRate),
                                                attributes: [.posixPermissions: 0o600]) else { throw CocoaError(.fileWriteUnknown) }
            handle = try FileHandle(forWritingTo: url)
            try handle!.seekToEnd()
        }
        current = segment; frames = 0
    }

    private func seal(reason: String) throws {
        guard var segment = current else { return }
        guard frames > 0 else { return }
        segment.endReason = reason
        // Persist the intended ending first; orphan recovery can retain it.
        try journal.save(segment)
        let part = journal.file(segment, ".part"), final = journal.file(segment, segment.mediaSuffix)
        if let encoder {
            try encoder.finish()
            self.encoder = nil
            let fd = Darwin.open(part.path, O_RDONLY)
            if fd >= 0 { _ = fcntl(fd, F_FULLFSYNC); Darwin.close(fd) }
        } else if let handle {
            try handle.seek(toOffset: 0)
            try handle.write(contentsOf: RecordingJournal.wavHeader(frames: frames, rate: session.sampleRate))
            try RecordingJournal.sync(handle)
            try handle.close(); self.handle = nil
        } else { return }
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

/// Mono AAC in fragmented MP4: 48 kbps at 16 kHz and below (measured as accurate as the PCM original for
/// transcription and voiceprints), at most 64 kbps at higher rates. A fragment is finalized every second.
final class AACPartEncoder {
    private let writer: AVAssetWriter
    private let input: AVAssetWriterInput
    private let format: CMAudioFormatDescription
    private let rate: Int

    static func bitrate(for rate: Int) -> Int { min(rate <= 16_000 ? 48_000 : 64_000, rate * 3) }

    init(url: URL, sampleRate: Int) throws {
        rate = sampleRate
        writer = try AVAssetWriter(outputURL: url, fileType: .m4a)
        writer.movieFragmentInterval = CMTime(seconds: 1, preferredTimescale: 600)
        input = AVAssetWriterInput(mediaType: .audio, outputSettings: [
            AVFormatIDKey: kAudioFormatMPEG4AAC, AVSampleRateKey: sampleRate, AVNumberOfChannelsKey: 1,
            AVEncoderBitRateKey: Self.bitrate(for: sampleRate),
        ])
        input.expectsMediaDataInRealTime = true
        guard writer.canAdd(input) else { throw RecordingJournalError.invalid }
        writer.add(input)
        var asbd = AudioStreamBasicDescription(mSampleRate: Double(sampleRate), mFormatID: kAudioFormatLinearPCM,
                                               mFormatFlags: kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked,
                                               mBytesPerPacket: 2, mFramesPerPacket: 1, mBytesPerFrame: 2, mChannelsPerFrame: 1,
                                               mBitsPerChannel: 16, mReserved: 0)
        var description: CMAudioFormatDescription?
        guard CMAudioFormatDescriptionCreate(allocator: nil, asbd: &asbd, layoutSize: 0, layout: nil, magicCookieSize: 0, magicCookie: nil,
                                             extensions: nil, formatDescriptionOut: &description) == noErr, let description,
              writer.startWriting() else { throw writer.error ?? RecordingJournalError.invalid }
        format = description
        writer.startSession(atSourceTime: .zero)
        try? FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
    }

    /// `pcm`: 16-bit mono samples starting `frame` frames into the part.
    func append(_ pcm: Data, at frame: Int64) throws {
        let count = pcm.count / 2
        guard count > 0 else { return }
        var block: CMBlockBuffer?
        guard CMBlockBufferCreateWithMemoryBlock(allocator: nil, memoryBlock: nil, blockLength: pcm.count, blockAllocator: nil, customBlockSource: nil,
                                                 offsetToData: 0, dataLength: pcm.count, flags: 0, blockBufferOut: &block) == noErr, let block,
              pcm.withUnsafeBytes({ CMBlockBufferReplaceDataBytes(with: $0.baseAddress!, blockBuffer: block, offsetIntoDestination: 0, dataLength: pcm.count) }) == noErr
        else { throw RecordingJournalError.invalid }
        var buffer: CMSampleBuffer?
        guard CMAudioSampleBufferCreateReadyWithPacketDescriptions(allocator: nil, dataBuffer: block, formatDescription: format, sampleCount: count,
                                                                    presentationTimeStamp: CMTime(value: frame, timescale: CMTimeScale(rate)),
                                                                    packetDescriptions: nil, sampleBufferOut: &buffer) == noErr, let buffer
        else { throw RecordingJournalError.invalid }
        // The encoder keeps up with real time; a stall beyond two seconds is a failure, not a reason to drop audio.
        let deadline = Date().addingTimeInterval(2)
        while !input.isReadyForMoreMediaData {
            guard writer.status == .writing, Date() < deadline else { throw writer.error ?? RecordingJournalError.overflow }
            usleep(2_000)
        }
        guard input.append(buffer) else { throw writer.error ?? RecordingJournalError.invalid }
    }

    func finish() throws {
        input.markAsFinished()
        let done = DispatchSemaphore(value: 0)
        writer.finishWriting { done.signal() }
        done.wait()
        guard writer.status == .completed else { throw writer.error ?? RecordingJournalError.invalid }
    }
}
