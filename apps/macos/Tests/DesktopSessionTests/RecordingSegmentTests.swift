import AVFoundation
import Darwin
import Foundation
import Testing
@testable import Candid

@Suite struct RecordingSegmentTests {
    private func temporary() -> URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("app008-synthetic-" + UUID().uuidString)
    }
    private func pcm(_ count: Int, start: Int = 0) -> Data {
        var samples = (start..<(start + count)).map { Int16(truncatingIfNeeded: $0).littleEndian }
        return samples.withUnsafeMutableBytes { Data($0) }
    }

    @Test(arguments: [30, 60]) func irregularBuffersHaveNoRepeatedOrMissingSamples(seconds: Int) throws {
        let root = temporary(); defer { try? FileManager.default.removeItem(at: root) }
        let journal = RecordingJournal(profile: root)
        let writer = try SegmentedRecordingWriter(journal: journal, sampleRate: 16_000, seconds: seconds)
        let original = pcm(16_000 * 137 + 173)
        var offset = 0
        for size in [2, 978, 16_002, 61_778, 4_446_000] where offset < original.count {
            let end = min(offset + size, original.count)
            try writer.append(original.subdata(in: offset..<end)); offset = end
        }
        if offset < original.count { try writer.append(original.subdata(in: offset..<original.count)) }
        try writer.finish()
        let inventory = try journal.recover()
        #expect(inventory.warnings.isEmpty, "\(inventory.warnings)")
        #expect(inventory.pending.count == (137 / seconds + 1))
        var joined = Data(), next: Int64 = 0
        for segment in inventory.pending {
            #expect(segment.sampleStart == next)
            #expect(!segment.recovered)
            let verified = try RecordingJournal.validate(journal.file(segment, ".wav"), sampleRate: 16_000)
            #expect(verified.frames == segment.sampleCount)
            next += verified.frames
            joined.append(try Data(contentsOf: journal.file(segment, ".wav")).dropFirst(44))
            if segment.index < inventory.pending.count - 1 {
                #expect(segment.sampleCount == Int64(seconds * 16_000))
                #expect(segment.endReason == "segment_limit")
            } else { #expect(segment.endReason == "stopped") }
        }
        #expect(joined == original)
        #expect(next == 16_000 * 137 + 173)
    }

    @Test func mediaBeforeManifestIsRecoveredAndReceiptPreventsRetry() throws {
        let root = temporary(); defer { try? FileManager.default.removeItem(at: root) }
        let journal = RecordingJournal(profile: root)
        let writer = try SegmentedRecordingWriter(journal: journal, sampleRate: 8_000, seconds: 30)
        writer.checkpoint = { if $0 == .mediaRenamed { throw POSIXError(.EIO) } }
        #expect(throws: (any Error).self) { try writer.append(pcm(240_000)) }
        writer.abandon()
        let recovered = try journal.recover()
        let segment = try #require(recovered.pending.first)
        #expect(segment.recovered && segment.sampleCount == 240_000)
        #expect(try journal.recover().pending.map(\.id) == [segment.id])
        try journal.acknowledge(segment)
        let again = try journal.recover()
        #expect(again.pending.isEmpty)
        #expect(again.segments.map(\.id) == [segment.id])
        #expect(again.segments.first?.recovered == true)
        #expect(!FileManager.default.fileExists(atPath: journal.file(segment, ".wav").path))
    }

    @Test(arguments: [ENOSPC, EIO, EACCES]) func writeFaultKeepsSealedSegments(code: Int32) throws {
        let root = temporary(); defer { try? FileManager.default.removeItem(at: root) }
        let journal = RecordingJournal(profile: root)
        let writer = try SegmentedRecordingWriter(journal: journal, sampleRate: 8_000, seconds: 30)
        try writer.append(pcm(240_000))
        writer.checkpoint = { if $0 == .beforeWrite { throw POSIXError(POSIXErrorCode(rawValue: code)!) } }
        #expect(throws: (any Error).self) { try writer.append(pcm(8_000)) }
        writer.abandon()
        let inventory = try journal.recover()
        #expect(inventory.pending.count == 1)
        #expect(inventory.pending.first?.sampleCount == 240_000)
        #expect(!inventory.warnings.isEmpty)
    }

    @Test func directoryPermissionRemovalPreservesSealedAudio() throws {
        let root = temporary(); defer { try? FileManager.default.removeItem(at: root) }
        let journal = RecordingJournal(profile: root)
        let writer = try SegmentedRecordingWriter(journal: journal, sampleRate: 8_000, seconds: 30)
        try writer.append(pcm(240_000))
        let directory = journal.directory(writer.session.id)
        try FileManager.default.setAttributes([.posixPermissions: 0o500], ofItemAtPath: directory.path)
        defer { try? FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: directory.path) }
        #expect(throws: (any Error).self) { try writer.append(pcm(8_000)) }
        writer.abandon()
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: directory.path)
        #expect(try journal.recover().pending.count == 1)
    }

    @Test func corruptSealedFileIsRejectedWithoutDeletingOriginal() throws {
        let root = temporary(); defer { try? FileManager.default.removeItem(at: root) }
        let journal = RecordingJournal(profile: root)
        let writer = try SegmentedRecordingWriter(journal: journal, sampleRate: 8_000, seconds: 30)
        try writer.append(pcm(8_000)); try writer.finish()
        let segment = try #require(journal.recover().pending.first)
        let file = journal.file(segment, ".wav")
        let handle = try FileHandle(forWritingTo: file)
        try handle.seek(toOffset: 100); try handle.write(contentsOf: Data([0xff, 0xff])); try handle.close()
        let result = try journal.recover()
        #expect(result.pending.isEmpty && !result.warnings.isEmpty)
        #expect(FileManager.default.fileExists(atPath: file.path))
    }

    @Test func boundedSinkStopsAndNeverPretendsDroppedInputWasContinuous() async throws {
        let root = temporary(); defer { try? FileManager.default.removeItem(at: root) }
        let sink = try RecordingAudioSink(journal: RecordingJournal(profile: root), sampleRate: 8_000, seconds: 30, startedAt: Date())
        sink.enqueue(pcm(8_000 * 6)) // more than its five-second IO backlog
        #expect(sink.status.failure == RecordingJournalError.overflow.localizedDescription)
        await #expect(throws: (any Error).self) { try await sink.finish(reason: "input_fault") }
        #expect(try RecordingJournal(profile: root).recover().pending.isEmpty)
        let other = try RecordingAudioSink(journal: RecordingJournal(profile: root), sampleRate: 8_000, seconds: 30, startedAt: Date())
        let format = try #require(AVAudioFormat(standardFormatWithSampleRate: 8_000, channels: 1))
        let buffer = try #require(AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 100))
        buffer.frameLength = 100
        for index in 0..<100 { buffer.floatChannelData![0][index] = 0 }
        other.receive(buffer, sampleTime: 0)
        other.receive(buffer, sampleTime: 200) // missing input cannot become apparently continuous audio
        #expect(other.status.failure == RecordingJournalError.inputStopped.localizedDescription)
        await #expect(throws: (any Error).self) { try await other.finish(reason: "input_fault") }
    }

    @Test func syntheticInputBuffersCrossTheActualSinkWithoutGaps() async throws {
        let root = temporary(); defer { try? FileManager.default.removeItem(at: root) }
        let journal = RecordingJournal(profile: root)
        let sink = try RecordingAudioSink(journal: journal, sampleRate: 8_000, seconds: 30, startedAt: Date())
        let format = try #require(AVAudioFormat(standardFormatWithSampleRate: 8_000, channels: 1))
        var expected = Data()
        for second in 0..<31 {
            let buffer = try #require(AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 8_000))
            buffer.frameLength = 8_000
            var samples: [Int16] = []
            for frame in 0..<8_000 {
                let sample = Int16(((second * 8_000 + frame) % 32_000) - 16_000)
                buffer.floatChannelData![0][frame] = Float(sample) / 32767
                samples.append(sample.littleEndian)
            }
            samples.withUnsafeBytes { expected.append(contentsOf: $0) }
            sink.receive(buffer, sampleTime: Int64(second * 8_000))
            // A device delivers one second per second; wait for the writer instead of outrunning its five-second backlog.
            let deadline = Date().addingTimeInterval(10)
            while sink.backlogBytes > 8_000 * 2 && sink.status.failure == nil && Date() < deadline {
                try await Task.sleep(for: .milliseconds(5))
            }
        }
        try await sink.finish(reason: "stopped")
        let inventory = try journal.recover()
        #expect(inventory.pending.map(\.sampleCount) == [240_000, 8_000])
        var actual = Data()
        for segment in inventory.pending { actual.append(try Data(contentsOf: journal.file(segment, ".wav")).dropFirst(44)) }
        #expect(actual == expected)
    }

    @Test func permissionStopSealsQueuedInputAndRejectsLateCallbacks() async throws {
        let root = temporary(); defer { try? FileManager.default.removeItem(at: root) }
        let journal = RecordingJournal(profile: root)
        let sink = try RecordingAudioSink(journal: journal, sampleRate: 8_000, seconds: 30, startedAt: Date())
        sink.enqueue(pcm(8_000))
        // The permission monitor uses the same stop path without opening a device.
        try await sink.finish(reason: "input_fault")
        sink.enqueue(pcm(8_000, start: 8_000))
        let inventory = try journal.recover()
        #expect(inventory.pending.first?.sampleCount == 8_000)
        #expect(inventory.pending.first?.endReason == "input_fault")
        #expect(inventory.pending.count == 1)
    }

    @MainActor @Test func startupRecoversSavedAudioWithoutOpeningMicrophoneAndKeepsLegacyDraft() async throws {
        let root = temporary(); defer { try? FileManager.default.removeItem(at: root) }
        let journal = RecordingJournal(profile: root)
        let writer = try SegmentedRecordingWriter(journal: journal, sampleRate: 8_000, seconds: 30)
        try writer.append(pcm(240_000)); try writer.append(pcm(1_000)); writer.abandon()
        // A legacy pending recording remains readable beside the new journal.
        let pending = root.appendingPathComponent("Pending")
        try FileManager.default.createDirectory(at: pending, withIntermediateDirectories: true)
        let legacy = MediaDraft(id: UUID(), filename: "旧录音.m4a", storedName: "legacy.m4a", kind: "audio",
                                source: "mac_microphone", occurredAt: Date(), needsReview: true)
        let legacyURL = pending.appendingPathComponent(legacy.storedName)
        do {
            let format = try #require(AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 1))
            let buffer = try #require(AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 4_800))
            buffer.frameLength = 4_800
            for index in 0..<4_800 { buffer.floatChannelData![0][index] = Float(index % 100) / 1000 }
            let audio = try AVAudioFile(forWriting: legacyURL, settings: [
                AVFormatIDKey: kAudioFormatMPEG4AAC, AVSampleRateKey: 48_000,
                AVNumberOfChannelsKey: 1, AVEncoderBitRateKey: 64_000
            ])
            try audio.write(from: buffer)
        }
        let legacyBytes = try Data(contentsOf: legacyURL)
        try JSONEncoder().encode(legacy).write(to: root.appendingPathComponent("pending.json"))
        let store = MediaWorkspaceStore(profile: root)
        await store.syncRecordings()
        #expect(!store.isRecording && store.capture.recordingID == nil)
        #expect(store.pendingRecordingCount == 1)
        #expect(store.draft?.id == legacy.id && store.draftFile?.lastPathComponent == "legacy.m4a")
        #expect(store.recordingWarning?.contains("本机") == true)
        #expect(try Data(contentsOf: legacyURL) == legacyBytes)
        #expect(try AVAudioFile(forReading: legacyURL).length > 0)
    }
}
