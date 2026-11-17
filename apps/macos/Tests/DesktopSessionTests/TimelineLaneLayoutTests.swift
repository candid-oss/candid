import AppKit
import SwiftUI
import CoreGraphics
import DesktopCore
import Foundation
import Testing
@testable import Candid

/// Folding of short interruptions and the lanes' level of detail, on synthetic events only.
@Suite struct TimelineLaneLayoutTests {
    private let day = Date(timeIntervalSince1970: 1_800_000_000)
    private var base: Date { Calendar.current.startOfDay(for: day).addingTimeInterval(9 * 3600) }
    private var later: Date { day.addingTimeInterval(86_400 * 3) }

    private func event(_ id: String, app: String = "test.synthetic.a", kind: DesktopRecordKind = .context,
                       at minutes: Double, until end: Double? = nil, image: Bool = false) -> DesktopTimelineEvent {
        let start = base.addingTimeInterval(minutes * 60)
        let record = DesktopRecord(id: id, timestamp: start, kind: kind, appName: app, bundleID: app,
                                   windowTitle: "Synthetic " + id, runID: "synthetic-run", activityID: id)
        return DesktopTimelineEvent(record: record, startDate: start, endDate: base.addingTimeInterval((end ?? minutes) * 60), hasScreenshot: image)
    }

    @Test func shortGlanceAtAnotherAppStaysInsideOneEpisode() {
        let scene = TimelineScene(events: [event("a1", at: 0), event("b", app: "test.synthetic.b", at: 10), event("a2", at: 12, until: 20)],
                                  day: day, now: later)
        #expect(scene.episodes.count == 1)
        #expect(scene.episodes[0].spans.map(\.id) == ["a1", "a2"])
        #expect(scene.episodes[0].interruptions.map(\.id) == ["b"])
        // The interruption still exists as its own activity for search and capture identity.
        #expect(scene.activities.map(\.id) == ["a1", "b", "a2"])
        #expect(scene.episodeIndex(at: base.addingTimeInterval(11 * 60)) == 0)
    }

    @Test func longerVisitsAndRecordedPausesStillSplit() {
        let long = TimelineScene(events: [event("a1", at: 0), event("b", app: "test.synthetic.b", at: 10), event("a2", at: 15, until: 20)],
                                 day: day, now: later)
        #expect(long.episodes.map { $0.spans.map(\.id) } == [["a1"], ["b"], ["a2"]])
        let paused = TimelineScene(events: [event("a1", at: 0), event("g", kind: .gap, at: 9), event("b", app: "test.synthetic.b", at: 10),
                                            event("a2", at: 11, until: 20)], day: day, now: later)
        #expect(paused.episodes.allSatisfy { $0.interruptions.isEmpty })
    }

    @Test func narrowEpisodesMergeIntoOneClusterAndWideOnesStayApart() {
        let apps = ["test.synthetic.a", "test.synthetic.b", "test.synthetic.c", "test.synthetic.d"]
        var events = [event("long", at: 0, until: 60)]
        for i in 0..<6 { events.append(event("s\(i)", app: apps[i % 3 + 1], at: 60 + Double(i) * 2)) }
        events.append(event("tail", at: 72, until: 140))
        let scene = TimelineScene(events: events, day: day, now: later)
        // 30 seconds per point: a two-minute episode is 4 points wide.
        let viewport = TimelineViewport(start: base, secondsPerPoint: 30)
        let items = TimelineLaneLayout.screenItems(scene.episodes, viewport: viewport, width: 400)
        #expect(items.count == 3)
        #expect(items[1].isCluster)
        #expect(items[1].episodes.count == 6)
        let icons = TimelineLaneLayout.icons(items, width: 400)
        // Icons never overlap, and every episode is either shown or counted in a +N badge.
        #expect(zip(icons, icons.dropFirst()).allSatisfy { $1.x - $0.x >= TimelineLaneLayout.iconSpacing })
        #expect(icons.reduce(0) { $0 + 1 + $1.extra } == items.reduce(0) { $0 + $1.episodes.count })
        #expect(icons[1].extra >= 5)
    }

