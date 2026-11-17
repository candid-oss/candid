import CoreGraphics
import DesktopCore
import Foundation

/// One drawable stretch on the home timeline. Presentation only: raw ledger
/// records and activity identity are never rewritten.
struct TimelineSpan: Identifiable, Equatable, Sendable {
    enum Kind: Equatable, Sendable { case activity, marker, gap, system }
    let id: String
    let kind: Kind
    let start: Date
    /// Display end. A single observation lasts until the next record (bounded),
    /// because the user kept using that window until something else happened.
    let end: Date
    let bundleID: String
    let appName: String
    let title: String
    var notes: String? = nil
    let sampleCount: Int
    let hasScreenshot: Bool

    var duration: TimeInterval { end.timeIntervalSince(start) }
    func contains(_ date: Date) -> Bool { date >= start && date < end }
}

/// One card on the timeline: a run of one application, a recording, or a
/// saved screenshot. All three share the same time axis and playhead.
struct TimelineEpisode: Identifiable, Equatable, Sendable {
    enum Kind: Equatable, Sendable { case app, audio, image }
    static let imageDisplayDuration: TimeInterval = 90

    let kind: Kind
    let spans: [TimelineSpan]
    /// Short runs of other apps inside this run (a glance at a message), kept
    /// for display; they are not part of `spans` and never change identity.
    let interruptions: [TimelineSpan]
    let audio: TimelineAudioSpan?
    let image: TimelineImagePin?

    init(spans: [TimelineSpan], interruptions: [TimelineSpan] = []) { kind = .app; self.spans = spans; self.interruptions = interruptions; audio = nil; image = nil }
    init(audio: TimelineAudioSpan) { kind = .audio; spans = []; interruptions = []; self.audio = audio; image = nil }
    init(image: TimelineImagePin) { kind = .image; spans = []; interruptions = []; audio = nil; self.image = image }

    var id: String {
        switch kind {
        case .app: spans[0].id
        case .audio: "audio:" + (audio?.id ?? "")
        case .image: "image:" + (image?.id ?? "")
        }
    }
    var start: Date { audio?.start ?? image?.date ?? spans[0].start }
    var end: Date {
        if let audio { return audio.end }
        if let image { return image.date.addingTimeInterval(Self.imageDisplayDuration) }
        return spans.map(\.end).max() ?? start
    }
    var bundleID: String { spans.first?.bundleID ?? "" }
    var appName: String {
        switch kind {
        case .app: spans[0].appName
        case .audio: audio?.source ?? "录音"
        case .image: image?.source ?? "截图"
        }
    }
    var latest: TimelineSpan? { spans.last }
    var title: String { audio?.title ?? image?.title ?? spans[spans.count - 1].title }
    var notes: String? { spans.last?.notes }
    var sampleCount: Int { spans.reduce(0) { $0 + $1.sampleCount } }
    var screenshotSpans: [TimelineSpan] { spans.filter(\.hasScreenshot) }
    var duration: TimeInterval { end.timeIntervalSince(start) }
    /// Library record behind a recording or screenshot card.
    var mediaID: String? { audio?.id ?? image?.id }
    func contains(_ date: Date) -> Bool { date >= start && date < end }
}

struct TimelineTranscriptLine: Equatable, Sendable {
    let start: Date
    let end: Date
    let speaker: String?
    let text: String
}

/// A recording placed on the timeline; speech ranges come from its transcript.
struct TimelineAudioSpan: Identifiable, Equatable, Sendable {
    let id: String
    let start: Date
    let end: Date
    let title: String
    let speech: [ClosedRange<Date>]
    var lines: [TimelineTranscriptLine] = []
    var source: String = "录音"

    /// The transcript line under `date`, with how far through it the playhead is.
    func line(at date: Date) -> (line: TimelineTranscriptLine, progress: Double)? {
        guard let line = lines.last(where: { $0.start <= date && date < max($0.end, $0.start.addingTimeInterval(1)) }) else { return nil }
        let length = max(1, line.end.timeIntervalSince(line.start))
        return (line, min(1, max(0, date.timeIntervalSince(line.start) / length)))
    }
}

