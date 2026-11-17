import AppKit
import CandidLocalization
import SwiftUI

/// Something on the timeline a click can open.
enum TimelineTarget: Hashable {
    case episode(String)
    case cluster([String])
    case prompt(String)
    /// A span with screenshots; opening it opens its episode at that moment.
    case shot(String)
}

/// An AI instruction, work stretch or note on the AI lane.
struct TimelinePromptMark: Identifiable, Equatable {
    enum Kind: Equatable { case prompt, work, note }
    let id: String
    let kind: Kind
    let start: Date
    let end: Date?
    let preview: String?
    /// Where it was sent (tool · project · session), once known, and its place in that session today.
    var source: AIInteractionTimeline.PromptSource? = nil
    var session: String? = nil
    var ordinal: Int? = nil
    /// The work stretch that followed this instruction; `workRunning` while it has not ended yet.
    var workEnd: Date? = nil
    var workRunning = false
    var hasWork: Bool { workEnd != nil || workRunning }

    /// Instructions with their session's name (its title, or its first instruction today), their place in that
    /// session, and the work stretch that followed each. Built when the AI entries change, never per frame.
    static func build(entries: [AIInteractionTimeline.Entry], sources: [String: AIInteractionTimeline.PromptSource]) -> [TimelinePromptMark] {
        let items = entries.compactMap { entry -> (AIInteractionTimeline.Item, String?, Date)? in
            guard let event = entry.event, let date = event.date else { return nil }
            return (event, entry.preview, date)
        }.sorted { $0.2 < $1.2 }
        let works = items.filter { $0.0.kind == "ai.work" }
        var firstInThread: [String: String] = [:], counts: [String: Int] = [:]
        return items.map { event, preview, date in
            let kind: Kind = event.kind == "ai.work" ? .work : event.kind == "note" ? .note : .prompt
            var mark = TimelinePromptMark(id: event.id, kind: kind, start: date, end: AIInteractionTimeline.Item.date(event.end), preview: preview)
            guard kind != .work else { return mark }
            mark.source = sources[event.id]
            if let thread = event.thread_id {
                counts[thread, default: 0] += 1
                mark.ordinal = counts[thread]
                if firstInThread[thread] == nil { firstInThread[thread] = preview.map { String($0.prefix(18)) } }
                mark.session = mark.source?.session ?? firstInThread[thread]
            }
            // The work stretch that starts with this instruction (same moment, within a few seconds).
            if let work = works.first(where: { abs($0.2.timeIntervalSince(date)) < 5 }) {
                mark.workEnd = AIInteractionTimeline.Item.date(work.0.end)
                mark.workRunning = work.0.end == nil
            }
            return mark
        }
    }
}

/// The multi-lane dock under the stage: one time axis, one playhead and
/// lanes for screen activity, screenshots, AI and recordings. Dragging
/// scrubs, the wheel pans, ⌘-wheel or a pinch zooms, the grip on top sets the
/// height, a lane title collapses its lane. Hover state stays local so the
/// stage above never re-renders while the pointer moves.
struct TimelineScrubber: View {
    let scene: TimelineScene
    /// Playhead, visible stretch and hover point. This view's own body reads only the visible stretch; the playhead
    /// and the hover point are drawn by small layers, so a drag or a mouse move does not rebuild the lanes.
    let cursor: TimelineCursor
    @Binding var playing: Bool
    @Binding var speed: Double
    let followingNow: Bool
    let isToday: Bool
    let cache: TimelineMediaCache
    var prompts: [TimelinePromptMark] = []
    /// What the stage currently shows opened, highlighted on its lane.
    var opened: TimelineTarget?
    /// Bumped by the home view when the playhead was moved on purpose (jump, step, now); the view then follows.
    var revealRequest: Int = 0
    /// True while the user drags, wheels or pinches; the home view holds data updates until it ends.
    @Binding var interacting: Bool
    var onScrub: () -> Void = {}
    var onReturnToNow: () -> Void = {}
    /// First click on an item: put the playhead on it. A click on the item already under the playhead opens it.
    var onLocate: (TimelineTarget) -> Void = { _ in }
    /// Called when the pointer rests on an instruction, so its source can be fetched for the card.
    var onPeekPrompt: (String) -> Void = { _ in }
    var onOpen: (TimelineTarget) -> Void = { _ in }

    var contextActions: (TimelineEpisode) -> AnyView = { _ in AnyView(EmptyView()) }
    var promptActions: (TimelinePromptMark) -> AnyView = { _ in AnyView(EmptyView()) }

    @AppStorage("timeline.laneGrow") private var grow: Double = 70
    @AppStorage("timeline.hiddenLanes") private var collapsedRaw: String = ""
    /// The user moved the view by hand; it no longer follows the playhead until a reveal request or playback.
    @ViewState private var browsing = false
    @ViewState private var wheelGeneration = 0
    @ViewState private var gripBase: Double?
    @ViewState private var width: CGFloat = 0
    @ViewState private var hovered: TimelineTarget?
    /// Screenshot under the pointer while skimming a filmstrip frame.
    @ViewState private var skimmed: String?
    @ViewState private var pressed: TimelineTarget?
    @ViewState private var dragging = false
    @ViewState private var pinchBase: CGFloat = 1
    @ViewState private var wheelMonitor: Any?

    private static let labelWidth: CGFloat = 56
    private var playhead: Date {
        get { cursor.playhead }
        nonmutating set { cursor.playhead = newValue }
    }
    private var viewport: TimelineViewport {
        get { cursor.viewport }
        nonmutating set { cursor.viewport = newValue }
    }
    private var viewportReady: Bool {
        get { cursor.viewportReady }
        nonmutating set { cursor.viewportReady = newValue }
    }
    /// Pointer position when it was last over the lanes, read in actions and menus (never tracked).
    private var hover: CGPoint? { cursor.latestHover }
    private var layout: TimelineLaneLayout { TimelineLaneLayout(grow: CGFloat(grow), collapsed: collapsed) }
    private var collapsed: Set<TimelineLane> { Set(collapsedRaw.split(separator: ",").compactMap { TimelineLane(rawValue: String($0)) }) }
    private var trackHeight: CGFloat { layout.lanesHeight + TimelineLaneLayout.axisHeight }

    var body: some View {
        let _ = TimelinePerf.tick(.scrubberBody)
        VStack(spacing: 0) {
            grip
            controls.padding(.horizontal, 20)
            HStack(alignment: .top, spacing: 0) {
                labels.frame(width: Self.labelWidth)
                GeometryReader { geo in
                    track(width: geo.size.width)
                        .onAppear { width = geo.size.width; prepareViewport() }
                        .onChange(of: geo.size.width) { _, new in width = new; prepareViewport() }
                }
            }
            .frame(height: trackHeight)
            .padding(.leading, 20).padding(.trailing, 20)
            .animation(Motion.spring, value: grow)
            .animation(Motion.spring, value: collapsedRaw)
        }
        .padding(.bottom, 10)
        .background { PlayheadWatcher(cursor: cursor, onMove: follow) }
        .onChange(of: revealRequest) { _, _ in reveal(animated: true) }
        .onChange(of: playing) { _, on in if on { browsing = false } }
        .onChange(of: scene.bounds) { _, _ in prepareViewport() }
        .onDisappear(perform: removeWheelMonitor)
    }

    /// Only playback and live following move the view by themselves; browsing keeps it where the user put it.
    private func follow(_ value: Date) {
        guard !dragging, width > 0, playing || (followingNow && !browsing) else { return }
        var next = viewport
        if next.reveal(value, width: width, margin: playing ? width * 0.18 : 60, bounds: scene.bounds) { viewport = next }
    }

