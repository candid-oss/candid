import DesktopPlatform
import AIProviders
import AppKit
import DesktopCore
import Foundation
import CandidShared
import Testing
@testable import Candid

@Suite struct HomeInteractionTests {
    @MainActor @Test func reopenedWindowResumesVisibilityAfterClose() async throws {
        final class VisibleWindow: NSWindow {
            override var isVisible: Bool { true }
            override var isMiniaturized: Bool { false }
            override var occlusionState: NSWindow.OcclusionState { [.visible] }
        }
        let window = VisibleWindow(contentRect: .zero, styleMask: [], backing: .buffered, defer: true)
        var delivered: [Bool] = []
        let coordinator = DesktopTimelineVisibility.Coordinator(onChange: { delivered.append($0) })
        defer { coordinator.stop() }
        func waitFor(_ expected: [Bool]) async throws {
            for _ in 0..<100 where delivered != expected { try await Task.sleep(for: .milliseconds(10)) }
            #expect(delivered == expected)
        }
        coordinator.observe(window)
        try await waitFor([true])
        NotificationCenter.default.post(name: NSWindow.willCloseNotification, object: window)
        try await waitFor([true, false])
        NotificationCenter.default.post(name: NSWindow.didBecomeKeyNotification, object: window)
        try await waitFor([true, false, true])
    }

    @MainActor @Test func reopeningAClosedMainWindowUsesTheExistingWindowAction() {
        let delegate = DesktopApplicationDelegate()
        var reopened = 0
        delegate.installStatusItem { reopened += 1 }
        #expect(delegate.applicationShouldHandleReopen(NSApplication.shared, hasVisibleWindows: true))
        #expect(reopened == 0)
        #expect(delegate.applicationShouldHandleReopen(NSApplication.shared, hasVisibleWindows: false))
        #expect(reopened == 1)
    }
    private let base = Date(timeIntervalSince1970: 1_800_000_000)

    private func observation(_ id: String, at seconds: Double, text: String?, protected: Bool = false) -> DesktopRecord {
        DesktopRecord(id: id, timestamp: base.addingTimeInterval(seconds), kind: .context, appName: "Synthetic", bundleID: "test.synthetic.a",
                      windowTitle: "Synthetic", runID: "run", trigger: "timer",
                      context: DesktopContextSnapshot(readableText: protected ? nil : text, textStatus: protected ? "protected" : nil, isProtected: protected),
                      activityID: "a")
    }

    private func shot(_ id: String, parent: String?, at seconds: Double) -> DesktopRecord {
        DesktopRecord(id: id, timestamp: base.addingTimeInterval(seconds), kind: .screenshot, appName: "Synthetic", bundleID: "test.synthetic.a",
                      imageHash: String(repeating: "a", count: 64), parentID: parent, runID: "run", activityID: "a")
    }

    @Test func momentsAttachScreenshotsAndCollapseRepeatedText() {
        let observations = [observation("o1", at: 0, text: "alpha"), observation("o2", at: 10, text: "alpha"),
                            observation("o3", at: 20, text: "beta"), observation("o4", at: 30, text: "secret", protected: true)]
        let event = DesktopTimelineEvent(record: observations[0], screenshots: [shot("s1", parent: "o3", at: 21), shot("s2", parent: nil, at: 40)],
                                         observations: observations)
        let moments = EpisodeMoments.build([event])
        #expect(moments.map(\.id) == ["o1", "o3", "o4", "s2"])
        #expect(moments[1].screenshot?.id == "s1")
        #expect(moments[2].text == nil && moments[2].isProtected)
        #expect(EpisodeMoments.index(of: base.addingTimeInterval(25), in: moments) == 1)
        #expect(EpisodeMoments.index(of: base.addingTimeInterval(-60), in: moments) == 0)
        // Moments one second apart: the playhead just after one stays on it.
        let close = [observation("c1", at: 0, text: "one"), observation("c2", at: 1, text: "two"), observation("c3", at: 2, text: "three")]
        let closeMoments = EpisodeMoments.build([DesktopTimelineEvent(record: close[0], observations: close)])
        #expect(EpisodeMoments.index(of: base.addingTimeInterval(1.25), in: closeMoments) == 1)
    }