/// A screenshot or image saved to the library at a moment in time.
struct TimelineImagePin: Identifiable, Equatable, Sendable {
    let id: String
    let date: Date
    let title: String
    var source: String = "截图"
}

struct TimelineAppTotal: Identifiable, Equatable, Sendable {
    let bundleID: String
    let appName: String
    let seconds: TimeInterval
    var id: String { bundleID }
}

struct TimelineScene: Equatable, Sendable {
    static let maximumImplicitDuration: TimeInterval = 20 * 60
    static let episodeJoinGap: TimeInterval = 2 * 60
    /// Another app used at most this long between two runs of the same app is
    /// shown as an interruption of one episode instead of splitting it.
    static let interruptionLimit: TimeInterval = 3 * 60

    let spans: [TimelineSpan]
    let episodes: [TimelineEpisode]
    let audio: [TimelineAudioSpan]
    let images: [TimelineImagePin]
    let bounds: ClosedRange<Date>

    /// The activity spans, kept apart once: playback and the playhead read them every frame.
    let activities: [TimelineSpan]
    var isEmpty: Bool { spans.isEmpty && audio.isEmpty && images.isEmpty }

    /// `events` may arrive newest-first (ledger order). `liveUntil` extends the
    /// latest activity while recording is active; nothing is extrapolated otherwise.
    init(events: [DesktopTimelineEvent], audio: [TimelineAudioSpan] = [], images: [TimelineImagePin] = [], day: Date, now: Date, liveUntil: Date? = nil,
         calendar: Calendar = .current) {
        let ordered = events.sorted { $0.startDate == $1.startDate ? $0.id < $1.id : $0.startDate < $1.startDate }
        var spans: [TimelineSpan] = []
        for (index, event) in ordered.enumerated() {
            let record = event.latestRecord
            let kind: TimelineSpan.Kind
            switch event.record.kind {
            case .marker: kind = .marker
            case .gap: kind = .gap
            case .system: kind = .system
            case .application, .window, .context, .screenshot: kind = .activity
            }
            var end = max(event.endDate, event.startDate)
            if kind == .activity {
                let next = ordered[(index + 1)...].first { $0.startDate >= event.startDate }?.startDate
                let cap = end.addingTimeInterval(Self.maximumImplicitDuration)
                if let next { end = max(end, min(next, cap)) }
                else if let liveUntil { end = max(end, min(liveUntil, cap)) }
            } else if kind == .gap, let next = ordered[(index + 1)...].first?.startDate {
                end = max(end, next)
            }
            let title = record.manualTitle ?? event.record.manualTitle ?? (record.windowTitle.isEmpty ? (event.record.windowTitle.isEmpty ? event.record.displayTitle : event.record.windowTitle) : record.windowTitle)
            spans.append(TimelineSpan(id: event.id, kind: kind, start: event.startDate, end: end,
                                      bundleID: event.record.bundleID, appName: event.record.appName.isEmpty ? record.appName : event.record.appName,
                                      title: kind == .activity ? title : (event.record.detail.isEmpty ? event.record.displayTitle : event.record.detail),
                                      notes: record.manualNotes ?? event.record.manualNotes, sampleCount: event.sampleCount, hasScreenshot: event.hasScreenshot))
        }
        self.spans = spans
        self.activities = spans.filter { $0.kind == .activity }
        self.audio = audio.sorted { $0.start < $1.start }
        self.images = images.sorted { $0.date < $1.date }
        self.episodes = (Self.fold(spans) + self.audio.map(TimelineEpisode.init(audio:)) + self.images.map(TimelineEpisode.init(image:)))
            .sorted { $0.start == $1.start ? $0.id < $1.id : $0.start < $1.start }
        let dayStart = calendar.startOfDay(for: day)
        let dayEnd = calendar.date(byAdding: .day, value: 1, to: dayStart) ?? dayStart.addingTimeInterval(86_400)
        let first = (spans.map(\.start) + audio.map(\.start) + images.map(\.date)).min()
        var last = (spans.map(\.end) + audio.map(\.end) + images.map(\.date)).max()
        if calendar.isDate(day, inSameDayAs: now) { last = max(last ?? now, now) }
        let lower = max(dayStart, (first ?? dayStart.addingTimeInterval(8 * 3600)).addingTimeInterval(-15 * 60))
        let upper = min(dayEnd, (last ?? lower.addingTimeInterval(3600)).addingTimeInterval(15 * 60))
        self.bounds = lower...max(upper, lower.addingTimeInterval(30 * 60))
    }

