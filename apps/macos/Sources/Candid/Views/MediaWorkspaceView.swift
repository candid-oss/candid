import SwiftUI
import CandidLocalization
import UniformTypeIdentifiers
import ImageIO

/// The library: everything captured on purpose (recordings, region
/// screenshots, imports), grouped by day, each one linked back to its moment.
struct MediaWorkspaceView: View {
    @Bindable var store: MediaWorkspaceStore
    /// Jumps to the record's moment on the home timeline.
    var showInTimeline: ((Date) -> Void)? = nil
    @ViewState private var importing = false
    @ViewState private var confirmDiscard = false
    @ViewState private var deleting: MediaLibraryRecord?
    @ViewState private var metadataEditTarget: UUID?
    @ViewState private var metadataEditRequest = UUID()
    @ViewState private var dropTargeted = false
    @FocusState private var searchFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header.padding(.horizontal, 40).padding(.top, 62)
            notices.padding(.horizontal, 40)
            HStack(alignment: .top, spacing: 22) {
                list.frame(width: 350)
                detail
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(RoundedRectangle(cornerRadius: Radius.card, style: .continuous).fill(.regularMaterial))
                    .overlay(RoundedRectangle(cornerRadius: Radius.card, style: .continuous).strokeBorder(Surface.hover.color, lineWidth: 0.5))
                    .clipShape(RoundedRectangle(cornerRadius: Radius.card, style: .continuous))
            }
            .padding(.leading, 28).padding(.trailing, 28).padding(.top, 18).padding(.bottom, 24)
        }
        .overlay {
            if dropTargeted {
                RoundedRectangle(cornerRadius: Radius.card, style: .continuous)
                    .strokeBorder(Color.accentColor, style: StrokeStyle(lineWidth: 2, dash: [8, 6]))
                    .background(Color.accentColor.opacity(0.06), in: RoundedRectangle(cornerRadius: Radius.card, style: .continuous))
                    .overlay { Label(L("放开导入"), systemImage: "square.and.arrow.down").typography(.section).foregroundStyle(.tint) }
                    .padding(18).padding(.top, 44)
                    .transition(.opacity)
                    .allowsHitTesting(false)
            }
        }
        .animation(Motion.snappy, value: dropTargeted)
        .accessibilityIdentifier("media-workspace")
        .task(id: store.query) {
            if !store.query.isEmpty {
                do { try await Task.sleep(for: .milliseconds(250)) } catch { return }
            }
            await store.refresh()
        }
        .task(id: store.hasActiveJobs) {
            while store.hasActiveJobs {
                do { try await Task.sleep(for: .seconds(2)) } catch { return }
                await store.refresh()
            }
        }
        .task(id: store.selectedDetailKey) { await store.loadDetails() }
        // The detail side is never an empty panel while there is something to show.
        .task(id: store.records.first?.id) {
            if store.selectedID == nil, store.selectedRecordingID == nil, store.draft == nil, let newest = store.records.max(by: { $0.displayedDate < $1.displayedDate }) { store.selectedID = newest.id }
        }
        .fileImporter(isPresented: $importing, allowedContentTypes: [.audio, .png, .jpeg]) { result in
            switch result {
            case .success(let url): Task { await store.importFile(url) }
            case .failure(let error): store.error = error.localizedDescription
            }
        }
        .dropDestination(for: URL.self) { urls, _ in
            guard store.canCapture, urls.count == 1, let file = urls.first, file.isFileURL else { return false }
            Task { await store.importFile(file) }
            return true
        } isTargeted: { dropTargeted = $0 && store.canCapture }
        .confirmCapsule(L("丢弃待入库素材？"), isPresented: $confirmDiscard, confirmTitle: L("丢弃")) { store.discardDraft() }
        .confirmCapsule(L("删除「%@」？", deleting?.displayTitle ?? L("这条资料")), isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }),
                        message: L("这会删除资料库里的记录和对应的识别结果。")) {
            if let deleting { Task { await store.delete(deleting) } }
            deleting = nil
        }
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text(L("资料")).typography(.display)
            Text(store.isDemo ? L("%@ 条 · 合成样例", store.records.count) : L("%@ 条", store.records.count + store.pendingRecordingCount))
                .typography(.headline).foregroundStyle(.secondary)
            Spacer()
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass").typography(.secondary, .medium).foregroundStyle(.secondary)
                TextField(L("搜索文件名或转写"), text: $store.query)
                    .textFieldStyle(.plain).typography(.body)
                    .focused($searchFocused)
                    .accessibilityIdentifier("media-search")
                    .onChange(of: store.query) { _, value in
                        if value.count > 256 { store.query = String(value.prefix(256)) }
                    }
                if !store.query.isEmpty {
                    Button { store.query = "" } label: { Image(systemName: "xmark.circle.fill").foregroundStyle(.tertiary) }
                        .buttonStyle(.plain).accessibilityLabel(L("清空搜索"))
                }
            }
            .padding(.horizontal, 12).frame(width: searchFocused || !store.query.isEmpty ? 260 : 200, height: 32)
            .background(Capsule().fill(searchFocused ? Surface.hover.color : Surface.fill.color))
            .animation(Motion.bouncy, value: searchFocused)
            if store.busy { ProgressView().controlSize(.small).accessibilityLabel(L("正在处理")) }
            Button { importing = true } label: {
                Label(L("导入"), systemImage: "square.and.arrow.down").typography(.body, .semibold)
                    .padding(.horizontal, 14).frame(height: 32)
                    .background(Capsule().fill(Color.accentColor)).foregroundStyle(.white)
            }
            .buttonStyle(.plain)
            .disabled(!store.canCapture).opacity(store.canCapture ? 1 : 0.45)
            .keyboardShortcut("o", modifiers: .command)
            .help(L("导入录音或图片（⌘O），也可以直接拖进来"))
            .accessibilityIdentifier("media-import")
            ChromeButton(systemImage: "arrow.clockwise", help: L("刷新资料")) { Task { await store.syncRecordings(); await store.refresh() } }
        }
    }

    @ViewBuilder private var notices: some View {
        if let status = store.recordingNotice {
            notice(status, symbol: store.isRecording ? "mic.fill" : store.recordingFinishing ? "arrow.down.circle" : store.error == nil ? "checkmark.circle" : "exclamationmark.triangle",
                   tint: store.isRecording ? .red : store.recordingFinishing ? Palette.accent : store.error == nil ? Palette.success : Palette.warning, action: store.isRecording
                   ? (L("停止录音"), { Task { await store.stopRecording() } }) : (L("知道了"), { store.recordingNotice = nil }))
        }
        if store.recoveredRecordingCount > 0 {
            notice(L("已恢复 %@ 段已保存录音 · 启动时未自动开启麦克风", store.recoveredRecordingCount), symbol: "arrow.clockwise.circle", tint: Palette.success,
                   action: (L("查看"), {
                       if let recovered = store.recordingInventory.segments.last(where: \.recovered) {
                           if store.recordingInventory.pending.contains(where: { $0.id == recovered.id }) { store.selectedRecordingID = recovered.id }
                           else { store.selectedRecordingID = nil; store.selectedID = recovered.id }
                       }
                   }))
        }
        if store.pendingRecordingCount > 0 || store.recordingWarning != nil {
            notice(store.recordingWarning ?? L("%@ 段已保存在本机，等待入库。", store.pendingRecordingCount), symbol: "externaldrive",
                   action: (store.recordingSyncing ? L("正在入库") : L("重试入库"), { Task { await store.syncRecordings(); await store.refresh() } }))
        }
        if let error = store.error {
            notice(error, symbol: "exclamationmark.triangle.fill", action: (L("知道了"), { store.error = nil }))
        }
        if let error = store.connectionError, !store.isDemo, store.pendingRecordingCount == 0 {
            notice(error, symbol: "externaldrive.badge.exclamationmark", action: (L("重新连接"), { Task { await store.refresh() } }))
        }
    }

    private func notice(_ text: String, symbol: String, tint: Color = Palette.warning, action: (String, () -> Void)) -> some View {
        HStack(spacing: 10) {
            Image(systemName: symbol).foregroundStyle(tint)
            Text(text).typography(.secondary).lineLimit(2)
            Spacer()
            Button(action.0, action: action.1).buttonStyle(.borderless).typography(.secondary, .semibold)
        }
        .padding(.horizontal, 14).padding(.vertical, 9)
        .background(tint.opacity(0.1), in: Capsule())
        .padding(.top, 12)
    }

    // MARK: List

    private var groups: [(title: String, items: [MediaLibraryRecord])] {
        let calendar = Calendar.current
        let grouped = Dictionary(grouping: store.records) { calendar.startOfDay(for: $0.displayedDate) }
        return grouped.keys.sorted(by: >).map { day in
            let title = calendar.isDateInToday(day) ? L("今天") : calendar.isDateInYesterday(day) ? L("昨天")
                : day.formatted(.dateTime.month(.defaultDigits).day().weekday(.abbreviated))
            return (title, grouped[day]!.sorted { $0.displayedDate > $1.displayedDate })
        }
    }

    @ViewBuilder private var list: some View {
        if store.records.isEmpty && store.draft == nil && store.pendingRecordingCount == 0 {
            VStack(spacing: 10) {
                Image(systemName: store.query.isEmpty ? "tray" : "magnifyingglass").typography(.display, .light).foregroundStyle(.tertiary)
                Text(store.query.isEmpty ? L("拖入录音或图片，或用右上角录一段、截一块") : L("没有匹配「%@」的资料", store.query))
                    .typography(.body).foregroundStyle(.secondary).multilineTextAlignment(.center)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 2) {
                    if let draft = store.draft {
                        sectionTitle(L("待保存"))
                        row(selected: store.selectedID == nil, glyph: draft.kind == "audio" ? .audio : .image,
                            title: draft.filename, detail: L("确认后才会保存到资料库")) { store.selectedID = nil }
                    }
                    if store.pendingRecordingCount > 0 {
                        sectionTitle(L("本机已保存 · 待入库"))
                        ForEach(store.pendingRecordingRows) { segment in
                            row(selected: store.selectedRecordingID == segment.id, glyph: .audio,
                                title: L("Mac 录音 ") + segment.occurredAt.formatted(date: .abbreviated, time: .shortened), detail: segment.recovered ? L("中断后恢复 · 可回听") : L("已保存 · 可回听")) {
                                store.selectedRecordingID = segment.id
                            }
                        }
                    }
                    ForEach(groups, id: \.title) { group in
                        sectionTitle(group.title)
                        ForEach(group.items) { item in
                            row(selected: store.selectedID == item.id && store.draft == nil, glyph: item.kind == .audio ? .audio : .image,
                                title: item.displayTitle,
                                detail: "\(item.displayedDate.clockText) · \(item.source.label)" + (store.recordingLabel(for: item.id).map { " · " + $0 } ?? (item.kind == .audio ? " · " + item.processingStatus.label : "")),
                                busy: item.processingStatus.isActive && item.processing?.isWaiting != true) { store.selectedRecordingID = nil; store.selectedID = item.id }
                                .contextMenu {
                                    MediaContextActions(item: item, store: store, edit: {
                                        metadataEditTarget = item.id; metadataEditRequest = UUID()
                                        store.selectedRecordingID = nil; store.selectedID = item.id
                                    }, remove: { deleting = item })
                                }
                        }
                    }
                }
                .padding(.vertical, 4)
            }
            .scrollIndicators(.never)
            .accessibilityIdentifier("media-timeline")
        }
    }

    private func sectionTitle(_ title: String) -> some View {
        Text(title).typography(.secondary, .semibold).foregroundStyle(.secondary)
            .padding(.horizontal, 12).padding(.top, 14).padding(.bottom, 4)
    }

    private func row(selected: Bool, glyph: TimelineEpisode.Kind, title: String, detail: String, busy: Bool = false, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 11) {
                MediaGlyph(kind: glyph, size: 30)
                VStack(alignment: .leading, spacing: 3) {
                    Text(verbatim: title).typography(.body, .medium).lineLimit(1)
                    Text(detail).typography(.secondary).foregroundStyle(.secondary).lineLimit(1).monospacedDigit()
                }
                Spacer(minLength: 0)
                if busy { ProgressView().controlSize(.mini) }
            }
            .padding(.horizontal, 12).padding(.vertical, 9)
            .background(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous).fill(selected ? Surface.selected.color : .clear))
            .contentShape(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous))
        }
        .buttonStyle(.plain)
        .animation(Motion.snappy, value: selected)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    @ViewBuilder private var detail: some View {
        if let segment = store.recordingInventory.pending.first(where: { $0.id == store.selectedRecordingID }) {
            SavedRecordingView(segment: segment, url: store.recordingJournal.file(segment, ".wav"), store: store)
                .id(segment.id)
        } else if let draft = store.draft, let url = store.draftFile, store.selectedID == nil || store.records.isEmpty {
            MediaDraftView(draft: draft, url: url, busy: store.busy, save: {
                Task { await store.saveDraft() }
            }, discard: { confirmDiscard = true })
            .id(draft.id)
        } else if let item = store.selected {
            MediaRecordView(item: item, store: store, requestDelete: { deleting = item }, showInTimeline: showInTimeline,
                            editRequest: metadataEditTarget == item.id ? metadataEditRequest : nil)
                .id(item.id)
                .transition(.opacity)
        } else if store.records.isEmpty && store.query.isEmpty {
            VStack(spacing: Space.s) {
                Text(L("还没有资料")).typography(.headline)
                Text(L("录音、框选截图和导入的文件会出现在这里。")).typography(.secondary).foregroundStyle(.secondary)
                PrimaryButton(title: L("导入文件")) { importing = true }.disabled(!store.canCapture)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            Text(store.records.isEmpty ? L("没有匹配的资料") : L("选一条资料")).typography(.body).foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

private struct SavedRecordingView: View {
    let segment: RecordingSegment
    let url: URL
    let store: MediaWorkspaceStore
    @ViewState private var cache = FileManager.default.temporaryDirectory.appendingPathComponent("recording-local-" + UUID().uuidString)
    @StateObject private var playback = PlaybackController()

    var body: some View {
        VStack(alignment: .leading, spacing: Space.m) {
            Text(segment.recovered ? L("中断后恢复的录音") : L("已保存的录音")).typography(.section)
            Text(L("Mac 录音 ") + segment.occurredAt.formatted(date: .abbreviated, time: .shortened)).typography(.body).textSelection(.enabled)
            Text(store.recordingLabel(for: segment.recordingID) ?? L("本机已保存")).typography(.secondary).foregroundStyle(.secondary)
            MediaPlaybackControls(playback: playback) {
                Task {
                    let parts = store.recordingInventory.pending.filter { $0.recordingID == segment.recordingID }
                    let revision = parts.map { $0.id.uuidString }.joined(separator: ":")
                    await playback.toggleFile(id: segment.recordingID.uuidString + revision) {
                        let destination = cache.appendingPathComponent("playback.wav")
                        let journal = store.recordingJournal
                        return try await Task.detached(priority: .utility) {
                            try RecordingPlaybackCopy.make(journal: journal, parts: parts, to: destination)
                        }.value
                    }
                }
            }
            Text(L("待入库 · 已保存部分可回听")).typography(.secondary).foregroundStyle(.secondary)
            QuietButton(title: store.recordingSyncing ? L("正在入库") : L("重试入库"), emphasized: true) {
                playback.stop()
                Task { await store.syncRecordings(); await store.refresh() }
            }.disabled(store.recordingSyncing)
            Spacer()
        }
        .padding(Space.xl).frame(maxWidth: .infinity, alignment: .leading)
        .onDisappear { playback.stop(); try? FileManager.default.removeItem(at: cache) }
    }
}

private struct MediaDraftView: View {
    let draft: MediaDraft
    let url: URL
    let busy: Bool
    let save: () -> Void
    let discard: () -> Void
    @StateObject private var playback = PlaybackController()

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Text(draft.needsReview ? L("确认素材") : L("待入库素材")).typography(.section)
                Text(verbatim: draft.filename).foregroundStyle(.secondary)
                if draft.kind == "audio" {
                    MediaPlaybackControls(playback: playback) {
                        Task { await playback.toggleFile(id: draft.id.uuidString) { url } }
                    }
                } else {
                    MediaImagePreview(url: url)
                }
                HStack(spacing: 6) {
                    PrimaryButton(title: draft.needsReview ? L("保存到时间线") : L("重试入库")) { playback.stop(); save() }
                        .accessibilityIdentifier("media-save-draft")
                    QuietButton(title: L("丢弃…"), role: .destructive, action: discard)
                }.disabled(busy)
            }.frame(maxWidth: .infinity, alignment: .leading).padding(22)
        }.onDisappear { playback.stop() }
    }
}