    @Test func swipeLocksToTheFirstClearAxisAndReportsTotalTranslation() {
        var swipe = SwipeAccumulator()
        swipe.began()
        #expect(swipe.changed(dx: 2, dy: 1) == nil)
        #expect(swipe.changed(dx: -10, dy: 2)?.axis == .horizontal)
        // Later vertical drift does not switch the locked axis.
        #expect(swipe.changed(dx: -5, dy: 40)?.axis == .horizontal)
        #expect(swipe.ended() == SwipeUpdate(phase: .ended, axis: .horizontal, translation: -13))
        #expect(swipe.ended() == nil)
        swipe.began()
        _ = swipe.changed(dx: 0, dy: 80)
        #expect(swipe.ended()?.translation == 80)
    }

    @Test func audioLineFollowsThePlayhead() {
        let audio = TimelineAudioSpan(id: "a", start: base, end: base.addingTimeInterval(100), title: "合成", speech: [],
                                      lines: [.init(start: base, end: base.addingTimeInterval(40), speaker: "1", text: "first"),
                                              .init(start: base.addingTimeInterval(50), end: base.addingTimeInterval(90), speaker: nil, text: "second")])
        #expect(audio.line(at: base.addingTimeInterval(20))?.line.text == "first")
        #expect(abs((audio.line(at: base.addingTimeInterval(20))?.progress ?? 0) - 0.5) < 0.001)
        #expect(audio.line(at: base.addingTimeInterval(45)) == nil)
        #expect(audio.line(at: base.addingTimeInterval(60))?.line.text == "second")
    }

    @Test func recordingsAndScreenshotsJoinTheSameTimeline() {
        let start = Calendar.current.startOfDay(for: base).addingTimeInterval(9 * 3600)
        let record = DesktopRecord(id: "app", timestamp: start, kind: .context, appName: "A", bundleID: "test.synthetic.a",
                                   windowTitle: "W", runID: "run", activityID: "app")
        let event = DesktopTimelineEvent(record: record, startDate: start, endDate: start.addingTimeInterval(1800))
        let audio = TimelineAudioSpan(id: "rec", start: start.addingTimeInterval(600), end: start.addingTimeInterval(900), title: "录音", speech: [])
        let image = TimelineImagePin(id: "img", date: start.addingTimeInterval(1200), title: "截图")
        let scene = TimelineScene(events: [event], audio: [audio], images: [image], day: start, now: start.addingTimeInterval(86_400 * 3))
        #expect(scene.episodes.map(\.kind) == [.app, .audio, .image])
        // Overlap: the recording started later, so it is in focus while it runs.
        #expect(scene.episodes[scene.episodeIndex(at: start.addingTimeInterval(700))!].kind == .audio)
        #expect(scene.episodes[scene.episodeIndex(at: start.addingTimeInterval(1000))!].kind == .app)
        #expect(scene.episodes[scene.episodeIndex(at: start.addingTimeInterval(1230))!].mediaID == "img")
    }

    @Test func widgetSnapshotRoundTripsAndComparesContentOnly() throws {
        let a = WidgetSnapshot(generatedAt: base, day: base, isDemo: true, isRecording: true, isMicRecording: false, recordedSeconds: 60,
                               appCount: 1, recordingCount: 0, topApps: [.init(name: "A", bundleID: "a", seconds: 60)], current: nil, hours: [])
        var b = a; b.generatedAt = base.addingTimeInterval(10)
        #expect(a.sameContent(as: b))
        #expect(a.hours.count == 24)
        let encoder = JSONEncoder(); encoder.dateEncodingStrategy = .iso8601
        let decoder = JSONDecoder(); decoder.dateDecodingStrategy = .iso8601
        #expect(try decoder.decode(WidgetSnapshot.self, from: encoder.encode(a)) == a)
        #expect(WidgetSnapshot.location.path.hasSuffix("Library/Application Support/Candid/widget/status.json"))
    }

