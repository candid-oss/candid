import CoreGraphics
import DesktopCore
import Foundation
import Testing
@testable import Candid

@Suite struct TimelineSceneTests {
    private let day = Date(timeIntervalSince1970: 1_800_000_000)
    private var base: Date { Calendar.current.startOfDay(for: day).addingTimeInterval(9 * 3600) }

    private func event(_ id: String, app: String = "test.synthetic.a", kind: DesktopRecordKind = .context,
                       at minutes: Double, until end: Double? = nil, image: Bool = false) -> DesktopTimelineEvent {
        let start = base.addingTimeInterval(minutes * 60)
        let record = DesktopRecord(id: id, timestamp: start, kind: kind, appName: app, bundleID: app,
                                   windowTitle: "Synthetic " + id, runID: "synthetic-run", activityID: id)
        return DesktopTimelineEvent(record: record, startDate: start, endDate: base.addingTimeInterval((end ?? minutes) * 60), hasScreenshot: image)
    }

    @Test func activitiesLastUntilTheNextRecordButNeverPastTheImplicitCap() {
        let scene = TimelineScene(events: [event("b", at: 10), event("a", at: 0), event("c", app: "test.synthetic.b", at: 60)],
                                  day: day, now: day.addingTimeInterval(86_400 * 3))
        let spans = scene.activities
        #expect(spans.map(\.id) == ["a", "b", "c"])
        #expect(spans[0].end == base.addingTimeInterval(10 * 60))
        // 50 minutes until the next record exceeds the 20 minute cap.
        #expect(spans[1].end == base.addingTimeInterval(30 * 60))
        // No next record and not live: the last observation is the end.
        #expect(spans[2].end == base.addingTimeInterval(60 * 60))
    }

    @Test func liveRecordingExtendsOnlyTheLatestActivity() {
        let live = base.addingTimeInterval(5 * 60)
        let scene = TimelineScene(events: [event("a", at: 0)], day: day, now: live, liveUntil: live)
        #expect(scene.activities.first?.end == live)
    }

    @Test func episodesFoldSameAppRunsAndBreakOnGapsAndOtherApps() {
        let events = [event("a1", at: 0), event("m", kind: .marker, at: 3), event("a2", at: 5, until: 8),
                      event("b", app: "test.synthetic.b", at: 9), event("g", kind: .gap, at: 12), event("a3", at: 20)]
        let scene = TimelineScene(events: events, day: day, now: day.addingTimeInterval(86_400 * 3))
        #expect(scene.episodes.map { $0.spans.map(\.id) } == [["a1", "a2"], ["b"], ["a3"]])
        #expect(scene.gap(at: base.addingTimeInterval(15 * 60))?.id == "g")
        #expect(scene.episodeIndex(at: base.addingTimeInterval(6 * 60)) == 0)
        // Inside the gap, focus stays on the latest episode that already started.
        #expect(scene.episodeIndex(at: base.addingTimeInterval(15 * 60)) == 1)
        #expect(scene.nextActivityStart(after: base.addingTimeInterval(15 * 60)) == base.addingTimeInterval(20 * 60))
    }

    @Test func appTotalsRankByRecordedTime() {
        let scene = TimelineScene(events: [event("a", at: 0), event("b", app: "test.synthetic.b", at: 2), event("a2", at: 12, until: 13)],
                                  day: day, now: day.addingTimeInterval(86_400 * 3))
        #expect(scene.appTotals().map(\.bundleID) == ["test.synthetic.b", "test.synthetic.a"])
        #expect(scene.recordedSeconds == 13 * 60)
    }

    @Test func emptyDayStillHasUsableBounds() {
        let scene = TimelineScene(events: [], day: day, now: day.addingTimeInterval(86_400 * 3))
        #expect(scene.isEmpty)
        #expect(scene.bounds.upperBound.timeIntervalSince(scene.bounds.lowerBound) >= 30 * 60)
        #expect(scene.episodeIndex(at: day) == nil)
    }

    @Test func viewportZoomKeepsTheAnchorAndRespectsLimits() {
        let bounds = base...base.addingTimeInterval(8 * 3600)
        var viewport = TimelineViewport.fitting(bounds, width: 800)
        let anchor = viewport.date(atX: 200)
        viewport.zoom(by: 4, anchorX: 200, width: 800, bounds: bounds)
        #expect(abs(viewport.date(atX: 200).timeIntervalSince(anchor)) < 1)
        viewport.zoom(by: 1_000, anchorX: 200, width: 800, bounds: bounds)
        #expect(viewport.secondsPerPoint == TimelineViewport.minimumSecondsPerPoint)
        viewport.zoom(by: 0.000_1, anchorX: 200, width: 800, bounds: bounds)
        #expect(viewport == TimelineViewport.fitting(bounds, width: 800))
    }

    @Test func viewportPanAndRevealStayInsideBounds() {
        let bounds = base...base.addingTimeInterval(8 * 3600)
        var viewport = TimelineViewport.showing(3600, endingAt: base.addingTimeInterval(4 * 3600), width: 600, bounds: bounds)
        viewport.pan(points: -100_000, width: 600, bounds: bounds)
        #expect(viewport.start == bounds.lowerBound)
        viewport.pan(points: 100_000, width: 600, bounds: bounds)
        #expect(viewport.end(width: 600) == bounds.upperBound)
        let target = base.addingTimeInterval(3600)
        let moved = viewport.reveal(target, width: 600, margin: 40, bounds: bounds)
        #expect(moved)
        #expect((40...560).contains(viewport.x(target)))
        let movedAgain = viewport.reveal(target, width: 600, margin: 40, bounds: bounds)
        #expect(!movedAgain)
    }

    @Test func declutterPrefersLongestAndMagnificationPeaksUnderPointer() {
        let shown = TimelineLayout.declutter([(id: "short", x: 100, weight: 1), (id: "long", x: 110, weight: 9), (id: "far", x: 300, weight: 2),
                                              (id: "off", x: 900, weight: 99)], spacing: 32, visible: 0...500)
        #expect(shown == ["long", "far"])
        #expect(TimelineLayout.magnification(distance: 0) == 1.75)
        #expect(TimelineLayout.magnification(distance: 400) < 1.001)
        #expect(TimelineLayout.tickStep(secondsPerPoint: 60) == 7200)
        #expect(TimelineLayout.tickStep(secondsPerPoint: 2) == 300)
    }
}
