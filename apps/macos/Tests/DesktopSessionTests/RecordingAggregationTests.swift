import AppKit
import AVFoundation
import Foundation
import SwiftUI
import Testing
@testable import Candid

@Suite struct RecordingAggregationTests {
    @Test func missingMiddleDoesNotHideLaterSegmentsAndPlaybackKeepsGap() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("recording-gap-synthetic-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let journal = RecordingJournal(profile: root)
        let writer = try SegmentedRecordingWriter(journal: journal, sampleRate: 8000, seconds: 30)
        try writer.append(Data(repeating: 1, count: 480000 * 3)); try writer.finish()
        let original = try journal.recover().pending
        try FileManager.default.removeItem(at: journal.file(original[1], ".json"))
        let recovered = try journal.recover()
        #expect(recovered.pending.map(\.index) == [0, 2])
        #expect(recovered.pending.map(\.sampleStart) == [0, 480000])
        #expect(!recovered.warnings.isEmpty)
        let destination = root.appendingPathComponent("cache/playback.wav")
        _ = try RecordingPlaybackCopy.make(journal: journal, parts: recovered.pending.reversed(), to: destination, preferOriginalParts: false)
        let bytes = try Data(contentsOf: destination)
        #expect(bytes.dropFirst(44) == Data(repeating: 1, count: 480000) + Data(repeating: 0, count: 480000) + Data(repeating: 1, count: 480000))
        let player = try AVAudioPlayer(contentsOf: destination)
        #expect(abs(player.duration - 90) < 0.001)
        player.currentTime = 61.25
        #expect(abs(player.currentTime - 61.25) < 0.001)
        let peaks = try RecordingWaveform.peaks(destination, buckets: 90)
        #expect(peaks.count == 90 && peaks[20] > 0 && peaks[40] == 0 && peaks[70] > 0)
        #expect(try RecordingJournal.validate(journal.file(original[0], ".wav"), sampleRate: 8000).hash == original[0].sha256)
        #expect(try RecordingJournal.validate(journal.file(original[2], ".wav"), sampleRate: 8000).hash == original[2].sha256)
    }

    @Test func corruptMiddleRecoversLaterAndRepeatedRecoveryIsStable() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("recording-corrupt-synthetic-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let journal = RecordingJournal(profile: root)
        let writer = try SegmentedRecordingWriter(journal: journal, sampleRate: 8000, seconds: 30)
        try writer.append(Data(repeating: 1, count: 480000 * 3)); try writer.finish()
        let original = try journal.recover().pending
        try Data([1, 2, 3]).write(to: journal.file(original[1], ".wav"))
        let once = try journal.recover(), twice = try journal.recover()
        #expect(once.pending.map(\.id) == [original[0].id, original[2].id])
        #expect(once.pending == twice.pending)
    }

    @MainActor @Test func pendingLibraryHasOneRowPerRecordingAndOldDraftStillDecodes() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("recording-list-synthetic-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let journal = RecordingJournal(profile: root)
        let writer = try SegmentedRecordingWriter(journal: journal, sampleRate: 8000, seconds: 30)
        try writer.append(Data(repeating: 1, count: 480000 * 3)); try writer.finish()
        let store = MediaWorkspaceStore(profile: root)
        await store.syncRecordings()
        #expect(store.recordingInventory.pending.count == 3)
        #expect(store.pendingRecordingCount == 1 && store.pendingRecordingRows.count == 1)
        let draft = try #require(store.recordingInventory.pending.first?.draft)
        #expect(draft.recording?.recordingID == writer.session.id)
        #expect(draft.recording?.sampleStart == 0)
        #expect(draft.recording?.startedAt.ISO8601Format() == writer.session.startedAt.ISO8601Format())
    }

