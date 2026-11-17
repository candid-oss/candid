import AVFoundation
import Foundation
import CandidClient
import Testing
@testable import Candid

@Suite struct NativeMediaTests {
    @Test func originalPartPlaylistHasContinuousWaveformWithoutCopyingAudio() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("native-playlist-synthetic-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let journal = RecordingJournal(profile: root)
        let writer = try SegmentedRecordingWriter(journal: journal, sampleRate: 8000, seconds: 30)
        try writer.append(Data(repeating: 1, count: 480000 * 3)); try writer.finish()
        let parts = try journal.recover().pending
        let playlist = try RecordingPlaybackCopy.make(journal: journal, parts: [parts[0], parts[2]], to: root.appendingPathComponent("cache/playback.wav"))
        let data = try Data(contentsOf: playlist)
        #expect(data.count < 4096)
        #expect(playlist.lastPathComponent.hasSuffix(".playlist.json"))
        let values = try RecordingWaveform.peaks(playlist, buckets: 90)
        #expect(values[20] > 0 && values[40] == 0 && values[70] > 0)
        #expect(!FileManager.default.fileExists(atPath: root.appendingPathComponent("cache/playback.wav").path))
    }
    @Test @MainActor func nativeClientImportsSearchesEditsAndReadsOriginalWithoutHub() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("native-media-synthetic-" + UUID().uuidString)
        defer { TestCore.finish(root) }
        let core = try await TestCore.ready(root)
        let client = MediaLibraryClient(channel: core.channel!)
        let path = root.appendingPathComponent("SYNTHETIC.wav")
        let bytes = RecordingJournal.wavHeader(frames: 8000, rate: 8000) + Data(repeating: 0, count: 16000)
        try bytes.write(to: path)
        let id = UUID()
        let draft = MediaDraft(id: id, filename: "SYNTHETIC.wav", storedName: "SYNTHETIC.wav", kind: "audio", source: "import", occurredAt: Date(timeIntervalSince1970: 1794441600), needsReview: false)
        let first = try await client.upload(draft, file: path)
        #expect(first.id == id)
        #expect(try await client.upload(draft, file: path).id == id)
        let edited = try await client.edit(id, title: "人工标题", notes: "合成备注")
        #expect(edited.displayTitle == "人工标题")
        #expect(try await client.list(query: "合成备注").count == 1)
        let answer = try JSONSerialization.jsonObject(with: await client.content(id)) as! [String: Any]
        let original = URL(fileURLWithPath: answer["path"] as! String)
        #expect(try Data(contentsOf: original) == bytes)
        let player = try AVAudioPlayer(contentsOf: original)
        #expect(abs(player.duration - 1) < 0.01)
        player.currentTime = 0.5
        #expect(abs(player.currentTime - 0.5) < 0.01)
        try await client.delete(id)
        #expect(try await client.list(query: "").isEmpty)
        #expect(try await client.recordingWasDeleted(id))
    }
}