struct MediaRecordView: View {
    let item: MediaLibraryRecord
    let store: MediaWorkspaceStore
    let requestDelete: () -> Void
    var showInTimeline: ((Date) -> Void)? = nil
    /// The home timeline detail already shows title and time.
    var showsHeader = true
    var editRequest: UUID? = nil
    @ViewState private var previewDirectory = FileManager.default.temporaryDirectory
        .appendingPathComponent("candid-preview-" + UUID().uuidString, isDirectory: true)
    @ViewState private var imageURL: URL?
    @ViewState private var previewError: String?
    @ViewState private var waveform: [Float] = []
    @ViewState private var editing = false
    @ViewState private var editedTitle = ""
    @ViewState private var editedNotes = ""
    @StateObject private var playback = PlaybackController()

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                if showsHeader {
                    HStack(alignment: .center, spacing: 12) {
                        MediaGlyph(kind: item.kind == .audio ? .audio : .image, size: 38)
                        VStack(alignment: .leading, spacing: 3) {
                            Text(verbatim: item.displayTitle).typography(.section).textSelection(.enabled).lineLimit(1)
                                .accessibilityIdentifier("media-record-title")
                            Text(item.source.label + " · " + item.displayedDate.formatted(date: .abbreviated, time: .shortened)
                                 + (item.occurredAt == nil ? L(" · 按导入时间排列") : ""))
                                .typography(.secondary).foregroundStyle(.secondary)
                        }
                        Spacer()
                        if let showInTimeline {
                            Button { playback.stop(); showInTimeline(item.displayedDate) } label: {
                                Label(L("在时间线中查看"), systemImage: "point.topleft.down.to.point.bottomright.curvepath")
                                    .typography(.secondary, .semibold).padding(.horizontal, 12).frame(height: 28)
                                    .background(Capsule().fill(Surface.hover.color))
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .padding(.bottom, 6)
                }
                if editing {
                    VStack(alignment: .leading, spacing: Space.s) {
                        QuietField(title: L("标题"), text: $editedTitle)
                        Text(L("备注")).typography(.secondary).foregroundStyle(.secondary)
                        TextField(L("备注"), text: $editedNotes, axis: .vertical)
                            .textFieldStyle(.plain).typography(.body).accessibilityLabel(L("资料备注"))
                            .padding(Space.s)
                            .background(RoundedRectangle(cornerRadius: Radius.small).fill(Surface.fill.color))
                        HStack {
                            QuietButton(title: L("保存")) {
                                Task {
                                    await store.edit(item, title: editedTitle, notes: editedNotes)
                                    if store.error == nil { editing = false }
                                }
                            }.disabled(store.busy || editedTitle.count > 255 || editedNotes.count > 20000)
                            QuietButton(title: L("取消")) { editing = false }
                        }
                    }
                } else {
                    if let notes = item.notes, !notes.isEmpty { Text(verbatim: notes).typography(.body).textSelection(.enabled) }
                    QuietButton(title: L("编辑标题与备注")) {
                        editedTitle = item.displayTitle; editedNotes = item.notes ?? ""; editing = true
                    }.disabled(store.isDemo || store.busy)
                }
                if let label = store.recordingLabel(for: item.id) {
                    Label(label, systemImage: "checkmark.circle").typography(.secondary).foregroundStyle(.secondary)
                }
                if item.kind == .audio {
                    MediaPlaybackControls(playback: playback) { play() }
                    if let previewError { Text(previewError).typography(.secondary).foregroundStyle(.secondary) }
                    if let parts = item.recordingParts {
                        RecordingSampleStrip(parts: parts, gaps: item.gaps ?? [], duration: item.durationSeconds ?? 0,
                                             position: playback.position, peaks: waveform) { play(at: $0) }
                    }
                    if item.recordingState == "recovered" || item.recordingState == "fault" {
                        Text(L("录音中断 · 已保存部分可回听")).typography(.secondary).foregroundStyle(.secondary)
                    }
                    ForEach(Array((item.gaps ?? []).enumerated()), id: \.offset) { _, gap in
                        Text(L("缺少录音 %@–%@", mediaTime(gap.startSeconds), mediaTime(gap.endSeconds)))
                            .typography(.secondary).foregroundStyle(.secondary)
                    }
                    HairlineDivider()
                    RecordingProcessingView(item: item, store: store)
                    if item.speakers?.isEmpty == false {
                        HairlineDivider()
                        RecordingSpeakersView(item: item, store: store)
                    }
                    HairlineDivider()
                    recognition
                } else {
                    if let imageURL { MediaImagePreview(url: imageURL) }
                    else if let previewError { Label(previewError, systemImage: "exclamationmark.triangle") }
                    else { ProgressView().accessibilityLabel(L("正在加载图片")) }
                }
                QuietButton(title: L("删除这条资料…"), role: .destructive) { playback.stop(); requestDelete() }
                    .disabled(store.busy)
                    .padding(.top, Space.xs).offset(x: -9)
            }
            .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 26).padding(.vertical, 24)
        }
        .onChange(of: editRequest, initial: true) { _, request in
            if request != nil { editedTitle = item.displayTitle; editedNotes = item.notes ?? ""; editing = true }
        }
        .task(id: item.contentRevision) {
            if item.kind == .audio, item.recordingParts != nil, !store.isDemo {
                do {
                    let url = try await store.preview(item, directory: previewDirectory)
                    waveform = try await Task.detached(priority: .utility) { try RecordingWaveform.peaks(url) }.value
                } catch { if !Task.isCancelled { previewError = error.localizedDescription } }
                return
            }
            guard item.kind == .image else { return }
            do { imageURL = try await store.preview(item, directory: previewDirectory) }
            catch { if !Task.isCancelled { previewError = error.localizedDescription } }
        }
        .onDisappear {
            playback.stop()
            try? FileManager.default.removeItem(at: previewDirectory)
        }
    }

    private var recognition: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let error = item.error, item.processingStatus == .failed {
                Label(error, systemImage: "exclamationmark.triangle").typography(.secondary)
            }
            if item.processingStatus == .ready, item.segments.isEmpty {
                if store.hasSelectedDetails {
                    Text(L("这段录音没有可用的转写。")).typography(.secondary).foregroundStyle(.secondary)
                } else {
                    ProgressView().controlSize(.small).accessibilityLabel(L("正在加载转写"))
                }
            }
            ForEach(Array(item.segments.enumerated()), id: \.offset) { _, segment in
                let current = playback.isPlaying && playback.position >= segment.startSeconds && playback.position < max(segment.endSeconds, segment.startSeconds + 1)
                HStack(alignment: .firstTextBaseline, spacing: 12) {
                    Button(mediaTime(segment.startSeconds)) { play(at: segment.startSeconds) }
                        .buttonStyle(.plain).typography(.secondary, .semibold).monospaced()
                        .foregroundStyle(.tint)
                        .disabled(playback.loading)
                        .accessibilityLabel(L("从 %@ 回听", mediaTime(segment.startSeconds)))
                        .frame(width: 44, alignment: .leading)
                    VStack(alignment: .leading, spacing: 3) {
                        if let speaker = segment.shownSpeaker {
                            Text(speaker)
                                .typography(.secondary, .semibold).foregroundStyle(.secondary)
                        }
                        Text(verbatim: segment.text).typography(.headline).lineSpacing(3).textSelection(.enabled)
                    }
                }
                .padding(.vertical, 6).padding(.horizontal, 10)
                .background(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous).fill(Color.accentColor.opacity(current ? 0.12 : 0)))
                .animation(.easeOut(duration: 0.2), value: current)
            }
        }
    }

    private func play(at time: Double? = nil) {
        Task {
            await playback.toggleFile(id: item.id.uuidString + (item.contentRevision ?? ""), startAt: time) {
                try await store.preview(item, directory: previewDirectory)
            }
        }
    }
}