    @MainActor @Test(.enabled(if: ProcessInfo.processInfo.environment["APP008_TEST_PROFILE"] != nil))
    func liveSyntheticHubRoundTrip() async throws {
        guard let path = ProcessInfo.processInfo.environment["APP008_TEST_PROFILE"] else { return }
        let profile = URL(fileURLWithPath: path)
        let journal = RecordingJournal(profile: profile)
        let writer = try SegmentedRecordingWriter(journal: journal, sampleRate: 8000, seconds: 30)
        let pcm = Data(repeating: 1, count: 480000)
        try writer.append(pcm)
        let original = try #require(journal.recover(excluding: writer.session.id).pending.first)
        let store = MediaWorkspaceStore(profile: profile)
        await store.syncRecordings()
        // This synthetic writer has no microphone; sync correctly marks interruption.
        #expect(store.pendingRecordingCount == 0)
        #expect(store.records.count == 1)
        #expect(store.records.first?.id == writer.session.id)
        try writer.append(Data(repeating: 2, count: 16000)); try writer.finish()
        await store.syncRecordings()
        store.selectedID = writer.session.id
        await store.refresh(); await store.loadDetails()
        let item = try #require(store.selected)
        #expect(item.recordingParts?.count == 2 && item.durationSeconds == 31)
        let url = try await store.preview(item, directory: profile.appendingPathComponent("test-cache"))
        #expect(try Data(contentsOf: url).dropFirst(44) == pcm + Data(repeating: 2, count: 16000))
        await store.edit(item, title: "人工标题", notes: "人工备注")
        let restarted = MediaWorkspaceStore(profile: profile)
        await restarted.refresh(); restarted.selectedID = item.id; await restarted.loadDetails()
        #expect(restarted.selected?.title == "人工标题" && restarted.selected?.notes == "人工备注")
        // Simulate losing only the local acknowledgement after the server committed.
        try FileManager.default.removeItem(at: journal.file(original, ".receipt"))
        try (RecordingJournal.wavHeader(frames: 240000, rate: 8000) + pcm).write(to: journal.file(original, ".wav"))
        await restarted.syncRecordings(); await restarted.refresh()
        #expect(restarted.pendingRecordingCount == 0 && restarted.records.count == 1)
        #expect(restarted.records.first?.title == "人工标题")
        await restarted.delete(try #require(restarted.selected))
        try FileManager.default.removeItem(at: journal.file(original, ".receipt"))
        try (RecordingJournal.wavHeader(frames: 240000, rate: 8000) + pcm).write(to: journal.file(original, ".wav"))
        let afterDelete = MediaWorkspaceStore(profile: profile)
        await afterDelete.syncRecordings(); await afterDelete.refresh()
        #expect(afterDelete.records.isEmpty && afterDelete.pendingRecordingCount == 0)
    }

    @MainActor @Test(.enabled(if: ProcessInfo.processInfo.environment["APP008_AGGREGATION_SNAPSHOTS"] != nil))
    func detailRendersGapAndManualMetadata() throws {
        guard let directory = ProcessInfo.processInfo.environment["APP008_AGGREGATION_SNAPSHOTS"] else { return }
        let destination = URL(fileURLWithPath: directory)
        try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
        let record = MediaLibraryRecord(title: "合成会议 · 人工标题", notes: "人工备注：确认下次讨论议题", recordingParts: [
            .init(id: UUID(), index: 0, sampleStart: 0, sampleCount: 240000, sampleRate: 8000),
            .init(id: UUID(), index: 2, sampleStart: 480000, sampleCount: 240000, sampleRate: 8000)
        ], gaps: [.init(sampleStart: 240000, sampleCount: 240000, sampleRate: 8000)], recordingState: "recovered", durationSeconds: 90,
           id: UUID(), filename: "synthetic.wav", kind: .audio, source: .macMicrophone, occurredAt: Date(), importedAt: Date(),
           processingStatus: .ready, segments: [.init(startSeconds: 61, endSeconds: 65, text: "合成文本：跨段定位测试。", speaker: nil, identity: nil)],
           error: nil, processingMode: "quick")
        let store = MediaWorkspaceStore(profile: destination, demoRecords: [record])
        _ = NSApplication.shared
        for scheme in [ColorScheme.light, .dark] {
            let view = NSHostingView(rootView: MediaRecordView(item: record, store: store, requestDelete: {})
                .environment(\.colorScheme, scheme).background(scheme == .dark ? Color.black : Color.white))
            view.frame = NSRect(x: 0, y: 0, width: 740, height: 800)
            let window = NSWindow(contentRect: view.frame, styleMask: [.borderless], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            window.appearance = NSAppearance(named: scheme == .dark ? .darkAqua : .aqua)
            window.contentView = view
            view.layoutSubtreeIfNeeded()
            let bitmap = try #require(view.bitmapImageRepForCachingDisplay(in: view.bounds))
            view.cacheDisplay(in: view.bounds, to: bitmap)
            try #require(bitmap.representation(using: .png, properties: [:])).write(to: destination.appendingPathComponent("detail-\(scheme).png"))
            window.close()
        }
    }
}
