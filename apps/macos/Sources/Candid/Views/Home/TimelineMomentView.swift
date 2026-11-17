import AppKit
import CandidLocalization
import DesktopCore
import SwiftUI

/// The stage above the lanes: what was happening under the playhead. Text on
/// the left (app, title, time and what else happened then), one calm preview
/// on the right with the neighbouring episodes faint at the edges. The whole
/// stack of screenshots lives on the screenshot lane below, aligned by time.
struct TimelineMomentView: View {
    let scene: TimelineScene
    let cursor: TimelineCursor
    let cache: TimelineMediaCache
    var prompts: [TimelinePromptMark] = []
    let onOpen: (TimelineTarget) -> Void
    let onJump: (Date) -> Void
    var onStep: (Int) -> Void = { _ in }
    var contextActions: (TimelineEpisode) -> AnyView = { _ in AnyView(EmptyView()) }
    /// False while the user drags or wheels the lanes: the stage then cuts from episode to episode instead of
    /// starting a cross-fade for each one it passes (dozens of overlapping springs in a fast drag).
    var animated = true
    /// A horizontal swipe at least this long steps one episode.
    static let swipeStep: CGFloat = 60

    /// Reads the playhead and reduces it to what the stage shows: the episode, the screenshot, the spoken line.
    /// The stage itself is skipped (and keeps its rendering) while those stay the same, which is most of a drag.
    var body: some View {
        let _ = TimelinePerf.tick(.stageBody)
        let playhead = cursor.playhead
        let index = scene.episodeIndex(at: playhead)
        let episode = Self.episode(at: playhead, index: index, in: scene)
        let shot = episode.flatMap { $0.kind == .app ? screenshot(in: $0, at: playhead) : nil }
        let gap: MomentStage.Gap? = episode == nil ? {
            let before = scene.episodes.last { $0.end <= playhead }, after = scene.episodes.first { $0.start > playhead }
            guard let before, let after else { return nil }
            return MomentStage.Gap(start: before.end, end: after.start)
        }() : nil
        // A recording follows the playhead (waveform and words) in quarter seconds; nothing else needs the exact time.
        let audioTime = episode?.kind == .audio ? Date(timeIntervalSince1970: (playhead.timeIntervalSince1970 * 4).rounded(.down) / 4) : nil
        MomentStage(scene: scene, cache: cache, prompts: prompts, index: index, episode: episode,
                    shot: shot.map { MomentStage.Shot(image: $0.image, date: $0.date) },
                    line: scene.audio(at: playhead)?.line(at: playhead)?.line, audioTime: audioTime, gap: gap, animated: animated,
                    onOpen: onOpen, onJump: onJump, onStep: onStep, contextActions: contextActions)
            .equatable()
    }

    /// The episode under the playhead; between episodes the previous one stays
    /// unless the time is a recorded pause or a stretch longer than ten minutes.
    static func episode(at playhead: Date, index: Int?, in scene: TimelineScene) -> TimelineEpisode? {
        guard let index else { return nil }
        let candidate = scene.episodes[index]
        if candidate.contains(playhead) { return candidate }
        if scene.gap(at: playhead) != nil { return nil }
        let hasLater = scene.episodes.contains { $0.start > playhead }
        return !hasLater || playhead.timeIntervalSince(candidate.end) < 600 ? candidate : nil
    }

    /// The screenshot the playhead is on: inside the span under it if that
    /// span has screenshots, else the latest one of the episode.
    private func screenshot(in episode: TimelineEpisode, at playhead: Date) -> (image: NSImage, date: Date)? {
        let candidates = episode.screenshotSpans
        guard !candidates.isEmpty else { return nil }
        let span = candidates.last { $0.start <= playhead } ?? candidates[0]
        if let records = cache.screenshots(for: span), !records.isEmpty {
            let record = records.last { $0.timestamp <= playhead } ?? records[0]
            if let image = cache.image(for: record, maxPixel: 1100) {
                if !cache.hasImage(for: record, maxPixel: 1100) { cache.requestImage(for: record, maxPixel: 1100) }
                return (image, record.timestamp)
            }
            cache.requestImage(for: record, maxPixel: 1100)
        }
        return cache.latestImage(for: span).map { ($0, span.start) }
    }
}