    // MARK: Grip and controls

    private var grip: some View {
        Capsule().fill(gripBase == nil ? Surface.selected.color : Color.secondary)
            .frame(width: gripBase == nil ? 38 : 52, height: 4)
            .frame(maxWidth: .infinity).frame(height: 14)
            .contentShape(Rectangle())
            .onHover { inside in if inside { NSCursor.resizeUpDown.push() } else { NSCursor.pop() } }
            .gesture(DragGesture(minimumDistance: 1, coordinateSpace: .global)
                .onChanged { value in
                    let base = gripBase ?? grow; gripBase = base
                    grow = Double(min(max(0, CGFloat(base) - value.translation.height), TimelineLaneLayout.maximumGrow))
                }
                .onEnded { _ in gripBase = nil; withAnimation(Motion.bouncy) { grow = Double(TimelineLaneLayout.snapped(CGFloat(grow))) } })
            .help(L("拖动调整时间线高度"))
            .accessibilityLabel(L("时间线高度"))
            .animation(Motion.snappy, value: gripBase == nil)
    }

    private var controls: some View {
        HStack(spacing: 6) {
            HStack(spacing: 4) {
                Button { playing.toggle() } label: {
                    Image(systemName: playing ? "pause.fill" : "play.fill")
                        .typography(.caption, .bold)
                        .frame(width: 28, height: 28)
                        .background(Circle().fill(Surface.hover.color))
                        .contentShape(Circle())
                        .contentTransition(.symbolEffect(.replace))
                }
                .buttonStyle(.plain).help(playing ? L("暂停（空格）") : L("按时间播放这一天（空格）"))
                QuietMenu(title: "\(Int(speed))×") {
                    ForEach([60.0, 240, 900], id: \.self) { value in
                        Button("\(Int(value))×") { speed = value }
                    }
                }
                .help(L("播放速度"))
                if isToday && !followingNow {
                    QuietButton(title: L("回到现在"), action: onReturnToNow)
                        .transition(.opacity.combined(with: .scale(scale: 0.9, anchor: .leading)))
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            PlayheadClock(cursor: cursor, animated: !dragging)
            HStack(spacing: 0) {
                QuietButton(title: "−") { zoom(1 / 1.8) }
                QuietButton(title: spanLabel) { browsing = true; withAnimation(Motion.spring) { viewport = .fitting(scene.bounds, width: width) } }
                QuietButton(title: L("＋")) { zoom(1.8) }
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
        }
        .frame(height: 32)
        .animation(Motion.snappy, value: followingNow)
    }

    private var spanLabel: String {
        let visible = Double(width) * viewport.secondsPerPoint
        let day = scene.bounds.upperBound.timeIntervalSince(scene.bounds.lowerBound)
        if width <= 0 || visible >= day - 60 { return L("整天") }
        if visible >= 3600 { return String(format: L("%.1f 小时"), visible / 3600).replacingOccurrences(of: ".0 ", with: " ") }
        return L("%@ 分钟", Int(visible / 60))
    }

    // MARK: Lane titles

    /// Lane titles are labels only; which lanes show is chosen from the right-click menu.
    private var labels: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(TimelineLane.allCases) { lane in
                HStack(spacing: 5) {
                    if !collapsed.contains(lane) {
                        Image(systemName: lane.systemImage)
                        Text(lane.title)
                    }
                }
                .typography(.caption).foregroundStyle(.tertiary)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: lane == .screen ? .bottomLeading : .leading)
                .padding(.bottom, lane == .screen ? 4 : 0)
                .frame(height: layout.height(lane))
                .clipped()
                .contentShape(Rectangle())
                .help(lane.help)
                .contextMenu { laneMenu }
            }
            Spacer(minLength: 0)
        }
    }

    @ViewBuilder private var laneMenu: some View {
        Section(L("显示的轨道")) {
            ForEach(TimelineLane.allCases) { lane in
                Toggle(lane.title, isOn: Binding(get: { !collapsed.contains(lane) }, set: { setVisible(lane, $0) }))
                    .disabled(lane == .screen)
            }
        }
        Section(L("高度")) {
            Button(L("紧凑")) { withAnimation(Motion.bouncy) { grow = 0 } }
            Button(L("标准")) { withAnimation(Motion.bouncy) { grow = Double(TimelineLaneLayout.detents[1]) } }
            Button(L("展开")) { withAnimation(Motion.bouncy) { grow = Double(TimelineLaneLayout.detents[2]) } }
        }
    }

    /// The screen lane always stays: it is what the playhead and the stage are about.
    private func setVisible(_ lane: TimelineLane, _ visible: Bool) {
        guard lane != .screen else { return }
        var set = collapsed
        if visible { set.remove(lane) } else { set.insert(lane) }
        withAnimation(Motion.spring) { collapsedRaw = TimelineLane.allCases.filter(set.contains).map(\.rawValue).joined(separator: ",") }
    }

    // MARK: Track

    private struct Frame { let bucket: TimelineLaneLayout.ShotBucket; let shown: String }

    private func track(width: CGFloat) -> some View {
        let layout = self.layout
        let items = TimelineLaneLayout.screenItems(scene.episodes, viewport: viewport, width: width)
        let icons = TimelineLaneLayout.icons(items, width: width)
        let groups = TimelineLaneLayout.promptGroups(prompts.filter { $0.kind != .work }.map { ($0.id, $0.start) }, viewport: viewport, width: width)
        let frameHeight = layout.height(.shots) - 10
        let frames = layout.showsFilmstrip
            ? TimelineLaneLayout.shotBuckets(scene.activities, viewport: viewport, width: width, frameWidth: (frameHeight * 1.6).rounded())
            : []
        let shown = viewport, collapsed = self.collapsed
        return ZStack(alignment: .topLeading) {
            PlayheadBackLayer(scene: scene, cursor: cursor, viewport: shown, height: layout.lanesHeight)
            Canvas { context, size in
                draw(in: &context, size: size, layout: layout, items: items)
            }
            // Recordings already played: the same waveform at full strength, revealed up to the playhead by a mask,
            // so moving the playhead never re-rasterizes the lanes.
            if !collapsed.contains(.audio), !scene.audio.isEmpty {
                Canvas { context, size in drawPlayedWaveforms(in: &context, size: size, layout: layout) }
                    .mask(alignment: .topLeading) { PlayedMask(cursor: cursor, viewport: shown) }
                    .allowsHitTesting(false)
            }
            PlayheadFrontLayer(scene: scene, cursor: cursor, viewport: shown, layout: layout, collapsed: collapsed, items: items)
            screenOverlays(items: items, icons: icons, layout: layout)
            if !collapsed.contains(.shots) { shotOverlays(frames: frames, frameHeight: frameHeight, layout: layout, width: width) }
            if !collapsed.contains(.ai) { promptOverlays(groups: groups, layout: layout) }
            if !collapsed.contains(.audio) { audioOverlays(layout: layout, width: width) }
            PlayheadMarker(cursor: cursor, viewport: shown, width: width, lanesHeight: layout.lanesHeight,
                           onReveal: { reveal(animated: true) },
                           onAdjust: { step in onScrub(); playhead = clamp(playhead.addingTimeInterval(step)) })
            HoverLine(cursor: cursor, hidden: dragging, height: layout.lanesHeight)
        }
        .frame(width: width, height: trackHeight, alignment: .topLeading)
        .clipped()
        .contentShape(Rectangle())
        .overlay(alignment: .bottomLeading) {
            HoverAnchor(cursor: cursor, width: width, cardWidth: 260, card: peek)
        }
        .onContinuousHover(coordinateSpace: .local) { phase in
            switch phase {
            case .active(let point):
                cursor.hover = point
                let target = hit(point, layout: layout, items: items, icons: icons, frames: frames, groups: groups)
                if target != hovered {
                    withAnimation(Motion.snappy) { hovered = target }
                    if case .prompt(let id) = target { onPeekPrompt(id) }
                }
                let shot = skim(point, frames: frames, target: target, frameWidth: (frameHeight * 1.6).rounded())
                if shot != skimmed { skimmed = shot }
                installWheelMonitor()
            case .ended:
                cursor.hover = nil
                if skimmed != nil { skimmed = nil }
                if hovered != nil { withAnimation(Motion.snappy) { hovered = nil } }
                removeWheelMonitor()
            }
        }
        .gesture(scrubGesture(layout: layout, items: items, icons: icons, frames: frames, groups: groups))
        .simultaneousGesture(MagnifyGesture()
            .onChanged { value in
                let factor = value.magnification / pinchBase; pinchBase = value.magnification
                if !browsing { browsing = true }
                if !interacting { interacting = true }
                viewport.zoom(by: factor, anchorX: hover?.x ?? viewport.x(playhead), width: width, bounds: scene.bounds)
            }
            .onEnded { _ in pinchBase = 1; interacting = false })
        .contextMenu { emptyMenu }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(L("时间轴"))
    }

