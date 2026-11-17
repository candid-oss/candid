import AppKit
import DesktopCore
import Foundation
import SwiftUI
import Testing
@testable import Candid

/// Home timeline smoothness on synthetic days only (the `--mock` demo day and a larger generated one), hosted in an
/// offscreen window with a throwaway core. Run with `CANDID_PERF=1 script/test.sh --filter HomePerformance`; the
/// numbers go to stdout and `$CANDID_PERF_OUT` (JSON) when set. Debug build: compare runs, not absolute values.
@MainActor @Suite(.serialized, .enabled(if: ProcessInfo.processInfo.environment["CANDID_PERF"] == "1"))
struct HomePerformanceTests {
    struct Result: Codable {
        var name: String
        var episodes = 0
        var activities = 0
        var steps = 0
        var meanMs = 0.0, p95Ms = 0.0, maxMs = 0.0
        /// Steps over one 60 Hz frame (16.7 ms) and over two (33 ms).
        var over16 = 0, over33 = 0
        var homeBodies = 0, stageBodies = 0, scrubberBodies = 0, headerBodies = 0
        var sceneBuilds = 0, sceneUnchanged = 0, decodes = 0
        /// Idle: builds and home bodies during 25 s of a quiet, followed "today".
        var idleBuilds = 0, idleHomeBodies = 0
        var playbackHomeBodies = 0, playbackStageBodies = 0, playbackScrubberBodies = 0
        var residentMB = 0.0
        /// Resident memory before the page is shown, once it settled, after the scrub, at the end.
        var memoryBeforeMB = 0.0, memorySettledMB = 0.0, memoryScrubbedMB = 0.0
        /// After a second identical drag: growth here would be a leak, none means the first drag set a high-water mark.
        var memoryRescrubbedMB = 0.0
    }

    @Test func demoDay() async throws {
        let result = try await measure(name: "demo", large: false)
        try write(result)
    }

    @Test func largeSyntheticDay() async throws {
        let result = try await measure(name: "large", large: true)
        try write(result)
    }

    private func write(_ result: Result) throws {
        let encoder = JSONEncoder(); encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        let data = try encoder.encode(result)
        print("HOME-PERF " + String(decoding: data, as: UTF8.self))
        if let path = ProcessInfo.processInfo.environment["CANDID_PERF_OUT"] {
            try data.write(to: URL(fileURLWithPath: path + "-" + result.name + ".json"))
        }
    }

    private func measure(name: String, large: Bool) async throws -> Result {
        _ = NSApplication.shared
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("home-perf-synthetic-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { TestCore.finish(root) }
        let session = DesktopSession(isDemo: true, dataRoot: root, core: try await TestCore.ready(root))
        defer { session.prepareToQuit() }
        session.setTimelineVisible(true)
        await session.bootstrap()
        try await eventually(seconds: 30) { session.statistics.screenshotCount > 0 && !session.timelineEvents.isEmpty }
        if large {
            let day = Calendar.current.date(byAdding: .day, value: -1, to: Calendar.current.startOfDay(for: Date()))!
            try await seedLargeDay(session, day: day)
            session.selectedDate = day.addingTimeInterval(12 * 3600)
            try await eventually(seconds: 60) { session.timelineEvents.count >= 480 }
        }
        // Let deliveries settle so the timeline is not still growing while we measure.
        var last = -1
        for _ in 0..<40 {
            try await Task.sleep(for: .milliseconds(250))
            if session.timelineEvents.count == last { break }
            last = session.timelineEvents.count
        }

        let memoryBefore = Self.residentMegabytes()
        let content = TimelineHomeView(session: session, media: nil, ambientTint: .constant(.accentColor), pendingJump: .constant(nil), autofocus: false)
            .frame(width: 1440, height: 900)
        let view = NSHostingView(rootView: content)
        view.frame = NSRect(x: 0, y: 0, width: 1440, height: 900)
        let window = NSWindow(contentRect: view.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = view
        window.alphaValue = 0
        window.orderFrontRegardless()
        defer { window.close() }
        func flush() {
            RunLoop.main.run(until: Date().addingTimeInterval(0.001))
            view.layoutSubtreeIfNeeded(); view.displayIfNeeded()
        }
        session.setTimelineVisible(true)
        // Initial load: scene, thumbnails, AI marks.
        let settle = Date().addingTimeInterval(4)
        while Date() < settle { flush(); try await Task.sleep(for: .milliseconds(20)) }

        if let dir = ProcessInfo.processInfo.environment["CANDID_RENDER_DIR"], let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds) {
            view.cacheDisplay(in: view.bounds, to: bitmap)
            try bitmap.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: dir).appendingPathComponent("home-\(name).png"))
        }
        var result = Result(name: name)
        result.memoryBeforeMB = memoryBefore
        result.memorySettledMB = Self.residentMegabytes()
        try Self.heapSnapshot("settled-" + name)
        let events = session.timelineEvents
        result.activities = events.count
        let starts = events.map(\.startDate).sorted()
        let first = try #require(starts.first), end = min(Date(), (events.map(\.endDate).max() ?? first).addingTimeInterval(60))