/// What the stage draws for one moment. Equatable on its content (closures aside), so a playhead move that keeps
/// the same episode, screenshot and line costs nothing here.
private struct MomentStage: View, @MainActor Equatable {
    struct Shot: Equatable {
        let image: NSImage
        let date: Date
        static func == (a: Shot, b: Shot) -> Bool { a.image === b.image && a.date == b.date }
    }
    struct Gap: Equatable { let start: Date; let end: Date }

    let scene: TimelineScene
    let cache: TimelineMediaCache
    let prompts: [TimelinePromptMark]
    let index: Int?
    let episode: TimelineEpisode?
    let shot: Shot?
    let line: TimelineTranscriptLine?
    let audioTime: Date?
    let gap: Gap?
    let animated: Bool
    let onOpen: (TimelineTarget) -> Void
    let onJump: (Date) -> Void
    let onStep: (Int) -> Void
    let contextActions: (TimelineEpisode) -> AnyView

    static func == (a: MomentStage, b: MomentStage) -> Bool {
        a.index == b.index && a.episode == b.episode && a.shot == b.shot && a.line == b.line && a.audioTime == b.audioTime
            && a.gap == b.gap && a.animated == b.animated && a.prompts == b.prompts && a.scene == b.scene && a.cache === b.cache
    }

    var body: some View {
        GeometryReader { geo in
            Group {
                if geo.size.height < 190 {
                    compact.frame(maxHeight: .infinity, alignment: .bottom)
                } else {
                    HStack(alignment: .center, spacing: Space.xl + Space.xs) {
                        info.frame(width: min(340, max(260, geo.size.width * 0.28)), alignment: .leading)
                        preview(height: geo.size.height)
                    }
                    .frame(maxHeight: .infinity)
                }
            }
            .frame(width: geo.size.width, height: geo.size.height)
        }
        .onTrackpadSwipe { update in
            if update.phase == .ended, update.axis == .horizontal, abs(update.translation) >= TimelineMomentView.swipeStep {
                onStep(update.translation < 0 ? 1 : -1)
            }
        }
        .animation(animated ? Motion.spring : nil, value: episode?.id)
    }

    // MARK: Text side