    @ViewBuilder private var emptyMenu: some View {
        let date = hover.map { clamp(viewport.date(atX: $0.x)) } ?? cursor.latestPlayhead
        Button(L("从 %@ 播放", date.clockText)) { onScrub(); playhead = date; playing = true }
        Button(L("放大到这一小时")) {
            var next = TimelineViewport(start: date.addingTimeInterval(-1800), secondsPerPoint: 3600 / Double(max(1, width)))
            next.clamp(width: width, bounds: scene.bounds)
            browsing = true
            withAnimation(Motion.spring) { viewport = next }
        }
        Button(L("显示整天")) { browsing = true; withAnimation(Motion.spring) { viewport = .fitting(scene.bounds, width: width) } }
        if isToday && !followingNow { Button(L("回到现在"), action: onReturnToNow) }
        Divider() // style-exempt: native context menu separator
        laneMenu
    }

    // MARK: Drawing

    /// Everything on the lanes that does not depend on the playhead; the playhead layers draw the rest.
    private func draw(in context: inout GraphicsContext, size: CGSize, layout: TimelineLaneLayout, items: [TimelineLaneLayout.ScreenItem]) {
        let lanesHeight = layout.lanesHeight
        let soft: CGFloat = 3, small = Radius.small
        // Ticks through every lane, labels on the axis.
        let step = TimelineLayout.tickStep(secondsPerPoint: viewport.secondsPerPoint)
        var t = (viewport.start.timeIntervalSince1970 / step).rounded(.up) * step
        while t <= viewport.end(width: size.width).timeIntervalSince1970 {
            let date = Date(timeIntervalSince1970: t), x = viewport.x(date)
            context.fill(Path(CGRect(x: x, y: 0, width: 0.5, height: lanesHeight)), with: .color(Surface.hairline))
            context.draw(Text(date.clockText).typography(.caption, digits: true).foregroundStyle(.tertiary),
                         at: CGPoint(x: x, y: lanesHeight + TimelineLaneLayout.axisHeight / 2), anchor: .center)
            t += step
        }
        for lane in TimelineLane.allCases {
            let top = layout.top(lane), height = layout.height(lane)
            if lane != .audio { context.fill(Path(CGRect(x: 0, y: top + height - 0.5, width: size.width, height: 0.5)), with: .color(Surface.hairline)) }
            if collapsed.contains(lane) {
                var path = Path(); path.move(to: CGPoint(x: 0, y: top + height / 2)); path.addLine(to: CGPoint(x: size.width, y: top + height / 2))
                context.stroke(path, with: .color(Surface.selected.color), style: StrokeStyle(lineWidth: 1, dash: [2, 4]))
            }
        }
        if isToday {
            let now = viewport.x(Date())
            if now < size.width {
                context.fill(Path(CGRect(x: max(0, now), y: 0, width: size.width - max(0, now), height: lanesHeight)), with: .color(Surface.wash.color))
                var path = Path(); path.move(to: CGPoint(x: now, y: 0)); path.addLine(to: CGPoint(x: now, y: lanesHeight))
                context.stroke(path, with: .color(.secondary.opacity(0.5)), style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
            }
        }

        // Screen: a thin bar per episode, notches for interruptions, striped clusters.
        if !collapsed.contains(.screen) {
            let barY = layout.top(.screen) + layout.height(.screen) - 11
            for item in items {
                let w = max(3, item.x1 - item.x0 - 2)
                let rect = CGRect(x: item.x0 + 1, y: barY, width: w, height: 6)
                if item.isCluster {
                    let total = item.episodes.reduce(0.0) { $0 + scene.episodes[$1].duration }
                    var x = rect.minX, stripes = context
                    stripes.clip(to: Path(roundedRect: rect, cornerRadius: soft))
                    for index in item.episodes {
                        let episode = scene.episodes[index], part = rect.width * CGFloat(episode.duration / max(1, total))
                        stripes.fill(Path(CGRect(x: x, y: rect.minY, width: part, height: rect.height)), with: .color(AppTint.color(for: episode.bundleID)))
                        x += part
                    }
                } else {
                    let episode = scene.episodes[item.episodes[0]]
                    let bar = rect
                    context.fill(Path(roundedRect: bar, cornerRadius: soft), with: .color(AppTint.color(for: episode.bundleID)))
                    for cut in episode.interruptions {
                        let n0 = viewport.x(cut.start), n1 = max(n0 + 3, viewport.x(cut.end))
                        guard n1 - n0 >= 1 else { continue }
                        let notch = CGRect(x: n0, y: bar.minY - 2, width: n1 - n0, height: bar.height + 4)
                        context.fill(Path(roundedRect: notch.insetBy(dx: -1.5, dy: -1.5), cornerRadius: soft), with: .color(Color(nsColor: .windowBackgroundColor)))
                        context.fill(Path(roundedRect: notch, cornerRadius: soft), with: .color(AppTint.color(for: cut.bundleID)))
                    }
                }
            }
            // Markers the user placed while recording.
            for span in scene.spans where span.kind == .marker {
                let x = viewport.x(span.start), y = barY + 3
                guard x >= -6, x <= size.width + 6 else { continue }
                let diamond = Path { p in
                    p.move(to: CGPoint(x: x, y: y - 5)); p.addLine(to: CGPoint(x: x + 5, y: y))
                    p.addLine(to: CGPoint(x: x, y: y + 5)); p.addLine(to: CGPoint(x: x - 5, y: y)); p.closeSubpath()
                }
                context.fill(diamond, with: .color(Color(nsColor: .windowBackgroundColor)))
                context.stroke(diamond, with: .color(.accentColor), lineWidth: 1.5)
            }
        }

        // Screenshots: dots when the lane is short; frames are views.
        if !collapsed.contains(.shots), !layout.showsFilmstrip {
            let y = layout.top(.shots) + layout.height(.shots) / 2
            for dot in TimelineLaneLayout.shotDots(scene.activities, viewport: viewport, width: size.width) {
                let tint = scene.spans.first { $0.id == dot.id }.map { AppTint.color(for: $0.bundleID) } ?? .gray
                context.fill(Path(roundedRect: CGRect(x: dot.x - 5, y: y - 5, width: 10, height: 10), cornerRadius: soft), with: .color(tint))
            }
            for image in scene.images {
                let x = viewport.x(image.date)
                guard x >= -6, x <= size.width + 6 else { continue }
                context.stroke(Path(roundedRect: CGRect(x: x - 5, y: y - 5, width: 10, height: 10), cornerRadius: soft), with: .color(MediaGlyph.tint(.image)), lineWidth: 2)
            }
        }

        // AI instructions are moments; only the hovered or opened one shows how long the work after it ran.
        if !collapsed.contains(.ai) {
            let y = layout.top(.ai) + layout.height(.ai) - 9
            let active: Set<String> = Set([hovered, opened].compactMap { target -> String? in if case .prompt(let id) = target { return id }; return nil })
            for work in prompts where work.kind != .work && active.contains(work.id) && work.hasWork {
                let x0 = viewport.x(work.start), x1 = viewport.x(work.workEnd ?? Date())
                guard x1 >= 0, x0 <= size.width else { continue }
                let rect = CGRect(x: x0, y: y, width: max(4, x1 - x0), height: 5)
                context.fill(Path(roundedRect: rect, cornerRadius: soft),
                             with: .linearGradient(Gradient(colors: AIGradient.colors.map { $0.opacity(0.35) }),
                                                   startPoint: CGPoint(x: rect.minX, y: y), endPoint: CGPoint(x: rect.maxX, y: y)))
            }
        }

        // Recordings: speech drawn as a waveform that fills as the playhead passes.
        if !collapsed.contains(.audio) {
            let top = layout.top(.audio) + 4, height = layout.height(.audio) - 8, mid = top + height / 2
            let tint = MediaGlyph.tint(.audio)
            for audio in scene.audio {
                let x0 = viewport.x(audio.start), x1 = viewport.x(audio.end)
                guard x1 >= 0, x0 <= size.width else { continue }
                context.fill(Path(roundedRect: CGRect(x: x0, y: top, width: max(4, x1 - x0), height: height), cornerRadius: small - 2),
                             with: .color(tint.opacity(0.14)))
                for speech in audio.speech {
                    let s0 = max(x0, viewport.x(speech.lowerBound)), s1 = max(s0 + 1.5, viewport.x(speech.upperBound))
                    var x = max(s0, -3)
                    while x < min(s1, size.width + 3) {
                        // Deterministic bar heights from position, so redraws never flicker.
                        let bar = Self.waveBar(x: x, audio: audio, mid: mid, height: height)
                        context.fill(Path(roundedRect: bar, cornerRadius: bar.width / 2), with: .color(tint.opacity(0.55)))
                        x += 3
                    }
                }
            }
        }
    }

    private func drawPlayedWaveforms(in context: inout GraphicsContext, size: CGSize, layout: TimelineLaneLayout) {
        let top = layout.top(.audio) + 4, height = layout.height(.audio) - 8, mid = top + height / 2
        let tint = MediaGlyph.tint(.audio)
        for audio in scene.audio {
            let x0 = viewport.x(audio.start), x1 = viewport.x(audio.end)
            guard x1 >= 0, x0 <= size.width else { continue }
            for speech in audio.speech {
                let s0 = max(x0, viewport.x(speech.lowerBound)), s1 = max(s0 + 1.5, viewport.x(speech.upperBound))
                var x = max(s0, -3)
                while x < min(s1, size.width + 3) {
                    let bar = Self.waveBar(x: x, audio: audio, mid: mid, height: height)
                    context.fill(Path(roundedRect: bar, cornerRadius: bar.width / 2), with: .color(tint))
                    x += 3
                }
            }
        }
    }

    /// Deterministic bar heights from position, so redraws never flicker; the played layer redraws the same bars.
    static func waveBar(x: CGFloat, audio: TimelineAudioSpan, mid: CGFloat, height: CGFloat) -> CGRect {
        let h = (0.25 + 0.75 * abs(sin(Double(x) * 0.37 + audio.start.timeIntervalSince1970.truncatingRemainder(dividingBy: 97)))) * (height - 6)
        return CGRect(x: x, y: mid - h / 2, width: 1.6, height: h)
    }

    // MARK: Overlays (icons, frames, prompts) — views so they can carry menus

    @ViewBuilder private func screenOverlays(items: [TimelineLaneLayout.ScreenItem], icons: [TimelineLaneLayout.Icon], layout: TimelineLaneLayout) -> some View {
        if !collapsed.contains(.screen) {
            let top = layout.top(.screen), barY = top + layout.height(.screen) - 11
            ForEach(items.indices, id: \.self) { index in
                let item = items[index], episode = scene.episodes[item.episodes[0]]
                let w = item.x1 - item.x0
                // Invisible hit target so each episode keeps its own menu.
                Color.clear.frame(width: max(8, w), height: 18).contentShape(Rectangle())
                    .offset(x: item.x0, y: barY - 6)
                    .contextMenu { screenMenu(item) }
            }
            ForEach(TimelineLaneLayout.screenLabels(items, icons: icons, width: width), id: \.item) { label in
                let item = items[label.item], episode = scene.episodes[item.episodes[0]]
                Text(item.isCluster ? L("%@ 次切换 · %@ 个应用", item.episodes.count, Set(item.episodes.map { scene.episodes[$0].bundleID }).count) : episode.title)
                    .typography(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.tail)
                    .frame(width: label.width, alignment: .leading).clipped()
                    .offset(x: label.x, y: barY - 17)
                    .allowsHitTesting(false).accessibilityHidden(true)
            }
            ForEach(icons, id: \.item) { icon in
                let item = items[icon.item], episode = scene.episodes[item.episodes[0]]
                let target = target(item)
                let lifted = hovered == target || opened == target
                VStack(spacing: 0) {
                    ZStack(alignment: .topTrailing) {
                        DesktopApplicationIcon(bundleID: episode.bundleID, fallback: "app", size: 22)
                            .shadow(color: .black.opacity(0.16), radius: 2.5, y: 1)
                        if icon.extra > 0 {
                            Text("+\(icon.extra)").typography(.caption, .semibold).monospacedDigit()
                                .padding(.horizontal, 3).frame(minWidth: 16, minHeight: 14)
                                .background(Capsule().fill(.regularMaterial)).offset(x: 10, y: -6)
                        }
                    }
                    .scaleEffect(lifted ? 1.18 : 1, anchor: .bottom)
                    Rectangle().fill(Color.secondary.opacity(0.5)).frame(width: 1, height: 6)
                }
                .frame(width: 22)
                .position(x: icon.x, y: barY - 15)
                .animation(Motion.magnify, value: lifted)
                .contextMenu { screenMenu(item) }
                .accessibilityLabel(episode.appName + " " + episode.title)
            }
        }
    }

    @ViewBuilder private func screenMenu(_ item: TimelineLaneLayout.ScreenItem) -> some View {
        if item.isCluster {
            let first = scene.episodes[item.episodes[0]], last = scene.episodes[item.episodes[item.episodes.count - 1]]
            Text(L("%@ 次切换 · %@–%@", item.episodes.count, first.start.clockText, last.end.clockText))
            Button(L("放大到这一段")) { zoomTo(first.start, last.end) }
            Button(L("从这里播放")) { onScrub(); playhead = first.start.addingTimeInterval(1); playing = true }
        } else {
            let episode = scene.episodes[item.episodes[0]]
            Button(L("打开详情")) { onOpen(.episode(episode.id)) }
            Button(L("从这里播放")) { onScrub(); playhead = episode.start.addingTimeInterval(1); playing = true }
            Divider() // style-exempt: native context menu separator
            contextActions(episode)
        }
    }

    @ViewBuilder private func shotOverlays(frames: [TimelineLaneLayout.ShotBucket], frameHeight: CGFloat, layout: TimelineLaneLayout, width: CGFloat) -> some View {
        let top = layout.top(.shots)
        if layout.showsFilmstrip {
            let frameWidth = (frameHeight * 1.6).rounded()
            ForEach(frames, id: \.id) { bucket in
                let shown = (skimmed.flatMap { id in bucket.spans.contains(id) ? id : nil }) ?? bucket.spans[0]
                let span = scene.spans.first { $0.id == shown }
                let on = hovered == .shot(shown) || bucket.spans.contains { opened == .shot($0) }
                ZStack(alignment: .topTrailing) {
                    ShotFrameImage(span: span, cache: cache)
                        .frame(width: frameWidth, height: frameHeight).clipped()
                    if bucket.spans.count > 1 {
                        Text("\(bucket.spans.count)").typography(.caption, .semibold).monospacedDigit().foregroundStyle(.white)
                            .padding(.horizontal, 4).background(Capsule().fill(.black.opacity(0.5))).padding(3)
                    }
                }
                .overlay(alignment: .bottomLeading) {
                    if bucket.spans.count > 1, let index = bucket.spans.firstIndex(of: shown) {
                        Capsule().fill(Color.accentColor).frame(width: 5, height: 3)
                            .offset(x: (frameWidth - 5) * CGFloat(index) / CGFloat(max(1, bucket.spans.count - 1)), y: -1)
                    }
                }
                .clipShape(RoundedRectangle(cornerRadius: Radius.small - 2, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: Radius.small - 2, style: .continuous)
                    .strokeBorder(on ? Color.accentColor : Surface.hairline, lineWidth: on ? 1.5 : 0.5))
                .shadow(color: .black.opacity(0.08), radius: 2, y: 1)
                .offset(x: bucket.x, y: top + 5 - (on ? 2 : 0))
                .animation(Motion.snappy, value: on)
                .contextMenu { shotMenu(shown) }
            }
            ForEach(scene.images) { image in
                let x = viewport.x(image.date)
                if x >= -10, x <= width + 10 {
                    MediaGlyph(kind: .image, size: 16).position(x: x, y: top + layout.height(.shots) - 10)
                        .contextMenu { if let episode = scene.episodes.first(where: { $0.mediaID == image.id }) { contextActions(episode) } }
                }
            }
        } else {
            ForEach(TimelineLaneLayout.shotDots(scene.activities, viewport: viewport, width: width), id: \.id) { dot in
                Color.clear.frame(width: 12, height: 14).contentShape(Rectangle())
                    .position(x: dot.x, y: top + layout.height(.shots) / 2)
                    .contextMenu { shotMenu(dot.id) }
            }
        }
    }

    @ViewBuilder private func shotMenu(_ spanID: String) -> some View {
        Button(L("打开详情")) { onOpen(.shot(spanID)) }
        if let episode = scene.episodes.first(where: { $0.spans.contains { $0.id == spanID } || $0.interruptions.contains { $0.id == spanID } }) {
            Divider() // style-exempt: native context menu separator
            contextActions(episode)
        }
    }

    @ViewBuilder private func promptOverlays(groups: [TimelineLaneLayout.PromptGroup], layout: TimelineLaneLayout) -> some View {
        let top = layout.top(.ai)
        // Identity is the instruction, not the position, so panning slides a point instead of recreating it.
        ForEach(groups, id: \.ids) { group in
            let mark = prompts.first { $0.id == group.ids[0] }
            let on = group.ids.contains { hovered == .prompt($0) || opened == .prompt($0) }
            HStack(spacing: 4) {
                ZStack(alignment: .topTrailing) {
                    Circle().fill(LinearGradient(colors: AIGradient.colors, startPoint: .leading, endPoint: .trailing))
                        .overlay(Image(systemName: mark?.kind == .note ? "note.text" : "text.bubble.fill").typography(.caption).imageScale(.small).foregroundStyle(.white))
                        .frame(width: 16, height: 16)
                        .overlay(Circle().strokeBorder(on ? Color.accentColor : .clear, lineWidth: 1.5).padding(-2))
                    if group.ids.count > 1 {
                        Text("\(group.ids.count)").typography(.caption, .semibold).monospacedDigit()
                            .padding(.horizontal, 3).background(Capsule().fill(.regularMaterial)).offset(x: 9, y: -7)
                    }
                }
                if layout.showsPromptText, group.room > 50, let text = mark?.preview, !text.isEmpty {
                    Text(text).typography(.caption).foregroundStyle(.secondary).lineLimit(1)
                        .frame(maxWidth: min(180, group.room), alignment: .leading)
                }
            }
            .fixedSize()
            // Scaled around the point itself (before the offset), so highlighting never moves it along the lane.
            .scaleEffect(on ? 1.06 : 1, anchor: .leading)
            .animation(Motion.snappy, value: on)
            .offset(x: group.x - 8, y: top + 4)
            .contextMenu { if let mark { promptActions(mark) } }
        }
    }

    @ViewBuilder private func audioOverlays(layout: TimelineLaneLayout, width: CGFloat) -> some View {
        let top = layout.top(.audio), height = layout.height(.audio)
        ForEach(scene.audio) { audio in
            let x0 = viewport.x(audio.start), x1 = viewport.x(audio.end)
            if x1 >= 0, x0 <= width, let episode = scene.episodes.first(where: { $0.audio?.id == audio.id }) {
                let on = opened == .episode(episode.id) || hovered == .episode(episode.id)
                ZStack(alignment: .topLeading) {
                    RoundedRectangle(cornerRadius: Radius.small - 2, style: .continuous)
                        .strokeBorder(on ? Color.accentColor : .clear, lineWidth: 1.5)
                    if x1 - x0 > 70, height > 34 {
                        Text(audio.title).typography(.caption, .semibold).lineLimit(1).padding(.leading, 6).padding(.top, 2)
                    }
                }
                .frame(width: max(6, x1 - x0), height: height - 8)
                .contentShape(Rectangle())
                .offset(x: x0, y: top + 4)
                .contextMenu {
                    Button(L("打开详情")) { onOpen(.episode(episode.id)) }
                    Button(L("从这里播放")) { onScrub(); playhead = audio.start; playing = true }
                    Divider() // style-exempt: native context menu separator
                    contextActions(episode)
                }
            }
        }
    }

    /// Moves the view only when the playhead is outside it (or hugging an edge); a click on something already
    /// visible never pans the timeline.
    private func reveal(animated: Bool) {
        guard width > 0 else { return }
        browsing = false
        let x = viewport.x(playhead)
        guard x < 12 || x > width - 12 else { return }
        var next = viewport
        guard next.reveal(playhead, width: width, margin: width * 0.25, bounds: scene.bounds) else { return }
        if animated { withAnimation(Motion.spring) { viewport = next } } else { viewport = next }
    }

    // MARK: Hover card

    @ViewBuilder private var peek: some View {
        if let hovered, !dragging {
            let cardWidth: CGFloat = 260
            Group {
                switch hovered {
                case .episode(let id):
                    if let episode = scene.episodes.first(where: { $0.id == id }) { EpisodePeek(episode: episode, cache: cache) }
                case .cluster(let ids):
                    ClusterPeek(episodes: ids.compactMap { id in scene.episodes.first { $0.id == id } })
                case .shot(let id):
                    if let span = scene.spans.first(where: { $0.id == id }) { ShotPeek(span: span, cache: cache) }
                case .prompt(let id):
                    if let mark = prompts.first(where: { $0.id == id }) { PromptPeek(mark: mark) }
                }
            }
            .frame(width: cardWidth)
            // Bottom edge sits above the controls row, never over the lanes.
            .offset(y: -(trackHeight + 2 + 32 + 14))
            .transition(.scale(scale: 0.94, anchor: .bottom).combined(with: .opacity))
            .id(hovered)
            .allowsHitTesting(false)
        }
    }

    // MARK: Hit testing

    private func target(_ item: TimelineLaneLayout.ScreenItem) -> TimelineTarget {
        item.isCluster ? .cluster(item.episodes.map { scene.episodes[$0].id }) : .episode(scene.episodes[item.episodes[0]].id)
    }

    private func lane(at y: CGFloat, layout: TimelineLaneLayout) -> TimelineLane? {
        TimelineLane.allCases.first { y >= layout.top($0) && y < layout.top($0) + layout.height($0) && !collapsed.contains($0) }
    }

    private func hit(_ point: CGPoint, layout: TimelineLaneLayout, items: [TimelineLaneLayout.ScreenItem], icons: [TimelineLaneLayout.Icon],
                     frames: [TimelineLaneLayout.ShotBucket], groups: [TimelineLaneLayout.PromptGroup]) -> TimelineTarget? {
        switch lane(at: point.y, layout: layout) {
        case .screen:
            let barY = layout.top(.screen) + layout.height(.screen) - 11
            if point.y < barY - 4, let icon = icons.min(by: { abs($0.x - point.x) < abs($1.x - point.x) }), abs(icon.x - point.x) < 13 {
                return target(items[icon.item])
            }
            return items.first { point.x >= $0.x0 && point.x <= max($0.x1, $0.x0 + 8) }.map(target)
        case .shots:
            if layout.showsFilmstrip {
                let frameWidth = ((layout.height(.shots) - 10) * 1.6).rounded()
                guard let bucket = frames.first(where: { point.x >= $0.x && point.x <= $0.x + frameWidth }) else { return nil }
                return .shot(bucket.spans[0])
            }
            let dots = TimelineLaneLayout.shotDots(scene.activities, viewport: viewport, width: width)
            return dots.min { abs($0.x - point.x) < abs($1.x - point.x) }.flatMap { abs($0.x - point.x) < 7 ? .shot($0.id) : nil }
        case .ai:
            if let group = groups.min(by: { abs($0.x - point.x) < abs($1.x - point.x) }), abs(group.x - point.x) < 10 { return .prompt(group.ids[0]) }
            return nil
        case .audio:
            let date = viewport.date(atX: point.x)
            return scene.episodes.first { $0.kind == .audio && $0.contains(date) }.map { .episode($0.id) }
        case nil:
            return nil
        }
    }

    /// Inside a filmstrip frame, the pointer position picks one of its screenshots.
    private func skim(_ point: CGPoint, frames: [TimelineLaneLayout.ShotBucket], target: TimelineTarget?, frameWidth: CGFloat) -> String? {
        guard case .shot = target, let bucket = frames.first(where: { point.x >= $0.x && point.x <= $0.x + frameWidth }) else { return nil }
        let index = Int(((point.x - bucket.x) / max(1, frameWidth) * CGFloat(bucket.spans.count)).rounded(.down))
        return bucket.spans[min(max(0, index), bucket.spans.count - 1)]
    }

    // MARK: Interaction

    private func scrubGesture(layout: TimelineLaneLayout, items: [TimelineLaneLayout.ScreenItem], icons: [TimelineLaneLayout.Icon],
                              frames: [TimelineLaneLayout.ShotBucket], groups: [TimelineLaneLayout.PromptGroup]) -> some Gesture {
        DragGesture(minimumDistance: 0)
            .onChanged { value in
                if !dragging && pressed == nil, value.translation == .zero {
                    pressed = hit(value.startLocation, layout: layout, items: items, icons: icons, frames: frames, groups: groups)
                    if case .shot = pressed, let skimmed { pressed = .shot(skimmed) }
                }
                if pressed != nil, hypot(value.translation.width, value.translation.height) < 4 { return }
                pressed = nil
                if !dragging { dragging = true; interacting = true; playing = false; browsing = false; withAnimation(Motion.snappy) { hovered = nil } }
                onScrub()
                let x = value.location.x
                if x < 24 { viewport.pan(points: -8, width: width, bounds: scene.bounds) }
                else if x > width - 24 { viewport.pan(points: 8, width: width, bounds: scene.bounds) }
                playhead = clamp(viewport.date(atX: min(max(0, x), width)))
            }
            .onEnded { value in
                if let target = pressed {
                    playing = false
                    activate(target)
                } else if !dragging, lane(at: value.location.y, layout: layout) != nil || value.location.y >= layout.lanesHeight {
                    onScrub(); withAnimation(Motion.snappy) { playhead = clamp(viewport.date(atX: value.location.x)) }
                }
                pressed = nil
                if dragging { dragging = false; interacting = false }
            }
    }

    private func activate(_ target: TimelineTarget) {
        switch target {
        case .cluster(let ids):
            let episodes = ids.compactMap { id in scene.episodes.first { $0.id == id } }
            if let first = episodes.first, let last = episodes.last { zoomTo(first.start, last.end) }
        case .prompt:
            onOpen(target)
        case .episode, .shot:
            // With a detail open, clicks switch it; otherwise the first click locates and a second one opens.
            if opened != nil || isLocated(target) { onOpen(target) } else { onLocate(target) }
        }
    }

    private func isLocated(_ target: TimelineTarget) -> Bool {
        switch target {
        case .episode(let id): return scene.episodes.first { $0.id == id }?.contains(playhead) ?? false
        case .shot(let id): return scene.activity(at: playhead)?.id == id
        default: return false
        }
    }

    private func zoomTo(_ start: Date, _ end: Date) {
        guard width > 0 else { return }
        let span = max(20 * 60, end.timeIntervalSince(start) + 6 * 60)
        var next = TimelineViewport(start: start.addingTimeInterval(-180), secondsPerPoint: span / Double(width))
        next.clamp(width: width, bounds: scene.bounds)
        onScrub(); browsing = true
        withAnimation(Motion.spring) { viewport = next; playhead = clamp(start.addingTimeInterval(1)) }
    }

    private func clamp(_ date: Date) -> Date {
        let upper = isToday ? min(scene.bounds.upperBound, Date()) : scene.bounds.upperBound
        return min(max(date, scene.bounds.lowerBound), max(upper, scene.bounds.lowerBound))
    }

    /// First layout shows the last three hours around the playhead; later
    /// layouts only keep the user's pan/zoom inside the day's bounds.
    private func prepareViewport() {
        guard width > 0 else { return }
        if !viewportReady {
            viewport = .showing(3 * 3600, endingAt: playhead.addingTimeInterval(1200), width: width, bounds: scene.bounds)
            viewportReady = true
        } else {
            viewport.clamp(width: width, bounds: scene.bounds)
        }
    }

    private func zoom(_ factor: Double) {
        var next = viewport
        next.zoom(by: factor, anchorX: next.x(playhead), width: width, bounds: scene.bounds)
        browsing = true
        withAnimation(Motion.spring) { viewport = next }
    }

    /// Wheel pans, ⌘-wheel zooms; only while the pointer is over the track.
    private func installWheelMonitor() {
        guard wheelMonitor == nil else { return }
        wheelMonitor = NSEvent.addLocalMonitorForEvents(matching: .scrollWheel) { event in
            MainActor.assumeIsolated {
                guard cursor.latestHover != nil, width > 0 else { return }
                if !browsing { browsing = true }
                if !interacting { interacting = true }
                wheelGeneration &+= 1
                let generation = wheelGeneration
                Task { @MainActor in
                    try? await Task.sleep(for: .milliseconds(300))
                    if wheelGeneration == generation { interacting = false }
                }
                let dx = abs(event.scrollingDeltaX) > abs(event.scrollingDeltaY) ? event.scrollingDeltaX : event.scrollingDeltaY
                if event.modifierFlags.contains(.command) {
                    viewport.zoom(by: exp(Double(event.scrollingDeltaY) * 0.01), anchorX: hover?.x ?? width / 2, width: width, bounds: scene.bounds)
                } else {
                    viewport.pan(points: -dx * (event.hasPreciseScrollingDeltas ? 1 : 8), width: width, bounds: scene.bounds)
                }
            }
            return nil
        }
    }

    private func removeWheelMonitor() {
        if let wheelMonitor { NSEvent.removeMonitor(wheelMonitor) }
        wheelMonitor = nil
    }
}

/// Colors of the AI mark; the gradient itself lives in Palette.ai.
enum AIGradient {
    static let colors = [Color(red: 0.42, green: 0.4, blue: 1), Color(red: 0.86, green: 0.36, blue: 0.86), Color(red: 1, green: 0.55, blue: 0.4)]
}

/// Brief hover card: latest screenshot, title, app and time range.
struct EpisodePeek: View {
    let episode: TimelineEpisode
    let cache: TimelineMediaCache

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if episode.kind == .audio, let line = episode.audio?.lines.first {
                Text(line.text).typography(.body).lineLimit(3).foregroundStyle(.secondary)
                    .padding([.horizontal, .top], 12)
            }
            if episode.kind == .image, let id = episode.mediaID, let image = cache.mediaImage(id: id) {
                Image(nsImage: image).resizable().aspectRatio(contentMode: .fit).frame(maxHeight: 136)
                    .clipShape(RoundedRectangle(cornerRadius: Radius.small, style: .continuous)).padding([.horizontal, .top], 8)
            }
            if let span = episode.screenshotSpans.last {
                ZStack {
                    Rectangle().fill(Surface.fill.color)
                    if let image = cache.latestImage(for: span) {
                        Image(nsImage: image).resizable().interpolation(.high).aspectRatio(contentMode: .fill)
                            .transition(.opacity)
                    }
                }
                .frame(height: 136).clipped()
                .clipShape(RoundedRectangle(cornerRadius: Radius.small, style: .continuous))
                .padding([.horizontal, .top], 8)
                .animation(.easeOut(duration: 0.2), value: cache.latestImage(for: span) != nil)
            }
            HStack(spacing: 9) {
                if episode.kind == .app { DesktopApplicationIcon(bundleID: episode.bundleID, fallback: "app", size: 22) }
                else { MediaGlyph(kind: episode.kind, size: 22) }
                VStack(alignment: .leading, spacing: 2) {
                    Text(episode.title).typography(.body, .semibold).lineLimit(1)
                    Text(subtitle).typography(.secondary).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            .padding(10)
        }
        .peekSurface()
    }

