import CoreGraphics
import CandidLocalization
import Foundation

/// The lanes of the home timeline, top to bottom. Calendar joins when its data does.
enum TimelineLane: String, CaseIterable, Identifiable, Sendable {
    case screen, shots, ai, audio
    var id: String { rawValue }
    var title: String {
        switch self { case .screen: L("屏幕"); case .shots: L("截图"); case .ai: "AI"; case .audio: L("录音") }
    }
    var systemImage: String {
        switch self { case .screen: "display"; case .shots: "photo"; case .ai: "sparkle"; case .audio: "waveform" }
    }
    var help: String {
        switch self {
        case .screen: L("用过的应用和窗口；单击定位，再点一次打开")
        case .shots: L("截图；悬停逐张预览，单击定位到那一张")
        case .ai: L("发给 AI 的指令和工作时段；单击展开")
        case .audio: L("录音；单击定位，再点一次打开")
        }
    }
}

/// Pure geometry for the lanes, so level of detail is testable without views.
/// `grow` is the user's dock height above the compact layout (points).
struct TimelineLaneLayout: Equatable {
    static let detents: [CGFloat] = [0, 70, 190]
    static let maximumGrow: CGFloat = 220
    /// Episodes narrower than this merge into one cluster at the current zoom.
    static let minimumEpisodeWidth: CGFloat = 16
    static let iconSpacing: CGFloat = 26
    static let promptSpacing: CGFloat = 14

    var grow: CGFloat
    var collapsed: Set<TimelineLane>

    func height(_ lane: TimelineLane) -> CGFloat {
        if collapsed.contains(lane) { return 0 }
        switch lane {
        case .screen: return 58
        case .shots: return (20 + grow * 0.5).rounded()
        case .ai: return (30 + grow * 0.1).rounded()
        case .audio: return (26 + grow * 0.12).rounded()
        }
    }
    var showsFilmstrip: Bool { !collapsed.contains(.shots) && height(.shots) >= 40 }
    var showsPromptText: Bool { !collapsed.contains(.ai) && height(.ai) >= 40 }
    func top(_ lane: TimelineLane) -> CGFloat {
        TimelineLane.allCases.prefix { $0 != lane }.reduce(0) { $0 + height($1) }
    }
    var lanesHeight: CGFloat { TimelineLane.allCases.reduce(0) { $0 + height($1) } }
    static let axisHeight: CGFloat = 20

    static func snapped(_ grow: CGFloat) -> CGFloat {
        let clamped = min(max(0, grow), maximumGrow)
        return detents.first { abs($0 - clamped) < 18 } ?? clamped
    }
    /// Double-clicking the grip cycles compact → standard → tall.
    static func next(after grow: CGFloat) -> CGFloat { grow < 35 ? detents[1] : grow < 130 ? detents[2] : detents[0] }

    // MARK: Screen lane

    struct ScreenItem: Equatable {
        /// Indices into the scene's episodes; more than one means a merged cluster.
        let episodes: [Int]
        let x0: CGFloat
        let x1: CGFloat
        var isCluster: Bool { episodes.count > 1 }
    }

    /// App episodes in view; runs of episodes each narrower than the minimum merge.
    static func screenItems(_ episodes: [TimelineEpisode], viewport: TimelineViewport, width: CGFloat) -> [ScreenItem] {
        var items: [ScreenItem] = []
        var pending: [Int] = []
        func flush() {
            guard let first = pending.first, let last = pending.last else { return }
            items.append(ScreenItem(episodes: pending, x0: viewport.x(episodes[first].start), x1: viewport.x(episodes[last].end)))
            pending = []
        }
        for (index, episode) in episodes.enumerated() where episode.kind == .app {
            let x0 = viewport.x(episode.start), x1 = viewport.x(episode.end)
            guard x1 >= -60, x0 <= width + 60 else { flush(); continue }
            if x1 - x0 < minimumEpisodeWidth {
                // Only adjacent short episodes merge; a gap of more than a point keeps them apart.
                if let last = pending.last, x0 - viewport.x(episodes[last].end) > 1 { flush() }
                pending.append(index)
            } else {
                flush()
                items.append(ScreenItem(episodes: [index], x0: x0, x1: x1))
            }
        }
        flush()
        return items
    }

    struct Icon: Equatable {
        let item: Int
        let x: CGFloat
        /// Hidden neighbours folded into this icon (shown as +N).
        let extra: Int
    }