    @ViewBuilder private var info: some View {
        if let episode {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: Space.xs) {
                    icon(episode, size: 16)
                    Text(episode.appName).typography(.secondary).foregroundStyle(.secondary)
                }
                Text(episode.title).typography(.section).font(TypeScale.title.font(.semibold)).lineLimit(2)
                    .padding(.top, Space.xs).padding(.bottom, Space.xxs)
                    .contentTransition(.opacity)
                HStack(spacing: 6) {
                    if isLive(episode) { Circle().fill(Palette.recording).frame(width: 6, height: 6); Text(L("进行中 ·")) }
                    Text("\(episode.start.clockText) – \(episode.end.clockText) · \(episode.duration.spokenDuration)").monospacedDigit()
                }
                .typography(.secondary).foregroundStyle(.secondary)
                VStack(alignment: .leading, spacing: 2) { facts(episode) }.padding(.top, Space.m)
                if let line {
                    HStack(alignment: .top, spacing: Space.xs) {
                        if let speaker = line.speaker { Text(speaker).typography(.body, .medium) }
                        Text(line.text).typography(.body).foregroundStyle(.secondary).lineLimit(3)
                    }
                    .padding(.leading, Space.s)
                    .overlay(alignment: .leading) { Capsule().fill(MediaGlyph.tint(.audio)).frame(width: 2) }
                    .padding(.top, Space.s)
                    .transition(.opacity)
                }
            }
        } else {
            VStack(alignment: .leading, spacing: Space.xxs) {
                Text(L("未记录")).typography(.section).font(TypeScale.title.font(.semibold))
                if let gap { Text("\(gap.start.clockText) – \(gap.end.clockText)").typography(.secondary, digits: true).foregroundStyle(.secondary) }
            }
        }
    }

    @ViewBuilder private func facts(_ episode: TimelineEpisode) -> some View {
        let overlapping = scene.episodes.filter { $0.kind == .audio && $0.id != episode.id && $0.start < episode.end && $0.end > episode.start }
        ForEach(overlapping) { audio in
            FactRow(systemImage: "waveform", tint: MediaGlyph.tint(.audio), title: audio.title,
                    detail: "\(audio.start.clockText)–\(audio.end.clockText)") { onOpen(.episode(audio.id)) }
        }
        let inside = prompts.filter { $0.kind != .work && $0.start >= episode.start && $0.start < episode.end }
        ForEach(inside.suffix(3)) { mark in
            FactRow(systemImage: mark.kind == .note ? "note.text" : "sparkle", tint: AIGradient.colors[1], title: mark.preview ?? L("AI 指令"), detail: nil) {
                onOpen(.prompt(mark.id))
            }
        }
        if inside.count > 3 {
            FactRow(systemImage: "sparkle", tint: AIGradient.colors[1], title: L("另有 %@ 条指令", inside.count - 3), detail: nil, secondaryTitle: true) {
                onJump(inside[0].start)
            }
        }
        if episode.kind == .app {
            let shots = episode.screenshotSpans.count
            FactRow(systemImage: "photo", tint: .secondary, title: shots > 0 ? L("%@ 段有截图", shots) : L("没有截图"),
                    detail: episode.interruptions.isEmpty ? nil : L("中途切换 %@ 次", episode.interruptions.count)) { onOpen(.episode(episode.id)) }
        }
    }

    // MARK: Preview side

    private func preview(height: CGFloat) -> some View {
        GeometryReader { geo in
            let frameHeight = min(geo.size.height * 0.86, 320, geo.size.width / 1.6 * 0.78)
            let ghostHeight = min(frameHeight * 0.62, 220)
            ZStack {
                if let index = index {
                    ghost(neighbour(of: index, step: -1), height: ghostHeight, leading: true)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    ghost(neighbour(of: index, step: 1), height: ghostHeight, leading: false)
                        .frame(maxWidth: .infinity, alignment: .trailing)
                }
                if let episode {
                    focusedPreview(episode)
                        .frame(width: frameHeight * 1.6, height: frameHeight)
                        .id(episode.id)
                        .transition(.opacity.combined(with: .scale(scale: 0.98)))
                } else {
                    Text(L("这段时间没有屏幕记录")).typography(.body).foregroundStyle(.secondary)
                }
            }
            .frame(width: geo.size.width, height: geo.size.height)
        }
    }

    @ViewBuilder private func focusedPreview(_ episode: TimelineEpisode) -> some View {
        Group {
            switch episode.kind {
            case .audio: AudioEpisodeCard(episode: episode, playhead: audioTime ?? episode.start, focused: true)
            case .image: ImageEpisodeCard(episode: episode, cache: cache, focused: true)
            case .app:
                ZStack(alignment: .bottomTrailing) {
                    LinearGradient(colors: [AppTint.color(for: episode.bundleID).opacity(0.4), AppTint.color(for: episode.bundleID).opacity(0.12)],
                                   startPoint: .topLeading, endPoint: .bottomTrailing)
                    if let shot {
                        Image(nsImage: shot.image).resizable().interpolation(.high).aspectRatio(contentMode: .fill)
                            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top).clipped()
                        Text(shot.date.clockText).typography(.caption, .medium).monospacedDigit().foregroundStyle(.white)
                            .padding(.horizontal, Space.xs).padding(.vertical, 2)
                            .background(Capsule().fill(.black.opacity(0.45))).padding(Space.s)
                    } else {
                        DesktopApplicationIcon(bundleID: episode.bundleID, fallback: "app", size: 64)
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                    }
                }
                .clipShape(RoundedRectangle(cornerRadius: Radius.large, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: Radius.large, style: .continuous).strokeBorder(Surface.hairline, lineWidth: 0.5))
                .shadow(color: .black.opacity(0.12), radius: 14, y: 6)
            }
        }
        .contentShape(RoundedRectangle(cornerRadius: Radius.large, style: .continuous))
        .onTapGesture { onOpen(.episode(episode.id)) }
        .contextMenu { contextActions(episode) }
        .help(L("打开详情（回车）"))
    }

    @ViewBuilder private func ghost(_ episode: TimelineEpisode?, height: CGFloat, leading: Bool) -> some View {
        if let episode {
            ZStack {
                LinearGradient(colors: [AppTint.color(for: episode.bundleID).opacity(0.4), AppTint.color(for: episode.bundleID).opacity(0.1)],
                               startPoint: .topLeading, endPoint: .bottomTrailing)
                if let span = episode.screenshotSpans.last, let image = cache.latestImage(for: span) {
                    Image(nsImage: image).resizable().aspectRatio(contentMode: .fill)
                }
            }
            .frame(width: height * 1.6, height: height).clipped()
            .clipShape(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous))
            .mask(LinearGradient(colors: [.black, .clear], startPoint: leading ? .trailing : .leading, endPoint: leading ? .leading : .trailing))
            .saturation(0.5).opacity(0.22)
            .contentShape(Rectangle())
            .onTapGesture { onJump(episode.start.addingTimeInterval(1)) }
            .accessibilityLabel((leading ? L("上一段：") : L("下一段：")) + episode.title)
            .accessibilityAddTraits(.isButton)
        }
    }

    // MARK: Compact (tall dock)

    private var compact: some View {
        HStack(spacing: Space.s) {
            if let episode {
                icon(episode, size: 18)
                Text(episode.title).typography(.headline).lineLimit(1)
                Text("\(episode.start.clockText)–\(episode.end.clockText)").typography(.secondary, digits: true).foregroundStyle(.secondary)
                let count = prompts.filter { $0.kind != .work && $0.start >= episode.start && $0.start < episode.end }.count
                if count > 0 { Text(L("· %@ 条 AI 指令", count)).typography(.secondary).foregroundStyle(.secondary) }
                if let line {
                    Text("· " + line.text).typography(.secondary).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer(minLength: 0)
                QuietButton(title: L("打开")) { onOpen(.episode(episode.id)) }
            } else {
                Text(L("未记录")).typography(.headline).foregroundStyle(.secondary)
                Spacer(minLength: 0)
            }
        }
        .padding(.bottom, Space.xxs)
    }

    // MARK: Helpers

    @ViewBuilder private func icon(_ episode: TimelineEpisode, size: CGFloat) -> some View {
        if episode.kind == .app { DesktopApplicationIcon(bundleID: episode.bundleID, fallback: "app", size: size) }
        else { MediaGlyph(kind: episode.kind, size: size) }
    }

    private func isLive(_ episode: TimelineEpisode) -> Bool {
        episode.kind == .app && episode.id == scene.episodes.last(where: { $0.kind == .app })?.id && scene.bounds.upperBound > Date().addingTimeInterval(-60)
            && Calendar.current.isDateInToday(episode.start)
    }

    private func neighbour(of index: Int, step: Int) -> TimelineEpisode? {
        var i = index + step
        while i >= 0, i < scene.episodes.count {
            if scene.episodes[i].kind == .app { return scene.episodes[i] }
            i += step
        }
        return nil
    }

}