    private var subtitle: String {
        var text = "\(episode.appName) · \(episode.start.clockText) – \(episode.end.clockText) · \(episode.duration.spokenDuration)"
        if !episode.interruptions.isEmpty { text += L(" · 中途切换 %@ 次", episode.interruptions.count) }
        return text
    }
}

/// A merged run of short episodes: which apps, in order.
struct ClusterPeek: View {
    let episodes: [TimelineEpisode]
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let first = episodes.first, let last = episodes.last {
                Text(L("%@ 次切换", episodes.count)).typography(.headline)
                Text("\(first.start.clockText) – \(last.end.clockText)").typography(.secondary, digits: true).foregroundStyle(.secondary)
            }
            ForEach(episodes.prefix(8)) { episode in
                HStack(spacing: 8) {
                    DesktopApplicationIcon(bundleID: episode.bundleID, fallback: "app", size: 16)
                    Text(episode.title).typography(.secondary).lineLimit(1)
                    Spacer(minLength: 4)
                    Text(episode.duration.spokenDuration).typography(.caption, digits: true).foregroundStyle(.tertiary)
                }
            }
            if episodes.count > 8 { Text(L("另有 %@ 段", episodes.count - 8)).typography(.caption).foregroundStyle(.tertiary) }
            Text(L("单击放大到这一段")).typography(.caption).foregroundStyle(.tertiary)
        }
        .padding(12)
        .peekSurface()
    }
}