    @Test func zoomingInSplitsTheClusterBackIntoEpisodes() {
        let events = (0..<4).map { event("s\($0)", app: $0 % 2 == 0 ? "test.synthetic.a" : "test.synthetic.b", at: Double($0) * 5, until: Double($0) * 5 + 4) }
        let scene = TimelineScene(events: events, day: day, now: later)
        let far = TimelineLaneLayout.screenItems(scene.episodes, viewport: TimelineViewport(start: base, secondsPerPoint: 60), width: 400)
        let near = TimelineLaneLayout.screenItems(scene.episodes, viewport: TimelineViewport(start: base, secondsPerPoint: 3), width: 1000)
        #expect(far.count == 1 && far[0].isCluster)
        #expect(near.count == scene.episodes.count && near.allSatisfy { !$0.isCluster })
    }

    @Test func screenshotFramesBucketByClockAndDotsDoNotOverlap() {
        let events = [0.0, 1, 2, 30, 31].enumerated().map { event("e\($0.offset)", at: $0.element, until: $0.element + 0.5, image: true) }
        let scene = TimelineScene(events: events, day: day, now: later)
        let viewport = TimelineViewport(start: base, secondsPerPoint: 6)
        let buckets = TimelineLaneLayout.shotBuckets(scene.activities, viewport: viewport, width: 800, frameWidth: 60)
        #expect(buckets.map(\.spans.count) == [3, 2])
        // Panning by any amount only slides the frames: same groups, same identities, same shown screenshot.
        for offset in [1.0, 37, 133, 290] {
            let panned = TimelineLaneLayout.shotBuckets(scene.activities, viewport: TimelineViewport(start: base.addingTimeInterval(-offset), secondsPerPoint: 6), width: 800, frameWidth: 60)
            #expect(panned.map(\.id) == buckets.map(\.id) && panned.map(\.spans) == buckets.map(\.spans))
        }
        let dots = TimelineLaneLayout.shotDots(scene.activities, viewport: viewport, width: 800)
        #expect(zip(dots, dots.dropFirst()).allSatisfy { $1.x - $0.x >= 9 })
    }

    @Test func heightDetentsSnapAndCollapsedLanesShrink() {
        #expect(TimelineLaneLayout.snapped(80) == 70)
        #expect(TimelineLaneLayout.snapped(120) == 120)
        #expect(TimelineLaneLayout.snapped(500) == TimelineLaneLayout.maximumGrow)
        #expect(TimelineLaneLayout.next(after: 0) == 70 && TimelineLaneLayout.next(after: 70) == 190 && TimelineLaneLayout.next(after: 190) == 0)
        let tall = TimelineLaneLayout(grow: 190, collapsed: [])
        #expect(tall.showsFilmstrip && tall.showsPromptText)
        let compact = TimelineLaneLayout(grow: 0, collapsed: [.audio])
        #expect(!compact.showsFilmstrip && compact.height(.audio) == 0)   // hidden lanes take no space
        #expect(compact.top(.ai) == compact.height(.screen) + compact.height(.shots))
    }