/// One line of "what else happened then": icon, title, quiet detail; the
/// whole row is a button with a hover tint.
struct FactRow: View {
    let systemImage: String
    let tint: Color
    let title: String
    let detail: String?
    var secondaryTitle = false
    let action: () -> Void
    @ViewState private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: Space.s) {
                Image(systemName: systemImage).foregroundStyle(tint).frame(width: 16)
                Text(title).foregroundStyle(secondaryTitle ? .secondary : .primary).lineLimit(1)
                if let detail { Text(detail).foregroundStyle(.secondary).monospacedDigit().lineLimit(1) }
                Spacer(minLength: 0)
            }
            .typography(.body)
            .padding(.vertical, 6).padding(.horizontal, Space.xs)
            .background(RoundedRectangle(cornerRadius: Radius.small, style: .continuous).fill(hovering ? Surface.hover.color : .clear))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(.horizontal, -Space.xs)
        .onHover { hovering = $0 }
    }
}

/// An AI instruction opened from the AI lane, in place of the stage. The
/// reply is read from its source on request and never stored here.
struct AIPromptDetailView: View {
    let model: AIInteractionTimeline
    let mark: TimelinePromptMark?
    let scene: TimelineScene
    /// This instruction's place among the day's instructions (1-based) and how many there are.
    var position: (index: Int, count: Int)?
    var onStep: (Int) -> Void = { _ in }
    let onOpen: (TimelineTarget) -> Void
    let onClose: () -> Void