        // Scrub: one drag across the whole day, 600 positions, each flushed through layout and display.
        TimelinePerf.reset()
        // Optional CPU profile of the scrub itself: `CANDID_PERF_SAMPLE=/path/prefix` runs /usr/bin/sample on this process.
        if let prefix = ProcessInfo.processInfo.environment["CANDID_PERF_SAMPLE"] {
            let sampler = Process()
            sampler.executableURL = URL(fileURLWithPath: "/usr/bin/sample")
            sampler.arguments = [String(ProcessInfo.processInfo.processIdentifier), "6", "-file", prefix + "-" + name + ".txt"]
            try sampler.run()
        }
        var samples: [Double] = []
        let steps = 600
        for index in 0..<steps {
            let date = first.addingTimeInterval(end.timeIntervalSince(first) * Double(index) / Double(steps - 1))
            let clock = ContinuousClock.now
            NotificationCenter.default.post(name: QASnapshot.command, object: "scrub \(date.timeIntervalSince1970)")
            flush()
            let elapsed = ContinuousClock.now - clock
            samples.append(Double(elapsed.components.attoseconds) / 1e15 + Double(elapsed.components.seconds) * 1000)
        }
        NotificationCenter.default.post(name: QASnapshot.command, object: "scrub-end")
        // Thumbnails requested along the way arrive here; they count as decodes, not as scrub frames.
        let tail = Date().addingTimeInterval(1.5)
        while Date() < tail { flush(); try await Task.sleep(for: .milliseconds(20)) }
        result.memoryScrubbedMB = Self.residentMegabytes()
        try Self.heapSnapshot("scrubbed-" + name)
        let sorted = samples.sorted()
        result.steps = steps
        result.meanMs = samples.reduce(0, +) / Double(samples.count)
        result.p95Ms = sorted[Int(Double(sorted.count) * 0.95)]
        result.maxMs = sorted.last ?? 0
        result.over16 = samples.filter { $0 > 16.7 }.count
        result.over33 = samples.filter { $0 > 33.3 }.count
        result.homeBodies = TimelinePerf.count(.homeBody)
        result.stageBodies = TimelinePerf.count(.stageBody)
        result.scrubberBodies = TimelinePerf.count(.scrubberBody)
        result.headerBodies = TimelinePerf.count(.headerBody)
        result.sceneBuilds = TimelinePerf.count(.sceneBuild)
        result.sceneUnchanged = TimelinePerf.count(.sceneUnchanged)
        result.decodes = TimelinePerf.count(.decode)

        for index in 0..<steps {
            let date = first.addingTimeInterval(end.timeIntervalSince(first) * Double(index) / Double(steps - 1))
            NotificationCenter.default.post(name: QASnapshot.command, object: "scrub \(date.timeIntervalSince1970)")
            flush()
        }
        NotificationCenter.default.post(name: QASnapshot.command, object: "scrub-end")
        flush()
        result.memoryRescrubbedMB = Self.residentMegabytes()

        // Playback for 3 s at the default speed: how much of the page redraws per tick.
        NotificationCenter.default.post(name: QASnapshot.command, object: "scrub \(first.addingTimeInterval(60).timeIntervalSince1970)")
        flush()
        TimelinePerf.reset()
        NotificationCenter.default.post(name: QASnapshot.command, object: "play")
        let playEnd = Date().addingTimeInterval(3)
        while Date() < playEnd { flush(); try await Task.sleep(for: .milliseconds(16)) }
        NotificationCenter.default.post(name: QASnapshot.command, object: "pause")
        flush()
        result.playbackHomeBodies = TimelinePerf.count(.homeBody)
        result.playbackStageBodies = TimelinePerf.count(.stageBody)
        result.playbackScrubberBodies = TimelinePerf.count(.scrubberBody)

        // Idle (demo is today): nothing touched for 25 s.
        if !large {
            NotificationCenter.default.post(name: QASnapshot.command, object: "scrub \(Date().timeIntervalSince1970)")
            flush()
            TimelinePerf.reset()
            let idleEnd = Date().addingTimeInterval(25)
            while Date() < idleEnd { flush(); try await Task.sleep(for: .milliseconds(50)) }
            result.idleBuilds = TimelinePerf.count(.sceneBuild)
            result.idleHomeBodies = TimelinePerf.count(.homeBody)
        }

