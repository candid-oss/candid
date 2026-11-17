import AppKit
import Foundation
import SwiftUI
import Testing
@testable import Candid

@Suite @MainActor struct RecordingPresentationTests {
    @Test func savedRecoveryAndSavingStatesRenderFromSyntheticAudio() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("app008-render-synthetic-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let journal = RecordingJournal(profile: root)
        let writer = try SegmentedRecordingWriter(journal: journal, sampleRate: 8_000, seconds: 30)
        try writer.append(Data(repeating: 1, count: 480_000))
        writer.abandon()
        var session = writer.session; session.state = "recording"; try journal.save(session)
        let store = MediaWorkspaceStore(profile: root)
        await store.syncRecordings()
        store.selectedRecordingID = try #require(store.recordingInventory.pending.first?.id)
        #expect(store.recoveredRecordingCount == 1 && !store.isRecording)
        let path = ProcessInfo.processInfo.environment["APP008_SNAPSHOT_DIR"]
        let destination = path.map { URL(fileURLWithPath: $0, isDirectory: true) } ?? root.appendingPathComponent("renders")
        try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
        _ = NSApplication.shared
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1280, height: 840),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let view = NSHostingView(rootView: MediaWorkspaceView(store: store).background(Color(nsColor: .windowBackgroundColor)))
        view.frame = NSRect(x: 0, y: 0, width: 1280, height: 840)
        window.contentView = view
        for (name, notice) in [("recovered", "录音已停止 · 分段已保存"), ("saving", "麦克风已停止 · 正在保存最后一段")] {
            store.recordingFinishing = name == "saving"
            store.recordingNotice = notice
            try await Task.sleep(for: .milliseconds(100))
            view.layoutSubtreeIfNeeded()
            let bitmap = try #require(view.bitmapImageRepForCachingDisplay(in: view.bounds))
            view.cacheDisplay(in: view.bounds, to: bitmap)
            let png = try #require(bitmap.representation(using: .png, properties: [:]))
            #expect(png.count > 10_000)
            try png.write(to: destination.appendingPathComponent(name + ".png"))
        }
    }
}