    var body: some View {
        ZStack(alignment: .topLeading) {
            RoundedRectangle(cornerRadius: Radius.card, style: .continuous).fill(.regularMaterial)
                .overlay(RoundedRectangle(cornerRadius: Radius.card, style: .continuous).strokeBorder(Surface.hover.color, lineWidth: 0.5))
                .shadow(color: .black.opacity(0.14), radius: 34, y: 14)
            VStack(alignment: .leading, spacing: Space.m) {
                header
                HStack(alignment: .top, spacing: Space.l) {
                    ScrollView {
                        VStack(alignment: .leading, spacing: Space.s) {
                            if let detail = model.detail {
                                if let note = model.sourceNote { InlineMessage(kind: .note, text: note) }
                                // The instruction as one compact block (empty paragraphs and blank runs dropped).
                                let prompt = AIReplyText.normalize(detail.paragraphs.map(\.text))
                                if !prompt.isEmpty {
                                    AIReplyView(text: prompt)
                                        .padding(Space.m).frame(maxWidth: .infinity, alignment: .leading)
                                        .background(RoundedRectangle(cornerRadius: Radius.large, style: .continuous).fill(Surface.fill.color))
                                }
                                if let reply = model.reply {
                                    SectionLabel(L("AI 的回复 · 从会话文件现读，不存进时间线"))
                                    AIReplyView(text: reply)
                                        .padding(Space.m).frame(maxWidth: .infinity, alignment: .leading)
                                        .overlay(RoundedRectangle(cornerRadius: Radius.large, style: .continuous).strokeBorder(Surface.hairline))
                                } else if model.replyLoading {
                                    HStack(spacing: Space.s) {
                                        ProgressView().controlSize(.small)
                                        Text(L("正在读取回复…")).typography(.secondary).foregroundStyle(.secondary)
                                    }
                                }
                            } else if let error = model.error {
                                InlineMessage(kind: .warning, text: error)
                            } else {
                                ProgressView().controlSize(.small)
                            }
                        }
                    }
                    side.frame(width: 260)
                }
            }
            .padding(Space.l)
        }
        // The reply is read as soon as the instruction is shown; it stays out of the timeline's storage.
        .task(id: model.detail?.event?.id) {
            if model.detail != nil, model.reply == nil, model.canReadReply { await model.readReply() }
        }
    }