/// One screenshot-bearing moment under the pointer.
struct ShotPeek: View {
    let span: TimelineSpan
    let cache: TimelineMediaCache
    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ZStack {
                Rectangle().fill(Surface.fill.color)
                if let image = cache.latestImage(for: span) {
                    Image(nsImage: image).resizable().interpolation(.high).aspectRatio(contentMode: .fill)
                }
            }
            .frame(height: 150).clipped()
            .clipShape(RoundedRectangle(cornerRadius: Radius.small, style: .continuous))
            .padding([.horizontal, .top], 8)
            HStack(spacing: 8) {
                DesktopApplicationIcon(bundleID: span.bundleID, fallback: "app", size: 18)
                Text(span.title).typography(.secondary, .semibold).lineLimit(1)
                Spacer(minLength: 4)
                Text(span.start.clockText).typography(.caption, digits: true).foregroundStyle(.secondary)
            }
            .padding(10)
        }
        .peekSurface()
    }
}

/// An AI instruction, work stretch or note under the pointer.
struct PromptPeek: View {
    let mark: TimelinePromptMark
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(mark.source?.agent ?? (mark.kind == .note ? L("笔记") : L("AI 指令"))).typography(.caption, .semibold)
                    .padding(.horizontal, 6).padding(.vertical, 1)
                    .background(Capsule().fill(Surface.fill.color))
                if let project = mark.source?.project { Text(project).typography(.caption).foregroundStyle(.secondary).lineLimit(1) }
            }
            // The session name gets its own line and may wrap, so a long title is never cut to a few characters.
            if let session = mark.session {
                Text(L("「%@」", session)).typography(.caption).foregroundStyle(.secondary).lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let text = mark.preview, !text.isEmpty {
                Text(text).typography(.body).lineLimit(3).fixedSize(horizontal: false, vertical: true)
            }
            Text(metaLine).typography(.caption, digits: true).foregroundStyle(.tertiary)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .peekSurface()
    }

    private var metaLine: String {
        var parts = [mark.start.clockText]
        if mark.hasWork {
            let end = mark.workEnd ?? Date()
            let minutes = Int((end.timeIntervalSince(mark.start) / 60).rounded())
            parts.append(minutes < 1 ? L("工作不到 1 分钟") : L("工作 %@ 分钟", minutes))
        }
        if let ordinal = mark.ordinal, ordinal > 1 { parts.append(L("今天本会话第 %@ 条", ordinal)) }
        return parts.joined(separator: " · ")
    }
}