        result.episodes = Self.residentEpisodes(session)
        result.residentMB = Self.residentMegabytes()
        return result
    }

    /// `CANDID_PERF_HEAP=/path/prefix`: object counts by class at a checkpoint (/usr/bin/heap), to find growth.
    private static func heapSnapshot(_ label: String) throws {
        guard let prefix = ProcessInfo.processInfo.environment["CANDID_PERF_HEAP"] else { return }
        let heap = Process()
        heap.executableURL = URL(fileURLWithPath: "/usr/bin/heap")
        heap.arguments = [String(ProcessInfo.processInfo.processIdentifier), "-sortBySize"]
        FileManager.default.createFile(atPath: prefix + "-" + label + ".txt", contents: nil)
        let output = try FileHandle(forWritingTo: URL(fileURLWithPath: prefix + "-" + label + ".txt"))
        heap.standardOutput = output; heap.standardError = output
        try heap.run(); heap.waitUntilExit()
    }

    private static func residentEpisodes(_ session: DesktopSession) -> Int {
        TimelineScene(events: session.timelineEvents, day: session.selectedDate, now: Date()).episodes.count
    }

    private static func residentMegabytes() -> Double {
        var info = mach_task_basic_info()
        var count = mach_msg_type_number_t(MemoryLayout<mach_task_basic_info>.size / MemoryLayout<natural_t>.size)
        let status = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { task_info(mach_task_self_, task_flavor_t(MACH_TASK_BASIC_INFO), $0, &count) }
        }
        return status == KERN_SUCCESS ? Double(info.resident_size) / 1_048_576 : 0
    }

    private func eventually(seconds: Double, _ condition: () -> Bool) async throws {
        let deadline = Date().addingTimeInterval(seconds)
        while !condition() {
            guard Date() < deadline else { Issue.record("condition not met in \(seconds) s"); throw CancellationError() }
            try await Task.sleep(for: .milliseconds(50))
        }
    }

    /// A busy synthetic day: 07:00–23:00 with a switch among twelve apps every 10–90 s, an observation every 15 s,
    /// a screenshot every few minutes (one generated image reused), and markers. All labeled 合成样例.
    private func seedLargeDay(_ session: DesktopSession, day: Date) async throws {
        let store = try #require(session.store)
        let apps = (0..<12).map { ("test.synthetic.app\($0)", "合成应用\($0)") }
        let image = Self.syntheticImage()
        var generator = SystemRandomNumberGenerator()
        var t = day.addingTimeInterval(7 * 3600)
        let stop = day.addingTimeInterval(23 * 3600)
        var index = 0, lastShot = Date.distantPast
        while t < stop {
            let app = apps[Int.random(in: 0..<apps.count, using: &generator)]
            let length = Double.random(in: 10...90, using: &generator) * (index % 37 == 0 ? 20 : 1)
            let activity = "large-\(index)"
            var s = t, first = true, parent: DesktopRecord?
            while s < t.addingTimeInterval(length) {
                let record = DesktopRecord(timestamp: s, kind: first ? .application : .context, appName: app.1, bundleID: app.0,
                                           windowTitle: "合成窗口 \(index)", detail: "合成样例", runID: "synthetic-large", trigger: first ? "application" : "timer",
                                           context: DesktopContextSnapshot(readableText: "合成样例文字 \(index)", textStatus: "partial"), activityID: activity)
                try await store.append(record)
                parent = record; first = false
                s = s.addingTimeInterval(15)
            }
            if let parent, t.timeIntervalSince(lastShot) > 180, let image {
                let shot = DesktopRecord(timestamp: parent.timestamp.addingTimeInterval(1), kind: .screenshot, appName: app.1, bundleID: app.0,
                                         windowTitle: "合成窗口 \(index)", detail: "合成样例截图", imageWidth: 1280, imageHeight: 800,
                                         parentID: parent.id, runID: "synthetic-large", trigger: "timer", activityID: activity)
                _ = try await store.storeScreenshot(record: shot, data: image)
                lastShot = t
            }
            if index % 150 == 75 {
                try await store.append(DesktopRecord(timestamp: t.addingTimeInterval(5), kind: .marker, detail: "合成样例标记", runID: "synthetic-large"))
            }
            t = t.addingTimeInterval(length + Double.random(in: 0...6, using: &generator))
            index += 1
        }
    }

    private static func syntheticImage() -> Data? {
        guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 1280, pixelsHigh: 800, bitsPerSample: 8, samplesPerPixel: 4,
                                         hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0),
              let context = NSGraphicsContext(bitmapImageRep: rep) else { return nil }
        NSGraphicsContext.saveGraphicsState(); NSGraphicsContext.current = context
        NSColor.systemIndigo.setFill(); NSRect(x: 0, y: 0, width: 1280, height: 800).fill()
        NSColor.white.setFill(); NSRect(x: 100, y: 100, width: 600, height: 300).fill()
        NSGraphicsContext.restoreGraphicsState()
        return rep.representation(using: .jpeg, properties: [.compressionFactor: 0.7])
    }
}