    private var header: some View {
        HStack(spacing: Space.s) {
            Text(mark?.kind == .note ? L("笔记") : L("AI 指令")).typography(.section)
                .foregroundStyle(LinearGradient(colors: AIGradient.colors, startPoint: .leading, endPoint: .trailing))
            if let mark { Text(mark.end.map { "\(mark.start.clockText) – \($0.clockText)" } ?? mark.start.clockText).typography(.secondary, digits: true).foregroundStyle(.secondary) }
            Spacer()
            if let position, position.count > 1 {
                HStack(spacing: 2) {
                    ChromeButton(systemImage: "chevron.left", help: L("上一条指令（←）")) { onStep(-1) }.disabled(position.index <= 1)
                    Text("\(position.index) / \(position.count)").typography(.secondary, digits: true).foregroundStyle(.secondary)
                    ChromeButton(systemImage: "chevron.right", help: L("下一条指令（→）")) { onStep(1) }.disabled(position.index >= position.count)
                }
            }
            let links = model.sourceLinks
            if !links.isEmpty {
                QuietMenu(title: L("打开来源")) {
                    if let command = links.resumeCommand {
                        Button(L("在终端继续这个会话"), systemImage: "terminal") { SourceJump.runInTerminal(command) }
                        Button(L("复制继续命令"), systemImage: "doc.on.doc") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(command, forType: .string) }
                    }
                    if let file = links.file {
                        Button(L("在访达中显示会话文件"), systemImage: "folder") { NSWorkspace.shared.activateFileViewerSelecting([file]) }
                    }
                    if let project = links.project {
                        Button(L("打开项目文件夹"), systemImage: "folder.badge.gearshape") { NSWorkspace.shared.open(project) }
                    }
                }
            }
            if let mark, let text = mark.preview {
                QuietButton(title: L("复制指令文字")) { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(text, forType: .string) }
            }
            ChromeButton(systemImage: "xmark", help: L("收起（Esc）"), action: onClose)
        }
    }

    @ViewBuilder private var side: some View {
        VStack(alignment: .leading, spacing: Space.m) {
            if let payload = model.detail?.payload {
                VStack(alignment: .leading, spacing: Space.xxs) {
                    SectionLabel(L("工作"))
                    if case .string(let status)? = payload["status"] {
                        row(L("状态"), ["running": L("工作中"), "done": L("已完成"), "completed": L("已完成"), "interrupted": L("已中断")][status] ?? status)
                    }
                    if case .object(let tools)? = payload["tool_calls"] {
                        row(L("工具调用"), L("%@ 次", tools.values.reduce(Int64(0)) { total, value in if case .int(let n) = value { total + n } else { total } }))
                    }
                    if case .int(let count)? = payload["followups"], count > 0 { row("追问", "\(count) 条") }
                    if case .string(let agent)? = payload["agent"] { row("来源", ["codex": "Codex", "claude_code": "Claude Code", "pi": "pi"][agent] ?? agent) }
                }
            }
            if let mark, let episode = scene.episodes.last(where: { $0.kind == .app && $0.start <= mark.start && $0.end >= mark.start }) {
                VStack(alignment: .leading, spacing: Space.xxs) {
                    SectionLabel(L("当时的屏幕"))
                    FactRow(systemImage: "display", tint: AppTint.color(for: episode.bundleID), title: episode.title, detail: episode.start.clockText) {
                        onOpen(.episode(episode.id))
                    }
                }
            }
            Spacer(minLength: 0)
        }
    }

    private func row(_ title: String, _ value: String) -> some View {
        HStack { Text(title).foregroundStyle(.secondary); Spacer(); Text(value) }.typography(.body).padding(.vertical, 3)
    }
}

/// Small caption heading inside a panel.
private struct SectionLabel: View {
    let text: String
    init(_ text: String) { self.text = text }
    var body: some View { Text(text).typography(.caption).foregroundStyle(.tertiary) }
}

/// Continues an agent session in Terminal. The command is written to a private `.command` file that Terminal
/// opens; no automation permission is needed and nothing runs until Terminal starts it.
enum SourceJump {
    static func runInTerminal(_ command: String) {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("candid-resume", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let file = dir.appendingPathComponent(UUID().uuidString + ".command")
        let script = "#!/bin/zsh -l\nrm -f \"$0\"\n" + command + "\n"
        guard (try? script.write(to: file, atomically: true, encoding: .utf8)) != nil,
              (try? FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: file.path)) != nil else { return }
        let terminal = URL(fileURLWithPath: "/System/Applications/Utilities/Terminal.app")
        NSWorkspace.shared.open([file], withApplicationAt: terminal, configuration: NSWorkspace.OpenConfiguration())
    }
}