    private static func fold(_ spans: [TimelineSpan]) -> [TimelineEpisode] {
        var runs: [[TimelineSpan]] = []
        for span in spans {
            switch span.kind {
            case .marker, .system: continue
            case .gap: runs.append([]); continue
            case .activity: break
            }
            if let previous = runs.last?.last, previous.bundleID == span.bundleID, !span.bundleID.isEmpty,
               span.start.timeIntervalSince(previous.end) <= episodeJoinGap {
                runs[runs.count - 1].append(span)
            } else { runs.append([span]) }
        }
        // Empty runs mark recorded pauses; they stay as separators so nothing folds across one.
        var episodes: [(spans: [TimelineSpan], interruptions: [TimelineSpan])] = runs.map { ($0, []) }
        var index = 0
        while index + 2 < episodes.count {
            let (a, b, c) = (episodes[index], episodes[index + 1], episodes[index + 2])
            guard !a.spans.isEmpty, !b.spans.isEmpty, !c.spans.isEmpty else { index += 1; continue }
            let bStart = b.spans[0].start, bEnd = b.spans.map(\.end).max() ?? bStart
            let aEnd = a.spans.map(\.end).max() ?? a.spans[0].start
            if let bundle = a.spans.first?.bundleID, !bundle.isEmpty, c.spans.first?.bundleID == bundle,
               b.spans.allSatisfy({ $0.bundleID != bundle }), b.interruptions.isEmpty,
               bEnd.timeIntervalSince(bStart) <= interruptionLimit,
               bStart.timeIntervalSince(aEnd) <= episodeJoinGap, c.spans[0].start.timeIntervalSince(bEnd) <= episodeJoinGap {
                episodes[index] = (a.spans + c.spans, a.interruptions + b.spans + c.interruptions)
                episodes.removeSubrange((index + 1)...(index + 2))
            } else { index += 1 }
        }
        return episodes.filter { !$0.spans.isEmpty }.map { TimelineEpisode(spans: $0.spans, interruptions: $0.interruptions) }
    }

    /// The episode under `date`; when a recording overlaps app activity the
    /// one that started most recently wins. Else the latest one already begun.
    func episodeIndex(at date: Date) -> Int? {
        guard !episodes.isEmpty else { return nil }
        if let exact = episodes.lastIndex(where: { $0.contains(date) }) { return exact }
        return episodes.lastIndex { $0.start <= date } ?? 0
    }

    func activity(at date: Date) -> TimelineSpan? { activities.last { $0.contains(date) } }
    func gap(at date: Date) -> TimelineSpan? { spans.first { $0.kind == .gap && $0.contains(date) } }
    func audio(at date: Date) -> TimelineAudioSpan? { audio.first { date >= $0.start && date < $0.end } }

    /// Start of the next activity strictly after `date`, used by playback to
    /// skip unrecorded stretches instead of replaying emptiness.
    func nextActivityStart(after date: Date) -> Date? { activities.first { $0.start > date }?.start }

    var recordedSeconds: TimeInterval { activities.reduce(0) { $0 + $1.duration } }

    func appTotals(limit: Int = 7) -> [TimelineAppTotal] {
        var totals: [String: (String, TimeInterval)] = [:]
        for span in activities where !span.bundleID.isEmpty {
            totals[span.bundleID, default: (span.appName, 0)].1 += span.duration
        }
        return totals.map { TimelineAppTotal(bundleID: $0.key, appName: $0.value.0, seconds: $0.value.1) }
            .sorted { $0.seconds == $1.seconds ? $0.bundleID < $1.bundleID : $0.seconds > $1.seconds }
            .prefix(limit).map { $0 }
    }

    var longestEpisode: TimelineEpisode? { episodes.max { $0.duration < $1.duration } }
}

