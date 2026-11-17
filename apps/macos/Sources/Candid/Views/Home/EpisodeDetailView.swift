import AppKit
import CandidLocalization
import DesktopCore
import SwiftUI

/// One recorded moment inside an episode: what was read, and the screenshot
/// taken for it if any. Built from ledger records; nothing is inferred.
struct EpisodeMoment: Identifiable, Equatable {
    let id: String
    let date: Date
    let eventID: String
    let observation: DesktopRecord?
    let screenshot: DesktopRecord?

    private var context: DesktopContextSnapshot? { observation?.context ?? screenshot?.context }
    var isProtected: Bool { context?.isProtected == true || context?.textStatus == "protected" }
    var text: String? { isProtected ? nil : Self.clean(context?.readableText) }
    var selection: String? { isProtected ? nil : Self.clean(context?.selectedText) }
    var record: DesktopRecord? { observation ?? screenshot }

    private static func clean(_ value: String?) -> String? {
        guard let value = value?.trimmingCharacters(in: .whitespacesAndNewlines), !value.isEmpty else { return nil }
        return value
    }
}

enum EpisodeMoments {
    /// Observations in time order, each carrying its latest screenshot.
    /// Consecutive text-only moments with identical text collapse into one.
    static func build(_ events: [DesktopTimelineEvent]) -> [EpisodeMoment] {
        var moments: [EpisodeMoment] = []
        for event in events {
            let observations = event.observations.filter { $0.kind != .screenshot && $0.kind != .gap }
            let byParent = Dictionary(grouping: event.screenshots.filter { $0.imageHash != nil }, by: { $0.parentID ?? "" })
            let parents = Set(observations.map(\.id))
            for observation in observations {
                let shot = byParent[observation.id]?.max { $0.timestamp < $1.timestamp }
                moments.append(EpisodeMoment(id: observation.id, date: observation.timestamp, eventID: event.id, observation: observation, screenshot: shot))
            }
            for shot in event.screenshots where shot.imageHash != nil && !parents.contains(shot.parentID ?? "") {
                moments.append(EpisodeMoment(id: shot.id, date: shot.timestamp, eventID: event.id, observation: nil, screenshot: shot))
            }
        }
        moments.sort { $0.date == $1.date ? $0.id < $1.id : $0.date < $1.date }
        var result: [EpisodeMoment] = []
        for moment in moments {
            if let last = result.last, last.screenshot == nil, moment.screenshot == nil, last.text == moment.text, last.eventID == moment.eventID { continue }
            result.append(moment)
        }
        return result
    }

    /// The moment the playhead is on: the latest one not after it.
    static func index(of date: Date, in moments: [EpisodeMoment]) -> Int? {
        guard !moments.isEmpty else { return nil }
        return moments.lastIndex { $0.date <= date } ?? 0
    }
}

/// Opened episode. Left: what it looked like, and the moments it went
/// through. Right: what was on screen as text, then recognition and details
/// on demand. Selecting a moment moves the shared playhead.
struct EpisodeDetailView: View {
    let episode: TimelineEpisode
    let session: DesktopSession
    let cache: TimelineMediaCache
    let cursor: TimelineCursor
    private var playhead: Date { cursor.playhead }
    @Binding var stepRequest: Int
    /// The screenshot shown large over the whole window; Esc or space closes it (handled by the home view).
    @Binding var zoomed: DesktopRecord?
    var prompts: [TimelinePromptMark] = []
    /// Recordings overlapping this episode.
    var recordings: [TimelineEpisode] = []
    var onOpen: (TimelineTarget) -> Void = { _ in }
    let onJump: (Date) -> Void
    let onClose: () -> Void
    let onDelete: (String) -> Void

    @ViewState private var moments: [EpisodeMoment] = []
    @ViewState private var selectedID: String?
    @ViewState private var loading = true
    @ViewState private var textExpanded = false
    @ViewState private var copied = false
    /// The playhead time this view set itself when a moment was picked; the
    /// echo of that change must not re-pick a neighbouring moment.
    @ViewState private var ownJump: Date?