    @Test func closePromptsGroupAndLeaveRoomForLabels() {
        let prompts = [("p1", base), ("p2", base.addingTimeInterval(60)), ("p3", base.addingTimeInterval(3600))]
        let groups = TimelineLaneLayout.promptGroups(prompts, viewport: TimelineViewport(start: base, secondsPerPoint: 10), width: 1000)
        #expect(groups.map(\.ids) == [["p1", "p2"], ["p3"]])
        #expect(groups[0].room > 300)
    }
    @Test func screenTitlesAvoidOverlappingEpisodesFoldedIconsAndViewportEdges() {
        let items = [
            TimelineLaneLayout.ScreenItem(episodes: [0], x0: -900, x1: 1800),
            .init(episodes: [1], x0: 140, x1: 1500),
            .init(episodes: [2], x0: 150, x1: 1900),
            .init(episodes: [3], x0: 400, x1: 700),
            .init(episodes: [4], x0: 740, x1: 2200)
        ]
        for width: CGFloat in [320, 760, 1200] {
            let icons = TimelineLaneLayout.icons(items, width: width)
            let labels = TimelineLaneLayout.screenLabels(items, icons: icons, width: width)
            #expect(!labels.isEmpty)
            #expect(!labels.contains { $0.item == 2 }, "a folded icon must not leave its title behind")
            for label in labels {
                #expect(label.x >= 0 && label.x + label.width <= width)
                let interval = label.x...(label.x + label.width)
                #expect(icons.allSatisfy { icon in icon.x + 11 <= interval.lowerBound || icon.x - 11 >= interval.upperBound })
            }
            #expect(zip(labels, labels.dropFirst()).allSatisfy { $0.x + $0.width <= $1.x })
        }
        #expect(TimelineLaneLayout.screenLabels(items, icons: TimelineLaneLayout.icons(items, width: 35), width: 35).isEmpty)
    }

    @MainActor @Test func denseLongTitlesRenderInTheActualTimeline() async throws {
        _ = NSApplication.shared
        let events = [event("合成：盘点未完成的计划和功能，标题很长需要省略", at: 0, until: 12),
                      event("合成：继续处理自动构造模拟器的讨论与实现", app: "test.synthetic.b", at: 4, until: 10),
                      event("合成：短暂切换的终端", app: "test.synthetic.c", at: 5, until: 6),
                      event("合成：继续整理项目的下一步计划与待办事项", app: "test.synthetic.d", at: 11, until: 35),
                      event("合成：回到长标题窗口继续工作", at: 39, until: 70)]
        let scene = TimelineScene(events: events, day: day, now: later)
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("synthetic-lane-labels-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let session = DesktopSession(isDemo: true, dataRoot: root)
        let defaults = try #require(UserDefaults(suiteName: "synthetic-labels-" + UUID().uuidString))
        defaults.set("shots,ai,audio", forKey: "timeline.hiddenLanes")
        for dark in [false, true] {
            for scale in [2.0, 6.0, 12.0] {
                let cursor = TimelineCursor(playhead: base.addingTimeInterval(300), viewport: TimelineViewport(start: base.addingTimeInterval(120), secondsPerPoint: scale))
                cursor.viewportReady = true
                let view = NSHostingView(rootView: TimelineScrubber(scene: scene, cursor: cursor, playing: .constant(false), speed: .constant(60), followingNow: false, isToday: false,
                                     cache: TimelineMediaCache(session: session), interacting: .constant(false))
                    .defaultAppStorage(defaults).frame(width: 900, height: 230, alignment: .bottom)
                    .environment(\.colorScheme, dark ? .dark : .light).environment(\.visualStyle, .flat)
                    .background(dark ? Color(white: 0.12) : Color(white: 0.96)))
                view.frame = NSRect(x: 0, y: 0, width: 900, height: 230)
                let window = NSWindow(contentRect: view.frame, styleMask: [.borderless], backing: .buffered, defer: false)
                window.isReleasedWhenClosed = false; window.contentView = view
                try await Task.sleep(for: .milliseconds(180)); view.layoutSubtreeIfNeeded()
                let bitmap = try #require(view.bitmapImageRepForCachingDisplay(in: view.bounds)); view.cacheDisplay(in: view.bounds, to: bitmap)
                if let folder = ProcessInfo.processInfo.environment["CANDID_RENDER_DIR"] {
                    try FileManager.default.createDirectory(atPath: folder, withIntermediateDirectories: true)
                    try #require(bitmap.representation(using: .png, properties: [:])).write(to: URL(fileURLWithPath: folder).appendingPathComponent("timeline-labels-\(Int(scale))-\(dark ? "dark" : "light").png"))
                }
                window.close()
            }
        }
    }

}