/// Maps time to horizontal points. Pure value so pan/zoom limits are testable.
struct TimelineViewport: Equatable, Sendable {
    static let minimumSecondsPerPoint: Double = 1.5
    var start: Date
    var secondsPerPoint: Double

    func x(_ date: Date) -> CGFloat { CGFloat(date.timeIntervalSince(start) / secondsPerPoint) }
    func date(atX x: CGFloat) -> Date { start.addingTimeInterval(Double(x) * secondsPerPoint) }
    func end(width: CGFloat) -> Date { date(atX: width) }

    static func fitting(_ bounds: ClosedRange<Date>, width: CGFloat) -> TimelineViewport {
        let span = max(60, bounds.upperBound.timeIntervalSince(bounds.lowerBound))
        return TimelineViewport(start: bounds.lowerBound, secondsPerPoint: span / Double(max(1, width)))
    }

    static func showing(_ duration: TimeInterval, endingAt end: Date, width: CGFloat, bounds: ClosedRange<Date>) -> TimelineViewport {
        var viewport = TimelineViewport(start: end.addingTimeInterval(-duration * 0.85), secondsPerPoint: duration / Double(max(1, width)))
        viewport.clamp(width: width, bounds: bounds)
        return viewport
    }

    mutating func clamp(width: CGFloat, bounds: ClosedRange<Date>) {
        let maximum = Self.fitting(bounds, width: width).secondsPerPoint
        secondsPerPoint = min(max(secondsPerPoint, Self.minimumSecondsPerPoint), max(maximum, Self.minimumSecondsPerPoint))
        let visible = Double(width) * secondsPerPoint
        let latestStart = bounds.upperBound.addingTimeInterval(-visible)
        if latestStart <= bounds.lowerBound { start = bounds.lowerBound; return }
        start = min(max(start, bounds.lowerBound), latestStart)
    }

    mutating func zoom(by factor: Double, anchorX: CGFloat, width: CGFloat, bounds: ClosedRange<Date>) {
        guard factor.isFinite, factor > 0 else { return }
        let anchor = date(atX: anchorX)
        secondsPerPoint /= factor
        clamp(width: width, bounds: bounds)
        start = anchor.addingTimeInterval(-Double(anchorX) * secondsPerPoint)
        clamp(width: width, bounds: bounds)
    }

    mutating func pan(points: CGFloat, width: CGFloat, bounds: ClosedRange<Date>) {
        start = start.addingTimeInterval(Double(points) * secondsPerPoint)
        clamp(width: width, bounds: bounds)
    }

    /// Keeps `date` inside the margins; returns whether the viewport moved.
    @discardableResult
    mutating func reveal(_ date: Date, width: CGFloat, margin: CGFloat, bounds: ClosedRange<Date>) -> Bool {
        let position = x(date)
        guard position < margin || position > width - margin else { return false }
        start = date.addingTimeInterval(-Double(width) * 0.6 * secondsPerPoint)
        clamp(width: width, bounds: bounds)
        return true
    }
}

enum TimelineLayout {
    /// Longest items keep their label first; others yield when closer than `spacing`.
    static func declutter(_ candidates: [(id: String, x: CGFloat, weight: Double)], spacing: CGFloat,
                          visible: ClosedRange<CGFloat>) -> Set<String> {
        var placed: [CGFloat] = []
        var result = Set<String>()
        for item in candidates.sorted(by: { $0.weight == $1.weight ? $0.id < $1.id : $0.weight > $1.weight })
        where visible.contains(item.x) && placed.allSatisfy({ abs($0 - item.x) > spacing }) {
            placed.append(item.x); result.insert(item.id)
        }
        return result
    }

    /// Dock-style magnification: 1 at rest, `1 + boost` under the pointer.
    static func magnification(distance: CGFloat, boost: CGFloat = 0.75, sigma: CGFloat = 44) -> CGFloat {
        1 + boost * exp(-(distance * distance) / (2 * sigma * sigma))
    }

    static func tickStep(secondsPerPoint: Double, minimumSpacing: CGFloat = 64) -> TimeInterval {
        let steps: [TimeInterval] = [300, 600, 900, 1800, 3600, 7200, 10_800]
        return steps.first { $0 / secondsPerPoint >= Double(minimumSpacing) } ?? 10_800
    }
}