    /// Falls back to the moment under the playhead, so an opening detail never shows another moment first.
    private var selected: EpisodeMoment? {
        moments.first { $0.id == selectedID } ?? EpisodeMoments.index(of: playhead, in: moments).map { moments[$0] } ?? moments.last
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            HStack(alignment: .top, spacing: 18) {
                momentList.frame(width: 200)
                hero.frame(maxWidth: .infinity, maxHeight: .infinity)
                    .contextMenu {
                        if let selected, let screenshot = selected.screenshot {
                            ScreenshotContextActions(record: screenshot, activityID: selected.eventID, text: selected.text ?? "", session: session) { onDelete(selected.eventID) }
                        }
                    }
                sidebar.frame(width: 270)
            }
            .padding(.horizontal, 28).padding(.bottom, 22)
        }
        .task(id: "\(episode.id):\(episode.spans.count):\(episode.spans.last?.id ?? "")") { await load() }
        .onChange(of: playhead) { _, date in follow(date) }
        .onChange(of: stepRequest) { _, value in
            guard value != 0 else { return }
            step(value); stepRequest = 0
        }
        // Stepping while zoomed keeps the zoom and shows the newly selected screenshot.
        .onChange(of: selected?.id) { _, _ in if zoomed != nil { zoomed = selected?.screenshot } }
    }

    // MARK: Header

    private var header: some View {
        HStack(spacing: 12) {
            DesktopApplicationIcon(bundleID: episode.bundleID, fallback: "app", size: 32)
            VStack(alignment: .leading, spacing: 3) {
                Text(episode.title).typography(.section).lineLimit(1)
                Text("\(episode.appName) · \(episode.start.clockText) – \(episode.end.clockText) · \(episode.duration.spokenDuration)")
                    .typography(.secondary).foregroundStyle(.secondary).monospacedDigit()
            }
            Spacer()
            ChromeMenu(systemImage: "ellipsis", help: L("更多操作")) {
                Button(L("导出这段活动"), systemImage: "square.and.arrow.up") {
                    if let id = selected?.eventID { session.selectedID = id; session.exportSelection() }
                }
                Button(L("删除这段活动…"), systemImage: "trash", role: .destructive) {
                    if let id = selected?.eventID { onDelete(id) }
                }
            }
            ChromeButton(systemImage: "xmark", help: L("收起（Esc）"), action: onClose)
        }
        .padding(.horizontal, 28).padding(.top, 20).padding(.bottom, 16)
    }

    // MARK: Hero

    @ViewBuilder private var hero: some View {
        ZStack {
            if let moment = selected {
                if let shot = moment.screenshot {
                    screenshot(shot).id(shot.id).transition(.opacity)
                        .contentShape(Rectangle())
                        .onTapGesture { zoomed = shot }
                        .help(L("单击放大"))
                        .overlay(alignment: .bottom) {
                            Text(L("单击放大 · ↑↓ 切换")).typography(.caption).foregroundStyle(.secondary)
                                .padding(.horizontal, 8).padding(.vertical, 2)
                                .background(Capsule().fill(.regularMaterial)).padding(10)
                                .allowsHitTesting(false)
                        }
                } else {
                    textPage(moment).id(moment.id).transition(.opacity)
                }
            } else if loading {
                ProgressView().controlSize(.small)
            } else {
                Text(L("这段没有可显示的记录")).foregroundStyle(.secondary)
            }
        }
        .animation(.easeInOut(duration: 0.22), value: selected?.id)
    }

    private func screenshot(_ shot: DesktopRecord) -> some View {
        // A smaller decode of the same screenshot stands in while the sharp
        // one loads; the request lives on the container so it fires once.
        Group {
            if let image = cache.image(for: shot, maxPixel: 1600) {
                Image(nsImage: image).resizable().interpolation(.high).aspectRatio(contentMode: .fit)
                    .clipShape(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous).strokeBorder(Surface.hover.color, lineWidth: 0.5))
                    .shadow(color: .black.opacity(0.18), radius: 24, y: 12)
            } else {
                ProgressView().controlSize(.small)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .task(id: shot.id) { cache.requestImage(for: shot, maxPixel: 1600) }
    }

    /// No screenshot: the window's text set as a page, never an empty box.
    private func textPage(_ moment: EpisodeMoment) -> some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 8) {
                DesktopApplicationIcon(bundleID: episode.bundleID, fallback: "app", size: 18)
                Text(moment.screenshot?.manualTitle ?? moment.record?.displayTitle ?? episode.title).typography(.secondary, .medium).foregroundStyle(.secondary).lineLimit(1)
            }
            if let text = moment.text {
                ScrollView {
                    Text(text).typography(.section).lineSpacing(7).textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .scrollIndicators(.never)
            } else {
                Spacer()
                VStack(spacing: 12) {
                    DesktopApplicationIcon(bundleID: episode.bundleID, fallback: "app", size: 56)
                    Text(moment.isProtected ? L("受保护的内容，没有读取文字") : L("这一刻只记录了窗口变化"))
                        .typography(.headline).foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity)
                Spacer()
            }
        }
        .padding(32)
        .frame(maxWidth: 720, maxHeight: .infinity, alignment: .topLeading)
        .background(Surface.wash.color, in: RoundedRectangle(cornerRadius: Radius.large, style: .continuous))
        .frame(maxWidth: .infinity)
    }

    // MARK: Moment list

    /// A row of the left list: a moment of this episode, or a stretch spent in another app.
    private enum ListRow: Identifiable {
        case moment(EpisodeMoment)
        case away(TimelineSpan)
        var id: String { switch self { case .moment(let m): "m:" + m.id; case .away(let s): "a:" + s.id } }
        var date: Date { switch self { case .moment(let m): m.date; case .away(let s): s.start } }
    }

    private var rows: [ListRow] {
        (moments.map(ListRow.moment) + episode.interruptions.map(ListRow.away)).sorted { $0.date < $1.date }
    }

    /// Every moment as a row (time, thumbnail, window), scrolling vertically; ↑↓ step through them.
    private var momentList: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(spacing: 4) {
                    ForEach(rows) { row in
                        switch row {
                        case .moment(let moment): momentRow(moment).id(row.id)
                        case .away(let span): awayRow(span).id(row.id)
                        }
                    }
                }
                .padding(.vertical, 2)
            }
            .scrollIndicators(.never)
            .onChange(of: selected?.id) { _, id in
                guard let id else { return }
                withAnimation(Motion.snappy) { proxy.scrollTo("m:" + id, anchor: .center) }
            }
            .onAppear { if let id = selected?.id { proxy.scrollTo("m:" + id, anchor: .center) } }
        }
    }

    private func momentRow(_ moment: EpisodeMoment) -> some View {
        let on = moment.id == selected?.id
        return Button { select(moment, jump: true) } label: {
            HStack(spacing: 10) {
                thumb(moment)
                    .frame(width: 64, height: 40)
                    .clipShape(RoundedRectangle(cornerRadius: Radius.small - 2, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: Radius.small - 2, style: .continuous).strokeBorder(Surface.hairline, lineWidth: 0.5))
                VStack(alignment: .leading, spacing: 1) {
                    Text(moment.date.formatted(.dateTime.hour(.twoDigits(amPM: .omitted)).minute(.twoDigits)))
                        .typography(.caption, digits: true).foregroundStyle(.secondary)
                    Text(rowTitle(moment)).typography(.secondary, on ? .semibold : .regular).lineLimit(1)
                }
                Spacer(minLength: 0)
            }
            .padding(6)
            .background(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous).fill(on ? Surface.selected.color : .clear))
            .contentShape(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous))
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
    }

    /// A stretch in another app inside this episode; clicking moves the playhead there.
    private func awayRow(_ span: TimelineSpan) -> some View {
        Button { onJump(span.start.addingTimeInterval(1)) } label: {
            HStack(spacing: 10) {
                DesktopApplicationIcon(bundleID: span.bundleID, fallback: "app", size: 18).frame(width: 64)
                VStack(alignment: .leading, spacing: 1) {
                    Text(span.start.formatted(.dateTime.hour(.twoDigits(amPM: .omitted)).minute(.twoDigits)))
                        .typography(.caption, digits: true).foregroundStyle(.tertiary)
                    Text(L("切到 %@ · %@", span.appName, span.duration.spokenDuration)).typography(.secondary).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer(minLength: 0)
            }
            .padding(6).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func rowTitle(_ moment: EpisodeMoment) -> String {
        if let title = moment.record?.windowTitle, !title.isEmpty { return title }
        if moment.screenshot == nil { return moment.isProtected ? L("受保护的内容") : L("只有文字") }
        return episode.title
    }

    @ViewBuilder private func thumb(_ moment: EpisodeMoment) -> some View {
        if let shot = moment.screenshot {
            ZStack {
                Surface.fill.color
                if let image = cache.image(for: shot, maxPixel: 600) {
                    Image(nsImage: image).resizable().aspectRatio(contentMode: .fill)
                } else {
                    Image(systemName: "photo").foregroundStyle(.tertiary)
                }
            }
            .task(id: shot.id) { cache.requestImage(for: shot, maxPixel: 600) }
        } else {
            Text(moment.text ?? (moment.isProtected ? L("受保护") : L("窗口变化")))
                .typography(.caption).foregroundStyle(.secondary).lineLimit(4)
                .padding(7).frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                .background(Surface.fill.color)
        }
    }

    // MARK: Sidebar

    /// What the episode was, what else happened then, and the screen text on demand. Recording details are folded.
    private var sidebar: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                if let notes = episode.notes, !notes.isEmpty { Text(notes).typography(.body).textSelection(.enabled) }
                section(L("这段在做什么"), trailing: EmptyView()) {
                    VStack(spacing: 0) {
                        fact(L("应用"), episode.appName)
                        fact(L("窗口"), selected.map(rowTitle) ?? episode.title)
                        fact(L("时长"), episode.duration.spokenDuration + (episode.interruptions.isEmpty ? "" : L("，中途切走 %@ 次", episode.interruptions.count)))
                    }
                }
                let alongside = prompts.filter { $0.kind != .work && $0.start >= episode.start && $0.start < episode.end }
                if !alongside.isEmpty || !recordings.isEmpty {
                    section(L("同一时间"), trailing: EmptyView()) {
                        VStack(alignment: .leading, spacing: 2) {
                            ForEach(alongside.prefix(4)) { mark in
                                FactRow(systemImage: "sparkle", tint: AIGradient.colors[1], title: mark.preview ?? L("AI 指令"),
                                        detail: Self.sourceText(mark)) { onOpen(.prompt(mark.id)) }
                            }
                            ForEach(recordings) { audio in
                                FactRow(systemImage: "waveform", tint: MediaGlyph.tint(.audio), title: audio.title,
                                        detail: "\(audio.start.clockText)–\(audio.end.clockText)") { onOpen(.episode(audio.id)) }
                            }
                        }
                    }
                }
                if let moment = selected {
                    if let text = moment.text {
                        section(L("这一刻的屏幕文字"), trailing: copyButton(text)) {
                            Button { withAnimation(Motion.snappy) { textExpanded.toggle() } } label: {
                                VStack(alignment: .leading, spacing: 6) {
                                    Text(text).typography(.body).lineSpacing(3).lineLimit(textExpanded ? nil : 2)
                                        .textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                                    Text(textExpanded ? L("收起") : L("展开 · %@ 字", text.count)).typography(.caption).foregroundStyle(.secondary)
                                }
                                .padding(Space.s)
                                .background(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous).fill(Surface.wash.color))
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                        }
                    } else if moment.isProtected {
                        Label(L("受保护的内容，没有读取文字"), systemImage: "lock").typography(.secondary).foregroundStyle(.secondary)
                    }
                    if let selection = moment.selection {
                        section(L("选中的文字"), trailing: copyButton(selection)) {
                            Text(selection).typography(.body).textSelection(.enabled)
                                .padding(.leading, 10)
                                .overlay(alignment: .leading) { Capsule().fill(Color.accentColor).frame(width: 2.5) }
                        }
                    }
                    FoldRow(title: L("技术细节")) {
                        VStack(alignment: .leading, spacing: 10) {
                            Text(momentCaption(moment)).typography(.secondary).foregroundStyle(.secondary)
                            details(moment)
                            if let shot = moment.screenshot {
                                DesktopRecognitionView(session: session.recognition, screenshot: shot).id(shot.id).controlSize(.small)
                            }
                        }
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.bottom, 12)
        }
        .scrollIndicators(.never)
        .animation(Motion.snappy, value: selected?.id)
    }

    private static func sourceText(_ mark: TimelinePromptMark) -> String? {
        let text = [mark.source?.agent, mark.source?.project].compactMap { $0 }.joined(separator: " · ")
        return text.isEmpty ? nil : text
    }

    private func fact(_ title: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(title).typography(.secondary).foregroundStyle(.secondary)
            Spacer(minLength: Space.s)
            Text(value).typography(.secondary).lineLimit(2).multilineTextAlignment(.trailing)
        }
        .padding(.vertical, 6)
        .overlay(alignment: .bottom) { HairlineDivider() }
    }

    private func section<Trailing: View, Content: View>(_ title: String, trailing: Trailing, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 9) {
            HStack {
                Text(title).typography(.secondary, .semibold).foregroundStyle(.secondary)
                Spacer()
                trailing
            }
            content()
        }
    }

    private func copyButton(_ text: String) -> some View {
        Button {
            NSPasteboard.general.clearContents(); NSPasteboard.general.setString(text, forType: .string)
            withAnimation(Motion.snappy) { copied = true }
            Task { try? await Task.sleep(for: .seconds(1.4)); withAnimation { copied = false } }
        } label: {
            Label(copied ? L("已复制") : L("复制"), systemImage: copied ? "checkmark" : "doc.on.doc").typography(.secondary)
                .contentTransition(.symbolEffect(.replace))
        }
        .buttonStyle(.borderless)
    }

    private func details(_ moment: EpisodeMoment) -> some View {
        let record = moment.record
        let rows: [(String, String?)] = [
            (L("触发"), DesktopRecordPresentation.triggerTitle(record?.trigger)),
            (L("窗口"), record?.windowTitle.isEmpty == false ? record?.windowTitle : nil),
            (L("焦点"), DesktopRecordPresentation.focusedRole(record?.context?.focusedRole)),
            (L("控件名称"), record?.context?.focusedLabel),
            (L("文字状态"), DesktopRecordPresentation.textStatus(record?.context?.textStatus)),
            (L("截图尺寸"), moment.screenshot.flatMap { s in s.imageWidth.map { "\($0) × \(s.imageHeight ?? 0)" } }),
        ]
        return Grid(alignment: .leading, horizontalSpacing: 14, verticalSpacing: 7) {
            ForEach(rows.filter { $0.1 != nil }, id: \.0) { row in
                GridRow {
                    Text(row.0).foregroundStyle(.secondary)
                    Text(row.1 ?? "").textSelection(.enabled).lineLimit(2)
                }
            }
        }
        .typography(.secondary)
    }

    private func momentCaption(_ moment: EpisodeMoment) -> String {
        var parts: [String] = []
        if let trigger = DesktopRecordPresentation.triggerTitle(moment.record?.trigger) { parts.append(trigger) }
        parts.append(moment.screenshot == nil ? L("文字") : (moment.text == nil ? L("截图") : L("截图与文字")))
        if let index = moments.firstIndex(of: moment) { parts.append("第 \(index + 1) / \(moments.count) 个时刻") }
        return parts.joined(separator: " · ")
    }

    // MARK: Data & selection

    /// The activity under the playhead first (shown at once), then its neighbours in parallel.
    private func load() async {
        loading = moments.isEmpty
        let spans = Array(episode.spans.suffix(16))
        let here = spans.last { $0.start <= playhead } ?? spans.first
        if moments.isEmpty, let here, let event = await session.eventDetail(id: here.id) {
            withTransaction(Transaction(animation: nil)) { moments = EpisodeMoments.build([event]); follow(playhead) }
        }
        let session = self.session
        let events = await withTaskGroup(of: (Int, DesktopTimelineEvent?).self) { group in
            for (index, span) in spans.enumerated() { group.addTask { (index, await session.eventDetail(id: span.id)) } }
            var found: [(Int, DesktopTimelineEvent)] = []
            for await (index, event) in group { if let event { found.append((index, event)) } }
            return found.sorted { $0.0 < $1.0 }.map(\.1)
        }
        guard !Task.isCancelled else { return }
        withTransaction(Transaction(animation: nil)) { moments = EpisodeMoments.build(events); follow(playhead) }
        loading = false
    }

    private func follow(_ date: Date) {
        if let ownJump, abs(ownJump.timeIntervalSince(date)) < 0.001 { return }
        ownJump = nil
        guard let index = EpisodeMoments.index(of: date, in: moments) else { return }
        if selectedID != moments[index].id { selectedID = moments[index].id; textExpanded = false }
    }

    private func select(_ moment: EpisodeMoment, jump: Bool) {
        selectedID = moment.id; textExpanded = false
        if jump {
            let target = moment.date.addingTimeInterval(0.25)
            ownJump = target
            onJump(target)
        }
    }

    private func step(_ direction: Int) {
        guard let current = moments.firstIndex(where: { $0.id == selected?.id }) else { return }
        let next = min(max(0, current + direction), moments.count - 1)
        select(moments[next], jump: true)
    }
}

