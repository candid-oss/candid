import AVFoundation
import Foundation
import Testing
@testable import Candid

/// Recordings written straight to fragmented AAC: exact frame coordinates, and an unfinished part survives a crash.
/// Synthetic tones only.
@Suite struct AACRecordingTests {
    private func temporary() -> URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("aac-synthetic-" + UUID().uuidString)
    }
    /// A 440 Hz tone as 16-bit PCM, `count` frames from `start`.
    private func tone(_ count: Int, start: Int = 0, rate: Int) -> Data {
        var samples = (start..<(start + count)).map { i -> Int16 in
            let phase = 2.0 * Double.pi * 440.0 * Double(i) / Double(rate)
            return Int16(8000.0 * sin(phase)).littleEndian
        }
        return samples.withUnsafeMutableBytes { Data($0) }
    }

    @Test(arguments: [16_000, 48_000]) func partsAreAACWithExactFrames(rate: Int) throws {
        let root = temporary(); defer { try? FileManager.default.removeItem(at: root) }
        let journal = RecordingJournal(profile: root)
        let writer = try SegmentedRecordingWriter(journal: journal, sampleRate: rate, seconds: 30, codec: .aac)
        let total = rate * 41 + 173
        var offset = 0
        // Device-like buffer sizes, not aligned to AAC packets or part boundaries.
        while offset < total {
            let n = min([480, 1_024, 4_800, 333][offset % 4 == 0 ? 0 : 2], total - offset)
            try writer.append(tone(n, start: offset, rate: rate)); offset += n
        }
        try writer.finish()
        let inventory = try journal.recover()
        #expect(inventory.warnings.isEmpty, "\(inventory.warnings)")
        #expect(inventory.pending.count == 2)
        var next: Int64 = 0
        for segment in inventory.pending {
            #expect(segment.codec == .aac && segment.mediaSuffix == ".m4a" && segment.filename.hasSuffix(".m4a"))
            #expect(segment.sampleStart == next)
            let file = journal.file(segment, ".m4a")
            #expect(!FileManager.default.fileExists(atPath: journal.file(segment, ".wav").path))
            let verified = try RecordingJournal.validate(file, sampleRate: rate)
            #expect(verified.frames == segment.sampleCount && verified.hash == segment.sha256)
            next += segment.sampleCount
        }
        #expect(inventory.pending[0].sampleCount == Int64(rate * 30))
        #expect(next == Int64(total))
        // Under a quarter of 16-bit PCM (48 kbps / 256 kbps at 16 kHz, 64 / 768 at 48 kHz).
        let bytes = inventory.pending.map(\.byteCount).reduce(0, +)
        #expect(bytes * 4 < total * 2, "\(bytes) bytes for \(total) frames")
    }

    @Test func anUnfinishedPartIsRecoveredUpToItsLastFragment() throws {
        let root = temporary(); defer { try? FileManager.default.removeItem(at: root) }
        let journal = RecordingJournal(profile: root)
        let rate = 16_000
        let writer = try SegmentedRecordingWriter(journal: journal, sampleRate: rate, seconds: 600, codec: .aac)
        let id = writer.session.id
        // Eight seconds, paced so fragments are written, then the process "dies" without finishing the part.
        for second in 0..<8 {
            try writer.append(tone(rate, start: second * rate, rate: rate))
            Thread.sleep(forTimeInterval: 0.15)
        }
        Thread.sleep(forTimeInterval: 1.2)
        // What a crash leaves on disk: the journal as it is now, copied before the writer could finish anything.
        let crashed = RecordingJournal(profile: root.appendingPathComponent("after-crash"))
        try FileManager.default.createDirectory(at: crashed.root, withIntermediateDirectories: true)
        try FileManager.default.copyItem(at: journal.directory(id), to: crashed.directory(id))
        let part = crashed.directory(id).appendingPathComponent("000000.part")
        #expect(FileManager.default.fileExists(atPath: part.path))
        let inventory = try crashed.recover()
        let recovered = try #require(inventory.pending.first { $0.recordingID == id }, "\(inventory.warnings)")
        #expect(recovered.recovered && recovered.codec == .aac)
        #expect(recovered.sampleCount > Int64(rate * 4) && recovered.sampleCount <= Int64(rate * 8), "\(recovered.sampleCount)")
        let verified = try RecordingJournal.validate(crashed.file(recovered, ".m4a"), sampleRate: rate)
        #expect(verified.frames == recovered.sampleCount)
        #expect(!FileManager.default.fileExists(atPath: part.path))
        withExtendedLifetime(writer) {}
    }

    @Test func olderWAVJournalsStillRecover() throws {
        let root = temporary(); defer { try? FileManager.default.removeItem(at: root) }
        let journal = RecordingJournal(profile: root)
        let writer = try SegmentedRecordingWriter(journal: journal, sampleRate: 8_000, seconds: 30)
        try writer.append(tone(8_000 * 5, rate: 8_000))
        try writer.finish()
        let segment = try #require(try journal.recover().pending.first)
        #expect(segment.codec == nil && segment.mediaSuffix == ".wav")
    }
}
