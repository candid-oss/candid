import AVFoundation
import Foundation

/// A bounded bridge from the audio callback to serial file IO. Only copied PCM
/// crosses queues; neither AVAudioPCMBuffer nor AVAudioEngine is shared.
final class RecordingAudioSink: @unchecked Sendable {
    struct Status: Sendable {
        var savedSegments = 0
        var savedFrames: Int64 = 0
        var level: Float = 0
        var lastInput = Date()
        var failure: String?
    }
    private let lock = NSLock()
    private let queue = DispatchQueue(label: "local.candid.recording-writer", qos: .userInitiated)
    private let writer: SegmentedRecordingWriter
    let recordingID: UUID
    private let maximumBytes: Int
    private var pendingBytes = 0
    private var accepting = true
    private var expectedSampleTime: Int64?
    private var state = Status()

    init(journal: RecordingJournal, sampleRate: Int, seconds: Int, startedAt: Date, input: RecordingInput = .microphone) throws {
        writer = try SegmentedRecordingWriter(journal: journal, sampleRate: sampleRate, seconds: seconds, startedAt: startedAt, input: input)
        recordingID = writer.session.id
        maximumBytes = sampleRate * 2 * 5 // five seconds, then fail visibly instead of dropping samples
    }

    var status: Status { lock.withLock { state } }
    /// Bytes admitted but not yet written; lets synthetic tests pace input like a real device would.
    var backlogBytes: Int { lock.withLock { pendingBytes } }

    func receive(_ buffer: AVAudioPCMBuffer, sampleTime: Int64? = nil) {
        guard let channel = buffer.floatChannelData?[0] else { fail(RecordingJournalError.invalid); return }
        let count = Int(buffer.frameLength)
        guard count > 0 else { return }
        let discontinuity = lock.withLock {
            guard accepting else { return false }
            guard let sampleTime else { return false }
            let mismatch = expectedSampleTime.map { $0 != sampleTime } ?? false
            expectedSampleTime = sampleTime + Int64(count)
            return mismatch
        }
        guard !discontinuity else { fail(RecordingJournalError.inputStopped); return }
        var pcm = Data(count: count * 2)
        var sum: Float = 0
        pcm.withUnsafeMutableBytes { bytes in
            let samples = bytes.bindMemory(to: Int16.self)
            for index in 0..<count {
                let value = channel[index].isFinite ? min(1, max(-1, channel[index])) : 0
                sum += value * value
                samples[index] = Int16((value * 32767).rounded()).littleEndian
            }
        }
        let rms = sqrt(sum / Float(max(1, count)))
        enqueue(pcm, level: MediaCapture.normalized(decibels: rms > 0 ? 20 * log10(rms) : -100))
    }

    /// Also used by synthetic tests without opening a capture device.
    func enqueue(_ pcm: Data, level: Float = 0) {
        lock.lock()
        guard accepting, state.failure == nil else { lock.unlock(); return }
        guard pendingBytes + pcm.count <= maximumBytes else {
            state.failure = RecordingJournalError.overflow.localizedDescription
            accepting = false; lock.unlock(); return
        }
        pendingBytes += pcm.count; state.level = level; state.lastInput = Date()
        // Queue insertion and the accepting gate share the lock: finish's barrier
        // cannot overtake a callback that has already been admitted.
        queue.async { [self] in
            defer { lock.withLock { pendingBytes -= pcm.count } }
            guard lock.withLock({ state.failure == nil }) else { return }
            do {
                try writer.append(pcm)
                lock.withLock { state.savedSegments = writer.savedSegments; state.savedFrames = writer.savedFrames }
            } catch { fail(error); writer.abandon() }
        }
        lock.unlock()
    }

    func fail(_ error: Error) {
        lock.withLock { if state.failure == nil { state.failure = error.localizedDescription }; accepting = false }
    }

    func finish(reason: String) async throws {
        lock.withLock { accepting = false }
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            queue.async { [self] in
                do {
                    if let failure = status.failure {
                        writer.abandon()
                        throw NSError(domain: "Recording", code: 1, userInfo: [NSLocalizedDescriptionKey: failure])
                    }
                    try writer.finish(reason: reason)
                    guard writer.savedFrames > 0 else { throw MediaCaptureError.emptyRecording }
                    lock.withLock { state.savedSegments = writer.savedSegments; state.savedFrames = writer.savedFrames }
                    continuation.resume()
                } catch { writer.abandon(); continuation.resume(throwing: error) }
            }
        }
    }
}