    /// One icon at the start of each item, folding neighbours closer than the spacing.
    static func icons(_ items: [ScreenItem], width: CGFloat) -> [Icon] {
        var result: [Icon] = []
        for (index, item) in items.enumerated() {
            let x = max(item.x0, 0) + 11
            guard x <= width + 11 else { continue }
            let hidden = item.episodes.count - 1
            if let last = result.last, x - last.x < iconSpacing {
                result[result.count - 1] = Icon(item: last.item, x: last.x, extra: last.extra + 1 + hidden)
            } else {
                result.append(Icon(item: index, x: x, extra: hidden))
            }
        }
        return result
    }

    struct ScreenLabel: Equatable {
        let item: Int
        let x: CGFloat
        let width: CGFloat
    }

    /// Titles belong to visible icons. Overlapping episodes and the clipped left edge must not let a title
    /// run underneath the next icon/title; folded neighbours are represented by the icon's +N badge.
    static func screenLabels(_ items: [ScreenItem], icons: [Icon], width: CGFloat) -> [ScreenLabel] {
        icons.enumerated().compactMap { index, icon in
            let start = icon.x + 15
            let next = index + 1 < icons.count ? icons[index + 1].x - 17 : width
            let end = min(width, items[icon.item].x1 - 6, next)
            let room = end - start
            guard room >= 60 else { return nil }
            return ScreenLabel(item: icon.item, x: start, width: room)
        }
    }

    // MARK: Screenshot lane

    struct ShotBucket: Equatable {
        /// Stable across panning and new data: the bucket's length and its slot on the clock.
        let id: String
        let x: CGFloat
        /// Span IDs with screenshots in time order; the frame shows the first one unless skimmed.
        let spans: [String]
    }

    /// Bucket lengths on the clock; the shortest one wide enough for a frame is used.
    static let shotBucketSteps: [TimeInterval] = [60, 120, 300, 600, 900, 1800, 3600, 7200, 10_800, 21_600, 43_200, 86_400]

    /// Frames on a fixed clock grid (every 5 min, 10 min, …): panning only slides them, it never regroups
    /// screenshots or changes which one a frame shows. Only zooming across a grid step regroups.
    static func shotBuckets(_ spans: [TimelineSpan], viewport: TimelineViewport, width: CGFloat, frameWidth: CGFloat) -> [ShotBucket] {
        let step = frameWidth + 6
        guard step > 0, viewport.secondsPerPoint > 0 else { return [] }
        let needed = Double(step) * viewport.secondsPerPoint
        let duration = shotBucketSteps.first { $0 >= needed } ?? 86_400
        var buckets: [Int64: [String]] = [:]
        for span in spans where span.hasScreenshot {
            let slot = Int64((span.start.timeIntervalSince1970 / duration).rounded(.down))
            let x = viewport.x(Date(timeIntervalSince1970: Double(slot) * duration))
            guard x >= -step, x <= width else { continue }
            buckets[slot, default: []].append(span.id)
        }
        return buckets.keys.sorted().map { slot in
            ShotBucket(id: "\(Int(duration))-\(slot)", x: viewport.x(Date(timeIntervalSince1970: Double(slot) * duration)) + 3, spans: buckets[slot]!)
        }
    }

    /// Dots for the compact screenshot lane, skipping ones that would overlap.
    static func shotDots(_ spans: [TimelineSpan], viewport: TimelineViewport, width: CGFloat, spacing: CGFloat = 9) -> [(id: String, x: CGFloat)] {
        var last = -CGFloat.infinity
        var result: [(String, CGFloat)] = []
        for span in spans where span.hasScreenshot {
            let x = viewport.x(span.start)
            guard x >= -6, x <= width + 6, x - last >= spacing else { continue }
            last = x; result.append((span.id, x))
        }
        return result
    }

    // MARK: AI lane

    struct PromptGroup: Equatable {
        let ids: [String]
        let x: CGFloat
        /// Room for a text label before the next group.
        let room: CGFloat
    }

    static func promptGroups(_ prompts: [(id: String, date: Date)], viewport: TimelineViewport, width: CGFloat) -> [PromptGroup] {
        var groups: [(ids: [String], x: CGFloat)] = []
        for prompt in prompts.sorted(by: { $0.date < $1.date }) {
            let x = viewport.x(prompt.date)
            guard x >= -8, x <= width + 8 else { continue }
            if let last = groups.last, x - last.x < promptSpacing { groups[groups.count - 1].ids.append(prompt.id) }
            else { groups.append(([prompt.id], x)) }
        }
        return groups.enumerated().map { index, group in
            let next = index + 1 < groups.count ? groups[index + 1].x : width + 200
            return PromptGroup(ids: group.ids, x: group.x, room: next - group.x - 40)
        }
    }
}