    /// Regression: a strip with more thumbnails than the cache holds used to
    /// evict what was on screen, re-request it and blink forever.
    @MainActor @Test func visibleThumbnailsSurviveEvictionWithoutRedecoding() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("home-cache-synthetic-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { TestCore.finish(root) }
        let session = DesktopSession(isDemo: true, dataRoot: root, core: try await TestCore.ready(root))
        defer { session.prepareToQuit() }
        await session.bootstrap()
        // The synthetic day goes through the outbox into the throwaway core before the timeline shows it.
        for _ in 0..<1000 where session.timelineEvents.isEmpty { try await Task.sleep(for: .milliseconds(10)) }
        var shots: [DesktopRecord] = []
        // A nonempty page can arrive while the synthetic outbox is still being delivered.
        // Wait for the images this cache test actually needs, rather than racing that first page.
        let deadline = ContinuousClock.now.advanced(by: .seconds(10))
        repeat {
            shots = []
            for event in session.timelineEvents where event.hasScreenshot && shots.count < 14 {
                shots += await session.screenshotRecords(forEventID: event.id)
            }
            if shots.count < 10 { try await Task.sleep(for: .milliseconds(20)) }
        } while shots.count < 10 && ContinuousClock.now < deadline
        try #require(shots.count >= 10, "synthetic day has screenshots")
        let cache = TimelineMediaCache(session: session)
        func settle(_ record: DesktopRecord) async throws {
            for _ in 0..<300 where !cache.hasImage(for: record, maxPixel: 600) { try await Task.sleep(for: .milliseconds(10)) }
            try #require(cache.hasImage(for: record, maxPixel: 600))
        }
        cache.requestImage(for: shots[0], maxPixel: 600)
        try await settle(shots[0])
        let one = cache.image(for: shots[0], maxPixel: 600)!.size
        cache.pixelBudget = Int(one.width * one.height * 4.5)   // room for four thumbnails

        let visible = Array(shots.prefix(3))
        for record in visible { cache.requestImage(for: record, maxPixel: 600); try await settle(record) }
        // Each "frame" reads what is on screen, then something offscreen loads.
        for record in shots.dropFirst(3) {
            for shown in visible { #expect(cache.image(for: shown, maxPixel: 600) != nil) }
            cache.requestImage(for: record, maxPixel: 600)
            try await settle(record)
        }
        for shown in visible { #expect(cache.hasImage(for: shown, maxPixel: 600)) }
        #expect(cache.decodeCount == shots.count)
        // Another size of the same screenshot stands in while the sharp one loads.
        #expect(cache.image(for: visible[0], maxPixel: 1600) != nil)
    }

    /// Regression: run just after midnight, the synthetic day used to land in yesterday.
    @Test func syntheticDayStaysInTodayAfterMidnight() {
        var calendar = Calendar(identifier: .gregorian); calendar.timeZone = TimeZone(identifier: "Asia/Shanghai")!
        let midnight = calendar.date(from: DateComponents(year: 2026, month: 11, day: 13))!
        let late = midnight.addingTimeInterval(20 * 3600), early = midnight.addingTimeInterval(1800)
        #expect(DesktopDemoContent.dayStart(total: 600, now: late, calendar: calendar) == late.addingTimeInterval(-36_000))
        let start = DesktopDemoContent.dayStart(total: 600, now: early, calendar: calendar)
        #expect(calendar.isDate(start, inSameDayAs: early) && start > midnight)
    }

    @Test func menuBarRecordingClockIsPlainText() {
        #expect(MediaWorkspaceStore.clockText(elapsed: 0) == "0:00")
        #expect(MediaWorkspaceStore.clockText(elapsed: 65.9) == "1:05")
        #expect(MediaWorkspaceStore.clockText(elapsed: 3_725) == "1:02:05")
        #expect(MediaWorkspaceStore.clockText(elapsed: -3) == "0:00")
    }

    @Test func microphoneLevelMapsDecibelsAndSilenceStaysBelowAudible() {
        #expect(MediaCapture.normalized(decibels: 0) == 1)
        #expect(MediaCapture.normalized(decibels: -50) == 0)
        #expect(MediaCapture.normalized(decibels: -160) == 0)
        #expect(MediaCapture.normalized(decibels: .nan) == 0)
        #expect(MediaCapture.normalized(decibels: 6) == 1)
        // A quiet room (~-55 dBFS) reads as silence; soft speech (~-35 dBFS) does not.
        #expect(MediaCapture.normalized(decibels: -55) < MediaWorkspaceStore.audibleLevel)
        #expect(MediaCapture.normalized(decibels: -35) > MediaWorkspaceStore.audibleLevel)
    }

    /// Regression: after the lid closed, occlusion was not re-sent on unlock and
    /// the timeline stayed paused. Wake and a hidden-state poll both recover it.
    @MainActor @Test func timelineVisibilityRecoversAfterWakeWithoutOcclusionNotice() async throws {
        @MainActor final class Window { var visible = false; var delivered: [Bool] = []; var probes = 0 }
        let state = Window()
        let coordinator = DesktopTimelineVisibility.Coordinator(onChange: { state.delivered.append($0) },
                                                                settleDelay: .milliseconds(20), hiddenPollInterval: .milliseconds(40))
        coordinator.probe = { state.probes += 1; return state.visible }
        defer { coordinator.stop() }
        // Waits for an expected delivery; generous deadline because the suite runs in parallel.
        func waitFor(_ expected: [Bool]) async throws {
            for _ in 0..<500 where state.delivered != expected { try await Task.sleep(for: .milliseconds(10)) }
            #expect(state.delivered == expected)
        }

        coordinator.refreshVisibility()
        try await waitFor([false])

        // Wake arrives while occlusion still lags; the delayed recheck must catch it.
        NSWorkspace.shared.notificationCenter.post(name: NSWorkspace.didWakeNotification, object: nil)
        state.visible = true
        try await waitFor([false, true])

        // Hidden again, then visible with no notification at all: the poll recovers it.
        state.visible = false; coordinator.refreshVisibility()
        try await waitFor([false, true, false])
        state.visible = true
        try await waitFor([false, true, false, true])

        // Once visible, polling stops.
        let settled = state.probes
        try await Task.sleep(for: .milliseconds(200))
        #expect(state.probes == settled)
    }

    @MainActor @Test func legacyVisionSettingsMoveOntoTheVisionRoleOnce() throws {
        let suite = "test.ai.migration." + UUID().uuidString
        let defaults = try #require(UserDefaults(suiteName: suite)); defer { defaults.removePersistentDomain(forName: suite) }
        defaults.set("http://127.0.0.1:1234/v1", forKey: "desktop.vision.endpoint")
        defaults.set("google/gemma-4-e4b", forKey: "desktop.vision.model")
        defaults.set("只写主视图", forKey: "desktop.vision.prompt")
        let lm = AIProvider(providerID: "lm", name: "LM Studio", api: .openaiChat, baseURL: "http://localhost:1234/v1", destination: .onDevice)
        let store = InMemoryProviderConfigStore(ProviderConfig(providers: [lm]))
        let runtime = AIRuntime(store: store, secrets: InMemorySecretStore(), sink: InMemoryUsageSink())
        runtime.migrateLegacyVision(defaults: defaults)
        let vision = try #require(runtime.assignment("vision"))
        #expect(vision.model == ModelRef(providerID: "lm", modelID: "google/gemma-4-e4b"))   // reused, not duplicated
        #expect(runtime.config().providers.count == 1)
        #expect(runtime.prompt(for: "vision") == "只写主视图")
        // Runs once: later changes to the old keys are ignored.
        defaults.set("other-model", forKey: "desktop.vision.model")
        runtime.migrateLegacyVision(defaults: defaults)
        #expect(runtime.assignment("vision")?.model?.modelID == "google/gemma-4-e4b")
    }

    @MainActor @Test func legacyVisionWithUnknownEndpointAddsASourceAndDefaultPromptStaysDefault() throws {
        let suite = "test.ai.migration." + UUID().uuidString
        let defaults = try #require(UserDefaults(suiteName: suite)); defer { defaults.removePersistentDomain(forName: suite) }
        defaults.set("http://192.168.1.20:8000/v1", forKey: "desktop.vision.endpoint")
        defaults.set("qwen-vl", forKey: "desktop.vision.model")
        defaults.set(DesktopVisionClient.defaultPrompt, forKey: "desktop.vision.prompt")
        let runtime = AIRuntime(store: InMemoryProviderConfigStore(), secrets: InMemorySecretStore(), sink: InMemoryUsageSink())
        runtime.migrateLegacyVision(defaults: defaults)
        let provider = try #require(runtime.config().providers.first)
        #expect(provider.destination == .selfHosted && provider.baseURL == "http://192.168.1.20:8000/v1")
        #expect(runtime.assignment("vision")?.prompt == nil)
        try runtime.setPrompt("  " + PromptCatalog.defaultPrompt("vision") + "\n", for: "vision")
        #expect(runtime.assignment("vision")?.prompt == nil)
        try runtime.setPrompt("自定义", for: "vision")
        #expect(runtime.prompt(for: "vision") == "自定义")
    }
}