private extension View {
    func peekSurface() -> some View {
        background(.regularMaterial, in: RoundedRectangle(cornerRadius: Radius.large, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: Radius.large, style: .continuous).strokeBorder(Surface.hover.color, lineWidth: 0.5))
            .shadow(color: .black.opacity(0.18), radius: 18, y: 10)
    }
}

/// One filmstrip frame. While another screenshot loads (skimming, a live activity growing) the previous image
/// stays up, so the frame never blinks to its background.
private struct ShotFrameImage: View {
    let span: TimelineSpan?
    let cache: TimelineMediaCache
    @ViewState private var shown: NSImage?

    var body: some View {
        let current = span.flatMap { cache.latestImage(for: $0) }
        ZStack {
            LinearGradient(colors: [AppTint.color(for: span?.bundleID ?? "").opacity(0.5), AppTint.color(for: span?.bundleID ?? "").opacity(0.15)],
                           startPoint: .topLeading, endPoint: .bottomTrailing)
            if let image = current ?? shown {
                Image(nsImage: image).resizable().interpolation(.medium).aspectRatio(contentMode: .fill)
            }
        }
        .onChange(of: current) { _, image in if let image { shown = image } }
        .onAppear { if let current { shown = current } }
        .transaction { $0.animation = nil }
    }
}

