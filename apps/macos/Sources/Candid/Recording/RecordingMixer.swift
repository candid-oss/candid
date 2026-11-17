import Foundation

/// Serial-queue, bounded timestamp mixer. The two SCK outputs share the host clock.
/// Half a second of headroom absorbs delivery skew; silent system audio is zero.
/// Mono linear interpolation preserves duration across 44.1/48 kHz devices.
final class RecordingMixer {
    enum Input { case system, microphone }
    let rate = 48_000
    private let capacity = 48_000 * 4
    private var ring = [Float](repeating: 0, count: 48_000 * 4)
    private var origin: Double?
    private(set) var cursor = 0
    private var end = 0
    private var microphoneEnd: Int?
    private var systemEnd: Int?
    private var finished = false
    var output: (Data, Float) -> Void

    init(output: @escaping (Data, Float) -> Void) { self.output = output }

    func append(_ samples: [Float], rate inputRate: Double, timestamp: Double, input: Input) throws {
        guard !finished else { return }
        guard timestamp.isFinite, (8_000...192_000).contains(inputRate), !samples.isEmpty,
              samples.count <= Int(inputRate) else { throw MediaCaptureError.audioFormat }
        if origin == nil { origin = timestamp }
        let start = (timestamp - origin!) * Double(rate)
        let stop = start + Double(samples.count) * Double(rate) / inputRate
        guard start.isFinite, stop.isFinite, abs(start - Double(cursor)) < Double(capacity),
              stop - Double(cursor) < Double(capacity) else { throw RecordingJournalError.inputStopped }
        let first = max(0, Int(ceil(start - 0.0001))), last = max(0, Int(ceil(stop - 0.0001)))
        // Never silently discard a delayed, audible packet after its output was saved.
        guard first >= cursor - 96 else { throw RecordingJournalError.inputStopped }
        if input == .microphone, let previous = microphoneEnd, first - previous > 960 {
            throw RecordingJournalError.inputStopped
        }
        for frame in max(cursor, first)..<max(cursor, last) {
            let position = max(0, (Double(frame) - start) * inputRate / Double(rate))
            let lower = min(samples.count - 1, Int(position))
            let upper = min(samples.count - 1, lower + 1)
            let fraction = Float(position - Double(lower))
            let a = samples[lower].isFinite ? samples[lower] : 0
            let b = samples[upper].isFinite ? samples[upper] : 0
            ring[frame % capacity] += (a + (b - a) * fraction) * 0.5
        }
        if input == .microphone { microphoneEnd = max(microphoneEnd ?? 0, last) }
        else { systemEnd = max(systemEnd ?? 0, last) }
        end = max(end, last)
        if let microphoneEnd { flush(to: min(microphoneEnd, end - rate / 2)) }
    }

    func finish() throws {
        guard !finished else { return }
        finished = true
        guard microphoneEnd != nil else { throw RecordingJournalError.inputStopped }
        flush(to: end)
    }

    private func flush(to limit: Int) {
        while cursor < limit {
            let count = min(4096, limit - cursor)
            var pcm = Data(count: count * 2), energy: Float = 0
            pcm.withUnsafeMutableBytes { bytes in
                let values = bytes.bindMemory(to: Int16.self)
                for i in 0..<count {
                    let index = (cursor + i) % capacity
                    let value = min(1, max(-1, ring[index]))
                    ring[index] = 0; energy += value * value
                    values[i] = Int16((value * 32767).rounded()).littleEndian
                }
            }
            cursor += count
            let rms = sqrt(energy / Float(count))
            output(pcm, MediaCapture.normalized(decibels: rms > 0 ? 20 * log10(rms) : -100))
        }
    }
}
