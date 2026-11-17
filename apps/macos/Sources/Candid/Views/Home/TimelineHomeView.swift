import AppKit
import DesktopCore
import SwiftUI

enum TimelineMode: Hashable { case episodes, rhythm }

/// Home page. Order of information: the day (title + one fact line), the
/// moment under the playhead (cards / ring), then evidence on demand
/// (detail). The playhead is the single source of "where am I".
struct TimelineHomeView: View {
    @Bindable var session: DesktopSession
    let media: MediaWorkspaceStore?
    @Binding var ambientTint: Color
    /// Set by other pages (library, assistant) to show a moment here.
    @Binding var pendingJump: Date?
    var autofocus = true
    @ViewState private var mediaSearch = TimelineMediaSearch()

    @ViewState private var interactions = AIInteractionTimeline()
    @ViewState private var timelineVisible = true
    @ViewState private var cache: TimelineMediaCache?
    @ViewState private var scene = TimelineScene(events: [], day: Date(), now: Date())
    /// Playhead and visible stretch; read only by the views that draw them, so moving them never re-evaluates this page.
    @ViewState private var cursor = TimelineCursor()
    @ViewState private var followingNow = true
    /// AI lane marks, rebuilt when the day's AI entries or their sources change (not on every frame).
    @ViewState private var promptMarks: [TimelinePromptMark] = []
    /// Bumped whenever the playhead is moved on purpose; the scrubber then brings it into view.
    @ViewState private var revealRequest = 0
    /// The user is dragging, wheeling or pinching the timeline; new data waits until that ends.
    @ViewState private var interacting = false
    @ViewState private var rebuildPending = false
    /// One timeline build at a time, at most about twice a second; changes meanwhile are folded into the next one.
    @ViewState private var sceneScheduler = TimelineSceneScheduler()
    @ViewState private var widgetPublishedAt = Date.distantPast
    @ViewState private var mode: TimelineMode = .episodes
    @ViewState private var openedID: String?
    /// An AI instruction opened from the AI lane; shown in place of the stage.
    @ViewState private var openedPrompt: String?
    @ViewState private var playing = false
    @ViewState private var speed: Double = 60
    @ViewState private var confirmingDelete = false
    @ViewState private var deletingActivityID: String?
    @ViewState private var deletingMedia: MediaLibraryRecord?
    @ViewState private var detailStep = 0
    /// A screenshot is shown enlarged inside the open detail.
    @ViewState private var detailZoomed: DesktopRecord?
    @Namespace private var cards
    @FocusState private var focused: Bool

    private var isToday: Bool { Calendar.current.isDateInToday(session.selectedDate) }
    /// Reads and writes go through the cursor. Inside `body` only child views read it (see `PlayheadEffects`).
    private var playhead: Date {
        get { cursor.playhead }
        nonmutating set { cursor.playhead = newValue }
    }
    private var focusedIndex: Int? { scene.episodeIndex(at: playhead) }
    @ViewState private var fragmentEditor: FragmentEditModel?
    /// Per-day amounts for the date jumper, kept while the page lives so reopening it is instant.
    @ViewState private var jumper: DateJumperModel?
    private var focusedEpisode: TimelineEpisode? { focusedIndex.map { scene.episodes[$0] } }

    var body: some View {
        let _ = TimelinePerf.tick(.homeBody)
        timelineBody
        .onReceive(NotificationCenter.default.publisher(for: .candidEditFragment)) { note in
            guard let target = note.object as? FragmentEditTarget else { return }
            if let id = target.mediaID, let episode = scene.episodes.first(where: { $0.mediaID == id.uuidString }) { open(episode); return }
            fragmentEditor = FragmentEditModel(target: target)
        }
        .sheet(item: $fragmentEditor) { model in FragmentEditView(model: model) { fragmentEditor = nil } } // style-exempt: source-attached edit sheet
    }