// MARK: Playhead and hover layers
// Each reads the cursor itself, so moving the playhead or the pointer redraws only these, not the lanes.

/// Runs `onMove` whenever the playhead moves.
private struct PlayheadWatcher: View {
    let cursor: TimelineCursor
    let onMove: (Date) -> Void
    var body: some View {
        Color.clear.onChange(of: cursor.playhead) { _, value in onMove(value) }
    }
}

/// The time under the playhead above the lanes.
private struct PlayheadClock: View {
    let cursor: TimelineCursor
    /// Digits roll on jumps and playback; during a drag they follow the finger directly.
    let animated: Bool
    var body: some View {
        let text = cursor.playhead.clockText
        Text(text)
            .typography(.section, digits: true)
            .contentTransition(animated ? .numericText(value: cursor.playhead.timeIntervalSince1970) : .identity)
            .animation(animated ? Motion.snappy : nil, value: text)
            .accessibilityLabel(L("当前位置 ") + text)
    }
}

/// Wash behind the episode under the playhead, drawn under the lanes. A shape, not a canvas: moving it only
/// moves a layer.
private struct PlayheadBackLayer: View {
    let scene: TimelineScene
    let cursor: TimelineCursor
    let viewport: TimelineViewport
    let height: CGFloat
    var body: some View {
        if let focused = scene.episodeIndex(at: cursor.playhead).map({ scene.episodes[$0] }) {
            let x0 = viewport.x(focused.start), x1 = viewport.x(focused.end)
            RoundedRectangle(cornerRadius: Radius.small, style: .continuous).fill(Surface.wash.color)
                .frame(width: max(6, x1 - x0 + 4), height: height)
                .offset(x: x0 - 2)
                .allowsHitTesting(false)
        }
    }
}

