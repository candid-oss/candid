import AppKit
import DesktopCore
import DesktopPlatform
import Foundation
import CandidClient
import Testing
@testable import Candid

/// A core that is not there yet (or went away); `inner` brings one in.
@MainActor private final class LateCore: DesktopCoreAccess {
    var inner: TestCore?
    var channel: LocalChannel? { inner?.channel }
    var captureDirect: Bool { inner?.captureDirect ?? false }
}

private func syntheticRoot() throws -> URL {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("desktop-store-synthetic-" + UUID().uuidString)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    return root
}

@MainActor private func until(_ what: String, _ predicate: @escaping @MainActor () -> Bool) async throws {
    let deadline = ContinuousClock.now.advanced(by: .seconds(10))
    while !predicate(), ContinuousClock.now < deadline { try await Task.sleep(for: .milliseconds(10)) }
    try #require(predicate(), Comment(rawValue: what))
}

private let jpeg = Data([0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 4, 0xFF, 0xD9])   // marker bytes only; never decoded here

private func context(_ id: String, activity: String = "act-1", at seconds: Double = 0, text: String = "合成正文") -> DesktopRecord {
    DesktopRecord(id: id, timestamp: Date().addingTimeInterval(-60 + seconds), kind: .context, appName: "Synthetic Editor", bundleID: "test.synthetic.editor",
                  windowTitle: "合成窗口", runID: "run-1", trigger: "timer", context: DesktopContextSnapshot(focusedRole: "AXTextArea", readableText: text, textStatus: "captured"),
                  activityID: activity)
}

@Suite(.serialized) @MainActor
struct DesktopStoreTests {
    @Test func directWritesReachTheTimelineOnceEvenWhenResent() async throws {
        let root = try syntheticRoot(); defer { TestCore.finish(root) }
        let store = try DesktopStore(directory: root, core: try await TestCore.ready(root))
        let observation = context("ctx-1")
        try await store.append(observation)
        var shot = DesktopRecord(id: "shot-1", timestamp: observation.timestamp.addingTimeInterval(1), kind: .screenshot, appName: "Synthetic Editor",
                                 bundleID: "test.synthetic.editor", windowTitle: "合成窗口", imageWidth: 2, imageHeight: 2, parentID: "ctx-1",
                                 runID: "run-1", trigger: "timer", activityID: "act-1")
        shot = try await store.storeScreenshot(record: shot, data: jpeg)
        try await drained(root)
        // A resend after a lost answer (the same records once more) adds nothing.
        try await store.append(observation)
        _ = try await store.storeScreenshot(record: shot, data: jpeg)
        try await drained(root)
        let stored = try await storedRecords(root)
        #expect(stored.map(\.id).sorted() == ["ctx-1", "shot-1"])
        let detail = try #require(try await store.activity(id: "ctx-1"))
        #expect(detail.observations.map(\.id) == ["ctx-1"] && detail.screenshots.map(\.id) == ["shot-1"])
        #expect(detail.observations[0].context?.readableText == "合成正文")
        let hash = try #require(shot.imageHash)
        let url = try #require(await store.imageURL(hash: hash))
        #expect(try Data(contentsOf: url) == jpeg, "the App opens the original by path")
        #expect(store.pending == 0 && store.deliveryProblem == nil)
    }

    @Test func captureWaitsWhileTheCoreIsAwayAndIsDeliveredInOrderLater() async throws {
        let root = try syntheticRoot(); defer { TestCore.finish(root) }
        let late = LateCore()
        let store = try DesktopStore(directory: root, core: late)
        store.retryDelay = { _ in .milliseconds(20) }
        try await store.append(context("a", at: 0))
        try await store.append(context("b", at: 1))
        _ = try await store.storeScreenshot(record: DesktopRecord(id: "s", timestamp: Date().addingTimeInterval(-58), kind: .screenshot, parentID: "b",
                                                                   runID: "run-1", activityID: "act-1"), data: jpeg)
        try await until("waiting with a visible reason") { store.pending == 3 && store.deliveryProblem != nil }
        // Saved on disk: a new App process finds the same entries.
        let names = try FileManager.default.contentsOfDirectory(atPath: root.appendingPathComponent("outbox").path)
        #expect(names.filter { $0.hasSuffix(".json") }.count == 3)
        late.inner = try await TestCore.ready(root)
        store.coreChanged()
        try await drained(root)
        let detail = try #require(try await store.activity(id: "a"))
        // "b" repeats "a" word for word, so the core folds it into "a" and the screenshot of "b" attaches to "a".
        #expect(detail.sampleCount == 1 && detail.screenshots.first?.parentID == "a")
        #expect(detail.endDate > detail.startDate, "the folded sample extends the observation")
    }

    @Test func captureWaitsForTheCutoverThenFollows() async throws {
        let root = try syntheticRoot(); defer { TestCore.finish(root) }
        let core = try await TestCore.ready(root, cutover: false)
        let store = try DesktopStore(directory: root, core: core)
        store.retryDelay = { _ in .milliseconds(20) }
        try await store.append(context("before-cutover"))
        try await until("held until the switch") { store.deliveryProblem == "等待时间线库完成切换。" && store.pending == 1 }
        try await core.cutover()
        store.coreChanged()
        try await drained(root)
        #expect(try await storedRecords(root).map(\.id) == ["before-cutover"])
    }

    @Test func aFullOutboxStopsCaptureWithAVisibleReason() async throws {
        let root = try syntheticRoot(); defer { TestCore.finish(root) }
        let late = LateCore()
        let session = DesktopSession(dataRoot: root, core: late, outboxLimits: .init(items: 4))
        defer { session.prepareToQuit() }
        await session.bootstrap()
        session.startRecording()
        try await until("recording") { session.isRecording }
        for _ in 0..<4 { session.addMarker() }
        try await until("capture stopped") { !session.isRecording && session.error != nil }
        #expect(session.error?.contains("已达上限") == true)
        #expect(session.statusText == "已暂停")
    }