private struct MediaPlaybackControls: View {
    @ObservedObject var playback: PlaybackController
    let toggle: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 10) {
                Button(action: toggle) {
                    Image(systemName: playback.isPlaying ? "pause.fill" : "play.fill")
                        .typography(.headline, .bold).foregroundStyle(.white)
                        .frame(width: 38, height: 38)
                        .background(Circle().fill(Color.accentColor))
                        .contentTransition(.symbolEffect(.replace))
                }
                .buttonStyle(.plain)
                .disabled(playback.loading)
                .accessibilityLabel(playback.isPlaying ? L("暂停") : L("播放"))
                .accessibilityIdentifier("media-play")
                if playback.loading { ProgressView().controlSize(.mini) }
                if playback.duration > 0 {
                    Slider(value: Binding(get: { playback.position }, set: playback.seek), in: 0...playback.duration)
                        .accessibilityLabel(L("播放位置"))
                    Text("\(mediaTime(playback.position)) / \(mediaTime(playback.duration))")
                        .typography(.caption, .regular).monospaced().foregroundStyle(.secondary)
                }
            }
            if let error = playback.error { Text(error).typography(.secondary).foregroundStyle(.secondary) }
        }
    }
}

private struct MediaImagePreview: View {
    let url: URL
    /// Decoded once per file, off the main thread; redraws reuse it.
    @ViewState private var image: CGImage?
    @ViewState private var failed = false

    var body: some View {
        Group {
            if let image {
                Image(decorative: image, scale: 1).resizable().scaledToFit()
                    .frame(maxWidth: .infinity, maxHeight: 480)
                    .accessibilityLabel(L("已保存的图片"))
            } else if failed {
                Label(L("无法预览此图片"), systemImage: "photo.badge.exclamationmark")
            } else {
                ProgressView().controlSize(.small).frame(maxWidth: .infinity, minHeight: 120)
            }
        }
        .task(id: url) {
            image = nil; failed = false
            let url = self.url
            let decoded = await Task.detached(priority: .userInitiated) { () -> CGImage? in
                guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else { return nil }
                return CGImageSourceCreateThumbnailAtIndex(source, 0, [
                    kCGImageSourceCreateThumbnailFromImageAlways: true,
                    kCGImageSourceCreateThumbnailWithTransform: true,
                    kCGImageSourceThumbnailMaxPixelSize: 1800
                ] as CFDictionary)
            }.value
            guard !Task.isCancelled else { return }
            image = decoded; failed = decoded == nil
        }
    }
}

func mediaTime(_ seconds: Double) -> String {
    let whole = max(0, Int(seconds.rounded(.down)))
    return String(format: "%d:%02d", whole / 60, whole % 60)
}