/// The focused episode's bar a little thicker, over the lanes (its interruptions keep their notches).
private struct PlayheadFrontLayer: View {
    let scene: TimelineScene
    let cursor: TimelineCursor
    let viewport: TimelineViewport
    let layout: TimelineLaneLayout
    let collapsed: Set<TimelineLane>
    let items: [TimelineLaneLayout.ScreenItem]
    var body: some View {
        let focused = scene.episodeIndex(at: cursor.playhead).map { scene.episodes[$0] }
        if !collapsed.contains(.screen), let focused, focused.kind == .app,
           let item = items.first(where: { !$0.isCluster && scene.episodes[$0.episodes[0]].id == focused.id }) {
            let barY = layout.top(.screen) + layout.height(.screen) - 11
            let rect = CGRect(x: item.x0 + 1, y: barY, width: max(3, item.x1 - item.x0 - 2), height: 6).insetBy(dx: 0, dy: -1)
            ZStack(alignment: .topLeading) {
                Capsule().fill(AppTint.color(for: focused.bundleID))
                    .frame(width: rect.width, height: rect.height)
                    .offset(x: rect.minX, y: rect.minY)
                ForEach(focused.interruptions) { cut in
                    let n0 = viewport.x(cut.start), n1 = max(n0 + 3, viewport.x(cut.end))
                    Capsule().fill(AppTint.color(for: cut.bundleID))
                        .frame(width: n1 - n0, height: rect.height + 4)
                        .padding(1.5)
                        .background(Capsule().fill(Color(nsColor: .windowBackgroundColor)))
                        .offset(x: n0 - 1.5, y: rect.minY - 3.5)
                }
            }
            .allowsHitTesting(false)
        }
    }
}

/// Reveals the played waveform up to the playhead.
private struct PlayedMask: View {
    let cursor: TimelineCursor
    let viewport: TimelineViewport
    var body: some View {
        Rectangle().frame(width: max(0, viewport.x(cursor.playhead)))
    }
}

/// The playhead line, and a tag on the edge that brings the view back to it when it is out of sight. Also the
/// accessible element for the playhead (adjustable in five-minute steps).
private struct PlayheadMarker: View {
    let cursor: TimelineCursor
    let viewport: TimelineViewport
    let width: CGFloat
    let lanesHeight: CGFloat
    let onReveal: () -> Void
    let onAdjust: (TimeInterval) -> Void

    var body: some View {
        let playhead = cursor.playhead
        let x = viewport.x(playhead)
        ZStack(alignment: .topLeading) {
            ZStack(alignment: .top) {
                Rectangle().fill(Color.primary).frame(width: 1.5, height: lanesHeight)
                Circle().fill(Color.primary).frame(width: 9, height: 9).offset(y: -3)
            }
            .frame(width: 9, height: lanesHeight, alignment: .top)
            .offset(x: x - 4.5)
            .allowsHitTesting(false)
            .accessibilityElement()
            .accessibilityLabel(L("播放头"))
            .accessibilityValue(playhead.clockText)
            .accessibilityAdjustableAction { direction in onAdjust(direction == .increment ? 300 : -300) }
            if width > 0, x < 0 || x > width {
                let leading = x < 0
                Button(action: onReveal) {
                    HStack(spacing: 3) {
                        if leading { Image(systemName: "chevron.left") }
                        Text(playhead.clockText).monospacedDigit()
                        if !leading { Image(systemName: "chevron.right") }
                    }
                    .typography(.caption, .semibold)
                    .padding(.horizontal, 7).frame(height: 20)
                    .background(Capsule().fill(.regularMaterial))
                    .overlay(Capsule().strokeBorder(Surface.hairline, lineWidth: 0.5))
                    .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .help(L("回到播放头所在的位置"))
                .frame(width: width, alignment: leading ? .leading : .trailing)
                .padding(.horizontal, 4)
                .offset(y: max(0, lanesHeight / 2 - 10))
                .transition(.opacity)
            }
        }
    }
}

/// Thin line under the pointer while it is over the lanes.
private struct HoverLine: View {
    let cursor: TimelineCursor
    let hidden: Bool
    let height: CGFloat
    var body: some View {
        if let hover = cursor.hover, !hidden {
            Rectangle().fill(Surface.selected.color).frame(width: 1, height: height)
                .offset(x: hover.x).allowsHitTesting(false)
        }
    }
}

/// Places the hover card horizontally under the pointer; the card itself only changes when the hovered item does.
private struct HoverAnchor<Card: View>: View {
    let cursor: TimelineCursor
    let width: CGFloat
    let cardWidth: CGFloat
    let card: Card
    var body: some View {
        if let anchor = cursor.hover {
            card.offset(x: min(max(anchor.x - cardWidth / 2, -8), width - cardWidth + 8))
        }
    }
}