    @Test func deletingAnActivityRemovesItAndItsQueuedRecords() async throws {
        let root = try syntheticRoot(); defer { TestCore.finish(root) }
        let late = LateCore()
        let store = try DesktopStore(directory: root, core: late)
        let first = context("a1", activity: "A", at: 0)
        try await store.append(first)
        try await store.append(context("b1", activity: "B", at: 1))
        late.inner = try await TestCore.ready(root)
        store.coreChanged()
        try await drained(root)
        // A record of A still waiting when the user deletes A never reaches the core.
        late.inner = nil
        try await store.append(context("a2", activity: "A", at: 2))
        try await store.deleteActivity(first)
        late.inner = try await TestCore.ready(root)
        store.coreChanged()
        try await drained(root)
        #expect(try await storedRecords(root).map(\.id) == ["b1"])
    }

    @Test func recognitionIsStoredRevisedAndSupersedesTheEarlierRun() async throws {
        let root = try syntheticRoot(); defer { TestCore.finish(root) }
        let store = try DesktopStore(directory: root, core: try await TestCore.ready(root))
        try await store.append(context("ctx"))
        let shot = try await store.storeScreenshot(record: DesktopRecord(id: "shot", timestamp: Date().addingTimeInterval(-30), kind: .screenshot,
                                                                         appName: "Synthetic Editor", parentID: "ctx", runID: "run-1", activityID: "act-1"), data: jpeg)
        try await drained(root)
        let pending = try #require(try await store.pendingRecognition(.ocr, since: .distantPast, skip: []))
        #expect(pending.id == "shot" && pending.context?.readableText == "合成正文", "the screenshot carries its observation's context")
        let input = try await store.recognitionInput(pending)
        #expect(input.jpeg == jpeg)
        let lines = [DesktopOCRLine(text: "第一行", x: 10, y: 10, width: 50, height: 10)]
        let first = DesktopRecognitionResult(source: "合成 OCR", text: "第一次", completedAt: Date().addingTimeInterval(-10), ocrLines: lines)
        try await store.saveRecognition(first, kind: .ocr, screenshot: shot)
        #expect(try await store.pendingRecognition(.ocr, since: .distantPast, skip: []) == nil)
        #expect(try await store.pendingRecognition(.vision, since: .distantPast, skip: [])?.id == "shot")
        let second = DesktopRecognitionResult(source: "合成 OCR", text: "第二次", completedAt: Date(), ocrLines: lines)
        try await store.saveRecognition(second, kind: .ocr, screenshot: shot)
        let results = try await store.recognitionResults(for: "shot")
        #expect(results[.ocr]?.text == "第二次" && results[.ocr]?.ocrLines == lines, "the full result comes back, line boxes included")
        let info: AppRecognitionInfo = try await TestCore.ready(root).channel!.app("app_recognition", ["screenshot_id": .string("shot")])
        #expect(info.results.count == 1 && info.older.isEmpty, "the earlier run was superseded")
        let imageHash = try #require(shot.imageHash)
        #expect(try await store.existingOCR(forImageHash: imageHash)?.text == "第二次")
    }

    /// Data model v1 §9.1 end to end: the frame is OCR'd before compression, stored as WebP through the outbox, kept
    /// by the core as `image/webp`, and read back (by path, and by OCR of the stored original).
    @Test func newScreenshotsAreStoredAsWebPAndReadBack() async throws {
        guard DesktopWebPEncoder.isAvailable else { return } // script/test.sh provides the core's encoder
        let size = NSSize(width: 960, height: 540)
        let rep = try #require(NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(size.width), pixelsHigh: Int(size.height), bitsPerSample: 8,
                                                samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0))
        NSGraphicsContext.saveGraphicsState(); NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        NSColor.white.setFill(); NSRect(origin: .zero, size: size).fill()
        ("Synthetic WebP capture 4721" as NSString).draw(at: NSPoint(x: 60, y: 260), withAttributes: [.font: NSFont.systemFont(ofSize: 40), .foregroundColor: NSColor.black])
        NSGraphicsContext.restoreGraphicsState()
        let frame = try #require(rep.cgImage)
        let raw = try DesktopOCR.recognize(frame: frame)
        #expect(raw.text.contains("4721"), "OCR of the uncompressed frame: \(raw.text)")
        let webp = try DesktopWebPEncoder.encode(frame)
        #expect(DesktopImageFormat(webp) == .webp)

        let root = try syntheticRoot(); defer { TestCore.finish(root) }
        let store = try DesktopStore(directory: root, core: try await TestCore.ready(root))
        try await store.append(context("ctx"))
        let shot = try await store.storeScreenshot(record: DesktopRecord(id: "shot", timestamp: Date().addingTimeInterval(-30), kind: .screenshot,
                                                                         appName: "Synthetic Editor", imageWidth: 960, imageHeight: 540, parentID: "ctx",
                                                                         runID: "run-1", activityID: "act-1"), data: webp)
        try await drained(root)
        let hash = try #require(shot.imageHash)
        let url = try #require(await store.imageURL(hash: hash))
        #expect(url.pathExtension == "webp")
        #expect(try Data(contentsOf: url) == webp)
        let input = try await store.recognitionInput(shot)
        #expect(input.jpeg == webp)
        #expect(try await DesktopOCR().recognize(input.jpeg).text.contains("4721"), "the stored WebP stays readable")
        #expect(DesktopImageFormat(try DesktopImageEncoder.jpegForModels(input.jpeg)) == .jpeg)
    }
}