    private var stageContent: some View {
        VStack(spacing: 0) {
            header.padding(.horizontal, 40).padding(.top, 62)
            ZStack {
                if let cache { stage(cache) }
                if let openedID, let episode = scene.episodes.first(where: { $0.id == openedID }) {
                    detail(episode).zIndex(5)
                }
                if let openedPrompt {
                    AIPromptDetailView(model: interactions, mark: promptMarks.first { $0.id == openedPrompt }, scene: scene,
                                       position: promptPosition, onStep: stepPrompt, onOpen: openTarget, onClose: close)
                        .padding(.horizontal, 28).padding(.top, 14).padding(.bottom, 6)
                        .transition(.opacity.combined(with: .scale(scale: 0.985)))
                        .zIndex(6)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            if let cache, !scene.isEmpty || !interactions.entries.isEmpty {
                TimelineScrubber(scene: scene, cursor: cursor, playing: $playing, speed: $speed,
                                 followingNow: followingNow, isToday: isToday, cache: cache,
                                 prompts: promptMarks, opened: openedTarget, revealRequest: revealRequest, interacting: $interacting,
                                 onScrub: { followingNow = false },
                                 onReturnToNow: returnToNow, onLocate: locateTarget,
                                 onPeekPrompt: { id in Task { await interactions.loadSource(id) } }, onOpen: openTarget,
                                 contextActions: contextActions, promptActions: promptActions)
                    .zIndex(10)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
            }
        }
        .background {
            DesktopTimelineVisibility(onChange: { visible in
                timelineVisible = visible || QASnapshot.isActive
                session.setTimelineVisible(timelineVisible)
            })
                .frame(width: 0, height: 0).allowsHitTesting(false).accessibilityHidden(true)
        }
    }

    private var timelineBody: some View {
        AnyView(interactiveContent)
        // The zoomed screenshot is drawn by the root view, above the navigation, so it covers the whole window.
        .preference(key: WindowOverlayKey.self, value: zoomOverlay)
        .onChange(of: interacting) { _, active in if !active && rebuildPending { rebuildPending = false; rebuild() } }
        .onChange(of: timelineVisible) { _, visible in
            sceneScheduler.setVisible(visible)
            if visible { rebuild(); rebuildPromptMarks() } else { playing = false }
        }
        .onDisappear { sceneScheduler.setVisible(false) }
        .onChange(of: session.searchText) { _, _ in rebuild() }
        .onChange(of: pendingJump) { _, _ in consumePendingJump() }
        .onChange(of: session.isRecording) { _, _ in requestRebuild() }
        .task(id: mediaSearchToken) { await refreshMediaSearch() }
        .onChange(of: mediaSearch.version) { _, _ in requestRebuild() }
        .onChange(of: session.timelineEvents) { _, _ in requestRebuild() }
        .onChange(of: mediaRevision) { _, _ in requestRebuild(); Task { await refreshMediaSearch() } }
        .onChange(of: session.selectedDate) { _, _ in
            cache?.reset(); openedID = nil; cursor.viewportReady = false; followingNow = isToday; rebuild()
        }
        .background {
            PlayheadEffects(scene: scene, cursor: cursor, openedID: openedID, selectedID: session.selectedID,
                            onSelectSpan: { session.selectedID = $0 },
                            onFocus: { episode in ambientTint = episode.kind == .app ? AppTint.color(for: episode.bundleID) : MediaGlyph.tint(episode.kind) })
        }
        .onChange(of: interactions.entries, initial: true) { _, _ in rebuildPromptMarks() }
        .onChange(of: interactions.sources) { _, _ in rebuildPromptMarks() }
        .task(id: playing) { await playback() }
        .task(id: timelineVisible) { if timelineVisible { await tickClock() } }
        .task(id: media?.records.count ?? -1) { if let media, media.records.isEmpty { await media.refresh() } }
        .confirmCapsule("删除这段活动？", isPresented: $confirmingDelete, message: "删除这段活动中的全部记录和截图。已导出的副本不受影响。", action: deleteActivity)
        .confirmCapsule("删除这条资料？", isPresented: Binding(get: { deletingMedia != nil }, set: { if !$0 { deletingMedia = nil } }), message: "这会删除资料库里的记录和对应的识别结果。", action: deleteMedia)
    }

    private var zoomOverlay: WindowOverlay? {
        guard let shot = detailZoomed, let cache else { return nil }
        return WindowOverlay(id: shot.id, view: AnyView(ZoomPreview(shot: shot, cache: cache) { detailZoomed = nil }.id(shot.id)))
    }

    /// Library changes already arrive through `media`; the core's change counter only matters while searching.
    private var mediaSearchToken: String { !timelineVisible ? "hidden" : session.searchText.isEmpty ? "" : session.searchText + "#" + String(TimelineService.shared.dataVersion) }
    /// A digest of what the timeline shows from the library (ids, titles, notes, transcript size); hashing avoids
    /// building and comparing one string per record on every evaluation of this page.
    private var mediaRevision: Int {
        var hasher = Hasher()
        for record in media?.records ?? [] {
            hasher.combine(record.id); hasher.combine(record.displayTitle); hasher.combine(record.notes); hasher.combine(record.segments.count)
        }
        return hasher.finalize()
    }
    private func refreshMediaSearch() async {
        guard timelineVisible else { return }
        await mediaSearch.refresh(query: session.searchText, media: media)
        requestRebuild()
    }

    private var interactiveContent: some View {
        stageContent
        .focusable().focusEffectDisabled().focused($focused)
        .onKeyPress(.space) { if detailZoomed != nil { detailZoomed = nil } else { playing.toggle() }; return .handled }
        .onKeyPress(.upArrow) { guard openedID != nil else { return .ignored }; detailStep = -1; return .handled }
        .onKeyPress(.downArrow) { guard openedID != nil else { return .ignored }; detailStep = 1; return .handled }
        .onKeyPress(.leftArrow) { openedPrompt != nil ? stepPrompt(-1) : openedID == nil ? step(-1) : (detailStep = -1); return .handled }
        .onKeyPress(.rightArrow) { openedPrompt != nil ? stepPrompt(1) : openedID == nil ? step(1) : (detailStep = 1); return .handled }
        .onKeyPress(.return) { if let episode = focusedEpisode { open(episode) }; return .handled }
        .onKeyPress(.escape) {
            if detailZoomed != nil { detailZoomed = nil; return .handled }
            if openedID != nil || openedPrompt != nil { close(); return .handled }
            return .ignored
        }
        .onKeyPress(characters: CharacterSet(charactersIn: "12")) { press in
            mode = [.episodes, .rhythm][Int(press.characters)! - 1]; return .handled
        }
        .onAppear {
            sceneScheduler.setVisible(timelineVisible)
            if cache == nil { cache = TimelineMediaCache(session: session, media: media) }
            if jumper == nil { jumper = DateJumperModel { [session] days in try await session.dayAmounts(days) } }
            if session.isDemo, !interactions.isDemo { interactions = AIInteractionTimeline.demo(now: Date()) }
            rebuild(); consumePendingJump(); if autofocus { focused = true }
        }
        .onReceive(NotificationCenter.default.publisher(for: QASnapshot.command)) { note in
            guard let line = note.object as? String else { return }
            let parts = line.split(separator: " ").map(String.init)
            switch parts.first {
            case "mode": mode = ["episodes": .episodes, "rhythm": .rhythm][parts.last ?? ""] ?? mode
            case "open": if let episode = focusedEpisode { open(episode) }
            case "close": close()
            case "step": step(Int(parts.last ?? "") ?? 1)
            case "back": jump(to: playhead.addingTimeInterval(-60 * (Double(parts.last ?? "") ?? 30)))
            // Self-test only: puts the playhead at a Unix time the way a drag does (no animation).
            case "scrub":
                if let seconds = Double(parts.last ?? "") {
                    if !interacting { interacting = true }
                    followingNow = false; playing = false; playhead = Date(timeIntervalSince1970: seconds)
                }
            case "scrub-end": interacting = false
            case "play": playing = true
            case "pause": playing = false
            default: break
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: .candidAgentConsentChanged)) { _ in
            interactions.clear()
            if timelineVisible { Task { await interactions.refresh(day: session.selectedDate, search: session.searchText) } }
        }
        .task(id: interactionRefreshKey) {
            guard timelineVisible else { return }
            do { try await Task.sleep(for: .milliseconds(180)) } catch { return }
            var loadedVersion: Int?
            while !Task.isCancelled {
                let version = TimelineService.shared.dataVersion
                if loadedVersion != version {
                    await interactions.refresh(day: session.selectedDate, search: session.searchText)
                    if interactions.error == nil { loadedVersion = version }
                }
                // Capture and OCR can publish several changes per second. Coalesce them without
                // reloading unchanged pages or continually cancelling an in-flight paged query.
                do { try await Task.sleep(for: .seconds(5)) } catch { return }
            }
        }
    }
    private struct InteractionRefreshKey: Hashable {
        var day: Date
        var query: String
        var visible: Bool
    }
    private var interactionRefreshKey: InteractionRefreshKey {
        .init(day: session.selectedDate, query: session.searchText, visible: timelineVisible)
    }
    private func tickClock() async {
        while !Task.isCancelled {
            do { try await Task.sleep(for: .seconds(20)) } catch { return }
            if followingNow && isToday && !playing { requestRebuild() }
        }
    }
    private func deleteActivity() {
        if let id = deletingActivityID { session.selectedID = id }
        session.deleteSelected(); deletingActivityID = nil; close()
    }
    private func deleteMedia() {
        if let record = deletingMedia, let media { Task { await media.delete(record) } }
        deletingMedia = nil; close()
    }

    private func mediaRecord(_ episode: TimelineEpisode) -> MediaLibraryRecord? {
        guard let id = episode.mediaID else { return nil }
        return media?.records.first { $0.id.uuidString == id } ?? mediaSearch.items.first { $0.id.uuidString == id }
    }

    // MARK: Header

    private var header: some View {
        DayHeader(date: session.selectedDate, factLine: factLine, showsModes: !scene.isEmpty, mode: $mode, jumper: jumper,
                  onShift: shiftDay, onPick: pick(day:))
    }

    private var factLine: String {
        if scene.isEmpty { return session.searchText.isEmpty ? "还没有记录" : "没有匹配「\(session.searchText)」的记录" }
        var parts = ["记录 \(scene.recordedSeconds.spokenDuration)", "\(Set(scene.activities.map(\.bundleID)).count) 个应用"]
        if !scene.audio.isEmpty { parts.append("\(scene.audio.count) 段录音") }
        if !scene.images.isEmpty { parts.append("\(scene.images.count) 张截图") }
        if !session.searchText.isEmpty { parts.insert("搜索「\(session.searchText)」", at: 0) }
        return parts.joined(separator: " · ")
    }

    // MARK: Stage

    @ViewBuilder private func stage(_ cache: TimelineMediaCache) -> some View {
        if scene.isEmpty {
            emptyState
        } else {
            ZStack {
                switch mode {
                case .episodes:
                    TimelineMomentView(scene: scene, cursor: cursor, cache: cache, prompts: promptMarks,
                                       onOpen: openTarget, onJump: { jump(to: $0) }, onStep: step, contextActions: contextActions,
                                       animated: !interacting)
                        .padding(.horizontal, 40).padding(.top, 8)
                        .transition(.asymmetric(insertion: .opacity.combined(with: .offset(y: -40)),
                                                removal: .opacity.combined(with: .offset(y: 60)).combined(with: .scale(scale: 0.94))))
                case .rhythm:
                    DayRhythmView(scene: scene, cursor: cursor)
                        .transition(.opacity.combined(with: .scale(scale: 0.985)))
                }
            }
            .animation(Motion.spring, value: mode)
            .opacity(openedID == nil && openedPrompt == nil ? 1 : 0)
            .scaleEffect(openedID == nil && openedPrompt == nil ? 1 : 0.97)
            .animation(Motion.spring, value: openedID)
            .animation(Motion.spring, value: openedPrompt)
        }
    }

    private var emptyState: some View {
        VStack(spacing: 16) {
            Image(systemName: "circle.dotted").typography(.display, .ultraLight).foregroundStyle(.tertiary)
                .symbolEffect(.pulse, options: .repeating, isActive: session.isRecording)
            if !session.searchText.isEmpty {
                QuietButton(title: "清除搜索") { session.searchText = "" }
            } else if isToday && !session.isRecording && !session.isDemo {
                Button { session.startRecording() } label: {
                    Text("开始记录").typography(.headline).padding(.horizontal, 20).frame(height: 34)
                        .background(Capsule().fill(Color.accentColor)).foregroundStyle(.white)
                }
                .buttonStyle(.plain)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    // MARK: Detail

    private func detail(_ episode: TimelineEpisode) -> some View {
        ZStack(alignment: .topTrailing) {
            if episode.kind == .app { appDetail(episode) } else { mediaDetail(episode) }
        }
        .padding(.horizontal, 28).padding(.top, 14).padding(.bottom, 6)
    }

    private func detailSurface(_ episode: TimelineEpisode) -> some View {
        // Glass over the ambient background: the page stays one surface.
        RoundedRectangle(cornerRadius: Radius.card, style: .continuous)
            .fill(.regularMaterial)
            .overlay(RoundedRectangle(cornerRadius: Radius.card, style: .continuous).strokeBorder(Surface.hover.color, lineWidth: 0.5))
            .matchedGeometryEffect(id: episode.id, in: cards)
            .shadow(color: .black.opacity(0.14), radius: 34, y: 14)
    }

    private func mediaDetail(_ episode: TimelineEpisode) -> some View {
        ZStack(alignment: .topLeading) {
            detailSurface(episode)
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 12) {
                    MediaGlyph(kind: episode.kind, size: 34)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(episode.title).typography(.section).lineLimit(1)
                        Text("\(episode.appName) · \(episode.start.clockText)\(episode.kind == .audio ? " – " + episode.end.clockText + " · " + episode.duration.spokenDuration : "")")
                            .typography(.secondary).foregroundStyle(.secondary).monospacedDigit()
                    }
                    Spacer()
                    ChromeButton(systemImage: "xmark", help: "收起（Esc）", action: close)
                }
                .padding(.horizontal, 26).padding(.top, 20).padding(.bottom, 4)
                if let media, let record = mediaRecord(episode) {
                    MediaRecordView(item: media.selected?.id == record.id ? (media.selected ?? record) : record, store: media,
                                    requestDelete: { deletingMedia = record }, showsHeader: false)
                        .id(record.id)
                        .task(id: record.id) { media.selectedID = record.id; await media.loadDetails() }
                } else {
                    Text("这条资料暂时读不到，资料库可能未连接。").foregroundStyle(.secondary).padding(26)
                }
            }
            .transition(.opacity.animation(.easeOut(duration: 0.25).delay(0.12)))
        }
    }

    private func appDetail(_ episode: TimelineEpisode) -> some View {
        ZStack(alignment: .topTrailing) {
            detailSurface(episode)
            if let cache {
                EpisodeDetailView(episode: episode, session: session, cache: cache, cursor: cursor, stepRequest: $detailStep,
                                  zoomed: $detailZoomed, prompts: promptMarks,
                                  recordings: scene.episodes.filter { $0.kind == .audio && $0.id != episode.id && $0.start < episode.end && $0.end > episode.start },
                                  onOpen: openTarget,
                                  onJump: { followingNow = false; playing = false; playhead = $0; revealRequest &+= 1 },
                                  onClose: close,
                                  onDelete: { id in playing = false; deletingActivityID = id; confirmingDelete = true })
                    .transition(.opacity.animation(.easeOut(duration: 0.25).delay(0.12)))
            }
        }
    }

    // MARK: Actions

    /// New data while the user is moving the timeline is merged once they stop, so nothing shifts under the pointer.
    private func requestRebuild() {
        if interacting { rebuildPending = true } else { rebuild() }
    }

    /// Coalesced: while one build runs, further requests only mark it to run once more afterwards.
    private func rebuild() {
        sceneScheduler.request { await rebuildNow() }
    }

    /// Inputs are read here; the scene itself is built off the main thread and applied when ready.
    private func rebuildNow() async {
        guard timelineVisible, !Task.isCancelled else { return }
        let records = mediaSearch.records(query: session.searchText, media: media)
        let audio = records.compactMap { record -> TimelineAudioSpan? in
            guard record.kind == .audio, Calendar.current.isDate(record.displayedDate, inSameDayAs: session.selectedDate) else { return nil }
            let length = max(30, record.segments.map(\.endSeconds).max() ?? 60)
            let start = record.displayedDate
            return TimelineAudioSpan(id: record.id.uuidString, start: start, end: start.addingTimeInterval(length), title: record.displayTitle,
                                     speech: record.segments.map { start.addingTimeInterval($0.startSeconds)...start.addingTimeInterval(max($0.startSeconds, $0.endSeconds)) },
                                     lines: record.segments.map { TimelineTranscriptLine(start: start.addingTimeInterval($0.startSeconds),
                                                                                        end: start.addingTimeInterval(max($0.startSeconds, $0.endSeconds)),
                                                                                        speaker: $0.shownSpeaker, text: $0.text) },
                                     source: record.source.label)
        }
        let images = records.compactMap { record -> TimelineImagePin? in
            guard record.kind == .image, Calendar.current.isDate(record.displayedDate, inSameDayAs: session.selectedDate) else { return nil }
            return TimelineImagePin(id: record.id.uuidString, date: record.displayedDate, title: record.displayTitle, source: record.source.label)
        }
        let now = Date()
        let events = session.timelineEvents, day = session.selectedDate, live = session.isRecording && isToday ? now : nil
        TimelinePerf.tick(.sceneBuild)
        let built = await Task.detached(priority: .userInitiated) {
            TimelineScene(events: events, audio: audio, images: images, day: day, now: now, liveUntil: live)
        }.value
        guard timelineVisible, !Task.isCancelled else { return }
        guard day == session.selectedDate else { rebuild(); return }
        // An identical scene is not reassigned, so a refresh that found nothing new redraws nothing.
        if built != scene { scene = built } else { TimelinePerf.tick(.sceneUnchanged) }
        if followingNow {
            playhead = isToday ? min(now, scene.bounds.upperBound) : (scene.activities.last?.end ?? scene.bounds.upperBound)
        } else {
            playhead = min(max(playhead, scene.bounds.lowerBound), scene.bounds.upperBound)
        }
        if let openedID, !scene.episodes.contains(where: { $0.id == openedID }) { self.openedID = nil }
        if session.searchText.isEmpty, now.timeIntervalSince(widgetPublishedAt) > 15 {
            widgetPublishedAt = now
            WidgetPublisher.publish(scene: scene, day: session.selectedDate, isDemo: session.isDemo,
                                    isRecording: session.isRecording, isMicRecording: media?.isRecording ?? false)
        }
    }

    private func contextActions(_ episode: TimelineEpisode) -> AnyView {
        AnyView(EpisodeContextActions(episode: episode, session: session, media: media, resolvedMediaRecord: mediaRecord(episode)) {
            if let record = mediaRecord(episode) { deletingMedia = record }
            else { playing = false; deletingActivityID = episode.latest?.id; confirmingDelete = true }
        })
    }

    // MARK: Lanes

    private func rebuildPromptMarks() {
        let marks = TimelinePromptMark.build(entries: interactions.entries, sources: interactions.sources)
        if marks != promptMarks { promptMarks = marks }
    }

    private var openedTarget: TimelineTarget? {
        if let openedPrompt { return .prompt(openedPrompt) }
        return openedID.map(TimelineTarget.episode)
    }

    /// First click on a lane item: the playhead goes there and the stage shows it; nothing opens.
    private func locateTarget(_ target: TimelineTarget) {
        switch target {
        case .episode(let id):
            guard let episode = scene.episodes.first(where: { $0.id == id }) else { return }
            if !episode.contains(playhead) { jump(to: episode.start.addingTimeInterval(1)) }
        case .shot(let spanID):
            guard let span = scene.spans.first(where: { $0.id == spanID }) else { return }
            jump(to: span.start.addingTimeInterval(1))
        default: openTarget(target)
        }
    }

    private func openTarget(_ target: TimelineTarget) {
        switch target {
        case .episode(let id):
            guard let episode = scene.episodes.first(where: { $0.id == id }) else { return }
            if !episode.contains(playhead) { jump(to: episode.start.addingTimeInterval(1)) }
            open(episode)
        case .shot(let spanID):
            guard let span = scene.spans.first(where: { $0.id == spanID }),
                  let episode = scene.episodes.first(where: { $0.spans.contains(span) || $0.interruptions.contains(span) }) else { return }
            jump(to: span.start.addingTimeInterval(1))
            open(episode, at: span.id)
        case .prompt(let id):
            guard let mark = promptMarks.first(where: { $0.id == id }) else { return }
            // Work stretches open the instruction they belong to.
            let prompt = mark.kind == .work ? (promptMarks.last { $0.kind == .prompt && $0.start <= mark.start } ?? mark) : mark
            jump(to: prompt.start)
            playing = false
            withAnimation(Motion.spring) { openedID = nil; openedPrompt = prompt.id }
            Task { await interactions.open(prompt.id) }
        case .cluster: break
        }
    }

    /// Instructions stacked on one lane mark are reached with the detail's ‹ › (or ←→), in time order.
    private var promptPosition: (index: Int, count: Int)? {
        let list = promptMarks.filter { $0.kind != .work }
        return list.firstIndex(where: { $0.id == openedPrompt }).map { ($0 + 1, list.count) }
    }

    private func stepPrompt(_ direction: Int) {
        let list = promptMarks.filter { $0.kind != .work }
        guard let current = list.firstIndex(where: { $0.id == openedPrompt }) else { return }
        let next = current + direction
        guard list.indices.contains(next) else { return }
        openTarget(.prompt(list[next].id))
    }

    private func promptActions(_ mark: TimelinePromptMark) -> AnyView {
        AnyView(Group {
            Button("打开详情") { openTarget(.prompt(mark.id)) }
            Button("从这里播放") { jump(to: mark.start); playing = true }
            Divider() // style-exempt: native context menu separator
            Button("复制指令文字") {
                NSPasteboard.general.clearContents(); NSPasteboard.general.setString(mark.preview ?? "", forType: .string)
            }.disabled((mark.preview ?? "").isEmpty)
            Button("复制引用") {
                let reference = "AI 指令 · \(mark.start.ISO8601Format())\nevent_id: \(mark.id)"
                NSPasteboard.general.clearContents(); NSPasteboard.general.setString(reference, forType: .string)
            }
        })
    }

    private func jump(to date: Date) {
        followingNow = false; playing = false
        withAnimation(Motion.snappy) { playhead = date }
        revealRequest &+= 1
    }

    private func returnToNow() {
        playing = false; followingNow = true
        withAnimation(Motion.snappy) { playhead = min(Date(), scene.bounds.upperBound) }
        revealRequest &+= 1
    }

    private func step(_ direction: Int) {
        guard let index = focusedIndex else { return }
        let next = min(max(0, index + direction), scene.episodes.count - 1)
        jump(to: scene.episodes[next].start.addingTimeInterval(1))
    }

    private func consumePendingJump() {
        guard let date = pendingJump else { return }
        pendingJump = nil
        if !Calendar.current.isDate(date, inSameDayAs: session.selectedDate) { session.selectedDate = date }
        mode = .episodes
        jump(to: date)
    }

    /// Opens on the activity under the playhead (or `spanID`), never on another one first.
    private func open(_ episode: TimelineEpisode, at spanID: String? = nil) {
        playing = false
        if openedPrompt != nil { interactions.close(); openedPrompt = nil }
        let here = scene.activity(at: playhead).flatMap { span in episode.spans.contains(span) || episode.interruptions.contains(span) ? span.id : nil }
        if let id = spanID ?? here ?? episode.latest?.id { session.selectedID = id }
        withAnimation(Motion.spring) { openedID = episode.id }
    }

    private func close() {
        detailZoomed = nil
        if openedPrompt != nil { interactions.close() }
        withAnimation(Motion.spring) { openedID = nil; openedPrompt = nil }
        focused = true
    }

    private func shiftDay(_ days: Int) {
        guard let date = Calendar.current.date(byAdding: .day, value: days, to: session.selectedDate) else { return }
        pick(day: date)
    }

    /// Shows another day (never a future one). Today follows now; another day opens at its end.
    private func pick(day: Date) {
        let today = Date()
        let target = min(day, today)
        guard !Calendar.current.isDate(target, inSameDayAs: session.selectedDate) else { return }
        playing = false
        // Noon of a past day keeps the date stable across time zones and daylight saving; today keeps now.
        session.selectedDate = Calendar.current.isDate(target, inSameDayAs: today) ? today
            : Calendar.current.startOfDay(for: target).addingTimeInterval(12 * 3600)
    }

    /// Replays the day at `speed`× and skips stretches with nothing recorded.
    private func playback() async {
        guard playing else { return }
        followingNow = false
        if playhead >= scene.bounds.upperBound.addingTimeInterval(-60) || (isToday && playhead >= Date().addingTimeInterval(-60)) {
            playhead = scene.activities.first?.start ?? scene.bounds.lowerBound
        }
        var last = Date()
        while playing && !Task.isCancelled {
            try? await Task.sleep(for: .milliseconds(33))
            let now = Date(); let elapsed = now.timeIntervalSince(last); last = now
            var next = playhead.addingTimeInterval(elapsed * speed)
            if scene.activity(at: next) == nil, scene.audio(at: next) == nil, let resume = scene.nextActivityStart(after: next) { next = resume }
            let end = isToday ? min(Date(), scene.bounds.upperBound) : scene.bounds.upperBound
            if next >= end { playhead = end; playing = false; break }
            playhead = next
        }
    }
}

/// What follows the playhead without redrawing the page: the ambient tint of the episode under it, and (after a
/// short pause) the selected activity, which loads its detail.
private struct PlayheadEffects: View {
    let scene: TimelineScene
    let cursor: TimelineCursor
    let openedID: String?
    let selectedID: String?
    let onSelectSpan: (String) -> Void
    let onFocus: (TimelineEpisode) -> Void

    var body: some View {
        let playhead = cursor.playhead
        let episode = scene.episodeIndex(at: playhead).map { scene.episodes[$0] }
        let spanID = (scene.activity(at: playhead) ?? episode?.latest)?.id
        Color.clear
            .onChange(of: episode?.id, initial: true) { _, _ in if let episode { onFocus(episode) } }
            .task(id: spanID) {
                guard let spanID, spanID != selectedID else { return }
                try? await Task.sleep(for: .milliseconds(openedID == nil ? 350 : 120))
                guard !Task.isCancelled else { return }
                onSelectSpan(spanID)
            }
            .allowsHitTesting(false).accessibilityHidden(true)
    }
}

/// The day title (date, weekday or 今天, one fact line), ‹ › for neighbouring days, and the date jumper behind the
/// title. Its own view, so it is evaluated when the day or its facts change, not with the playhead or the lanes.
private struct DayHeader: View {
    let date: Date
    let factLine: String
    let showsModes: Bool
    @Binding var mode: TimelineMode
    let jumper: DateJumperModel?
    let onShift: (Int) -> Void
    let onPick: (Date) -> Void
    @ViewState private var jumping = false
    @ViewState private var hovering = false

    private var isToday: Bool { Calendar.current.isDateInToday(date) }

    var body: some View {
        let _ = TimelinePerf.tick(.headerBody)
        HStack(alignment: .bottom, spacing: 18) {
            VStack(alignment: .leading, spacing: 6) {
                HStack(alignment: .firstTextBaseline, spacing: 10) {
                    Button {
                        // Opening re-reads the amounts it shows (today keeps growing); the cache spares only the wait.
                        if !jumping { jumper?.refresh() }
                        jumping.toggle()
                    } label: {
                        HStack(alignment: .firstTextBaseline, spacing: 10) {
                            Text("\(Calendar.current.component(.month, from: date))月\(Calendar.current.component(.day, from: date))日")
                                .typography(.display)
                                .contentTransition(.numericText())
                            Text(isToday ? "今天" : date.formatted(.dateTime.weekday(.wide)))
                                .typography(.section, .medium).foregroundStyle(.secondary)
                            Image(systemName: "chevron.down").typography(.caption, .bold).foregroundStyle(.tertiary)
                                .rotationEffect(.degrees(jumping ? 180 : 0))
                                .opacity(hovering || jumping ? 1 : 0)
                        }
                        .padding(.horizontal, Space.xs).padding(.vertical, Space.xxs)
                        .background(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous)
                            .fill(jumping ? Surface.selected.color : hovering ? Surface.hover.color : .clear))
                        .contentShape(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous))
                    }
                    .buttonStyle(.plain)
                    .padding(.horizontal, -Space.xs).padding(.vertical, -Space.xxs)
                    .onHover { hovering = $0 }
                    .animation(Motion.snappy, value: hovering)
                    .animation(Motion.snappy, value: jumping)
                    .help("选择日期（⌘D）")
                    .accessibilityLabel("选择日期")
                    .accessibilityValue(date.formatted(date: .complete, time: .omitted))
                    .keyboardShortcut("d", modifiers: .command)
                    .popover(isPresented: $jumping, arrowEdge: .bottom) {
                        if let jumper {
                            DateJumperView(selected: date, model: jumper,
                                           onPick: { day in jumping = false; onPick(day) },
                                           onClose: { jumping = false })
                        }
                    }
                    HStack(spacing: 0) {
                        ChromeButton(systemImage: "chevron.left", help: "前一天") { onShift(-1) }
                        ChromeButton(systemImage: "chevron.right", help: "后一天") { onShift(1) }.disabled(isToday)
                    }
                }
                Text(factLine).typography(.body).foregroundStyle(.secondary)
                    .contentTransition(.opacity)
            }
            Spacer()
            PillSwitcher(items: [(TimelineMode.episodes, "片段"), (.rhythm, "节律")], selection: $mode, scale: .secondary)
                .opacity(showsModes ? 1 : 0)
        }
        .animation(Motion.spring, value: date)
    }
}