/// The selected screenshot large inside the detail: dimmed backdrop, pinch or ⌘-scroll to zoom further, drag to
/// pan, click the backdrop (or Esc / space) to close.
/// A screenshot over the whole window: pinch or double-click to magnify, drag to pan, ←→ to step.
struct ZoomPreview: View {
    let shot: DesktopRecord
    let cache: TimelineMediaCache
    let close: () -> Void
    @ViewState private var scale: CGFloat = 1
    @ViewState private var base: CGFloat = 1
    @ViewState private var offset: CGSize = .zero
    @ViewState private var dragBase: CGSize = .zero

    var body: some View {
        ZStack(alignment: .topTrailing) {
            Rectangle().fill(.black.opacity(0.72)).contentShape(Rectangle()).onTapGesture(perform: close)
            Group {
                if let image = cache.image(for: shot, maxPixel: 2600) {
                    Image(nsImage: image).resizable().interpolation(.high).aspectRatio(contentMode: .fit)
                        .clipShape(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous))
                        .shadow(color: .black.opacity(0.35), radius: 30, y: 14)
                } else {
                    ProgressView().controlSize(.small)
                }
            }
            .padding(.horizontal, 36).padding(.top, 52).padding(.bottom, 44)
            .scaleEffect(scale)
            .offset(offset)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .gesture(MagnifyGesture()
                .onChanged { scale = min(6, max(1, base * $0.magnification)) }
                .onEnded { _ in base = scale; if scale == 1 { withAnimation(Motion.snappy) { offset = .zero; dragBase = .zero } } })
            .simultaneousGesture(DragGesture()
                .onChanged { offset = CGSize(width: dragBase.width + $0.translation.width, height: dragBase.height + $0.translation.height) }
                .onEnded { _ in dragBase = offset })
            .onTapGesture(count: 2) { withAnimation(Motion.snappy) { scale = scale > 1 ? 1 : 2; base = scale; if scale == 1 { offset = .zero; dragBase = .zero } } }
            ChromeButton(systemImage: "xmark", help: L("关闭（Esc）"), action: close).padding(.top, 14).padding(.trailing, 16)
            Text(L("双击或捏合放大 · 拖动平移 · ←→ 上一张/下一张 · Esc 关闭")).typography(.caption).foregroundStyle(.white.opacity(0.8))
                .padding(12).frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottom).allowsHitTesting(false)
        }
        .task(id: shot.id) { cache.requestImage(for: shot, maxPixel: 2600) }
        .transition(.opacity)
    }
}
