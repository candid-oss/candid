import AppKit
import CandidLocalization
import SwiftUI

// What the floating ball and its panel draw. Window handling and mouse input live in FloatingControl.swift.

extension FloatingBallState {
    var color: Color {
        switch self {
        case .recording: Palette.success
        case .recordingAudio: Palette.recording
        case .paused: Color.secondary
        case .problem: Palette.warning
        }
    }
    var title: String {
        switch self {
        case .recording: L("正在记录")
        case .recordingAudio: L("正在录音")
        case .paused: L("已暂停")
        case .problem: L("需要处理")
        }
    }
}

// MARK: Ball

struct FloatingBallView: View {
    let assistant: FloatingAssistant
    @ViewState private var pulse = false

    var body: some View {
        let state = assistant.state
        let voice = assistant.voice.phase
        ZStack(alignment: .topTrailing) {
            ZStack {
                Circle().fill(.regularMaterial)
                Circle().strokeBorder(state.color.opacity(0.55), lineWidth: 1.5)
                switch voice {
                case .listening:
                    VoiceBars(levels: Array(assistant.voice.levels.suffix(5)), height: 16)
                        .frame(width: 20)
                case .transcribing:
                    ProgressView().controlSize(.small)
                default:
                    Circle().fill(state.color)
                        .frame(width: 12, height: 12)
                        .shadow(color: state.color.opacity(0.6), radius: pulse ? 6 : 2)
                }
            }
            .frame(width: FloatingControl.ballSize, height: FloatingControl.ballSize)
            .shadow(color: .black.opacity(0.22), radius: 5, y: 2)
            .scaleEffect(voice == .listening ? 1.12 : 1)
            .padding(FloatingControl.margin)
            if assistant.pendingCount > 0 {
                Text(assistant.pendingCount > 99 ? "99+" : String(assistant.pendingCount))
                    .typography(.caption, .bold, digits: true).foregroundStyle(.white)
                    .padding(.horizontal, 5).frame(minWidth: 18, minHeight: 18)
                    .background(Capsule().fill(Palette.danger))
                    .padding(.top, FloatingControl.margin / 2).padding(.trailing, FloatingControl.margin / 2)
            }
        }
        .frame(width: FloatingControl.windowSize, height: FloatingControl.windowSize)
        .animation(Motion.bouncy, value: voice)
        .animation(Motion.snappy, value: assistant.pendingCount)
        .onAppear { withAnimation(.easeInOut(duration: 1.4).repeatForever()) { pulse = true } }
        .help(assistant.state.problem ?? (assistant.voiceEnabled ? L("Candid：点按打开，按住说话") : L("Candid：点按打开")))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(L("Candid 悬浮球，%@", assistant.state.title) + (assistant.pendingCount > 0 ? L("，%@ 条待确认", String(assistant.pendingCount)) : ""))
    }
}

/// The level while speaking: bars, newest on the right.
struct VoiceBars: View {
    let levels: [Float]
    var height: CGFloat = 22
    var body: some View {
        HStack(alignment: .center, spacing: 2) {
            ForEach(Array(levels.enumerated()), id: \.offset) { _, level in
                Capsule().fill(Palette.accent)
                    .frame(width: 3, height: max(3, CGFloat(level) * height))
            }
        }
        .frame(height: height)
        .animation(.linear(duration: 0.08), value: levels)
        .accessibilityHidden(true)
    }
}

// MARK: Panel

struct FloatingPanelView: View {
    @Bindable var assistant: FloatingAssistant
    let resized: (CGFloat) -> Void
    @Environment(\.visualStyle) private var style
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: Space.s) {
            header
            if assistant.showingSettings {
                FloatingSettingsView(voice: assistant.voice) { assistant.showingSettings = false }
            } else {
                ForEach(assistant.problems.prefix(2), id: \.self) { InlineMessage(kind: .warning, text: $0) }
                input
                if let exchange = assistant.exchange { answer(exchange.question, exchange.answer) }
                if assistant.pendingCount > 0 { pending }
                HairlineDivider()
                footer
            }
        }
        .padding(Space.m)
        .frame(width: FloatingControl.panelWidth, alignment: .topLeading)
        .fixedSize(horizontal: false, vertical: true)
        .background {
            let shape = RoundedRectangle(cornerRadius: Radius.large, style: .continuous)
            ZStack {
                if style == .flat { shape.fill(Palette.raised) } else { shape.fill(.regularMaterial) }
                shape.strokeBorder(Surface.hairline, lineWidth: Surface.hairlineWidth)
            }
        }
        .background(GeometryReader { geometry in Color.clear.preference(key: FloatingPanelHeight.self, value: geometry.size.height) })
        .onPreferenceChange(FloatingPanelHeight.self) { resized($0) }
        .frame(maxHeight: .infinity, alignment: .top)
        .animation(Motion.snappy, value: assistant.showingSettings)
        .onChange(of: assistant.focusRequest) { _, _ in focused = true }
        .onAppear { focused = true }
    }

    // MARK: Header

    private var header: some View {
        let state = assistant.state
        return HStack(spacing: Space.xs) {
            Circle().fill(state.color).frame(width: 8, height: 8)
            Text(state == .recordingAudio ? state.title : assistant.session.isRecording ? L("正在记录") : L("已暂停"))
                .typography(.secondary, .medium)
            QuietButton(title: assistant.session.isRecording ? L("暂停记录") : L("开始记录"), emphasized: !assistant.session.isRecording) {
                assistant.toggleRecording()
            }
            .disabled(assistant.session.isDemo)
            Spacer(minLength: 0)
            ChromeButton(systemImage: "gearshape", help: L("悬浮球设置"), active: assistant.showingSettings) { assistant.showingSettings.toggle() }
            ChromeButton(systemImage: "xmark", help: L("收起")) { FloatingControl.shared.closePanel() }
        }
    }

    // MARK: Input

    @ViewBuilder private var input: some View {
        let voice = assistant.voice
        HStack(alignment: .center, spacing: Space.xs) {
            switch voice.phase {
            case .listening:
                VoiceBars(levels: voice.levels)
                Text(voice.partial.isEmpty ? L("正在听…松开结束") : voice.partial)
                    .typography(.body).foregroundStyle(voice.partial.isEmpty ? .secondary : .primary)
                    .lineLimit(3).frame(maxWidth: .infinity, alignment: .leading)
            case .transcribing:
                ProgressView().controlSize(.small)
                Text(L("正在识别…")).typography(.body).foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: .leading)
            default:
                TextField(L("问 Candid…"), text: $assistant.draft, axis: .vertical)
                    .textFieldStyle(.plain).typography(.body).lineLimit(1...6)
                    .focused($focused)
                    .onSubmit { Task { await assistant.ask() } }
                    .onChange(of: assistant.draft) { _, value in if value.isEmpty { assistant.draftFromVoice = false } }
            }
            if assistant.voiceEnabled {
                ChromeButton(systemImage: voice.phase == .listening ? "stop.circle.fill" : "mic", help: voice.phase == .listening ? L("结束说话") : L("点一下开始说话，再点一下结束"),
                             active: voice.phase == .listening) {
                    if voice.phase == .listening { assistant.endVoice() } else { assistant.beginVoice() }
                }
                .disabled(voice.phase == .transcribing)
            }
            if !assistant.draft.isEmpty, !voice.isActive {
                ChromeButton(systemImage: "arrow.up.circle.fill", help: L("发送（回车）")) { Task { await assistant.ask() } }
                    .disabled(assistant.ai.turnRunning)
            }
        }
        .padding(.horizontal, Space.s).padding(.vertical, Space.xxs)
        .frame(minHeight: ControlHeight.field + Space.xs)
        .background(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous).fill(Surface.fill.color))
        if case .failed(let why) = voice.phase {
            HStack { InlineMessage(kind: .error, text: why); Spacer(); QuietButton(title: L("知道了")) { voice.dismissError() } }
        }
        if assistant.draftFromVoice, !assistant.draft.isEmpty, !voice.isActive {
            HStack(spacing: Space.xxs) {
                ChipButton(title: L("发送"), systemImage: "arrow.up", emphasized: true) { Task { await assistant.ask() } }
                    .disabled(assistant.ai.turnRunning)
                ChipButton(title: L("记一笔"), systemImage: "square.and.pencil") { Task { await assistant.saveNote() } }
                ChipButton(title: L("帮我写"), systemImage: "pencil.line") { assistant.compose() }
                Spacer(minLength: 0)
                if let engine = voice.lastEngine { Text(engine.title).typography(.caption).foregroundStyle(.tertiary) }
            }
        }
    }

    // MARK: Answer

    private func answer(_ question: AIChatMessage, _ reply: AIChatMessage?) -> some View {
        let ai = assistant.ai
        let running = ai.turnRunning && reply == ai.conversation.messages.last
        return VStack(alignment: .leading, spacing: Space.xs) {
            Text(question.text).typography(.secondary, .medium).foregroundStyle(.secondary).lineLimit(2)
            if let reply, !reply.text.isEmpty {
                Text(AIMarkdown.inline(AIMarkdown.citations(reply.text, label: ai.recordLabel)))
                    .typography(.body).foregroundStyle(reply.role == .note ? Palette.danger : .primary)
                    .lineLimit(12).textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            } else if ai.turnRunning {
                HStack(spacing: Space.xs) {
                    ProgressView().controlSize(.small)
                    Text(ai.liveCall.map { PiToolResult.live($0.tool, arguments: $0.arguments) } ?? L("正在想…"))
                        .typography(.secondary).foregroundStyle(.secondary)
                }
            }
            HStack(spacing: Space.xxs) {
                if running || (reply == nil && ai.turnRunning) { QuietButton(title: L("停止")) { assistant.stopAnswer() } }
                Spacer(minLength: 0)
                QuietButton(title: L("在 AI 页继续"), emphasized: true) { assistant.continueInAI() }
            }
        }
        .padding(Space.s)
        .background(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous).fill(Surface.wash.color))
    }

    // MARK: Pending

    private var pending: some View {
        let ai = assistant.ai
        let items = Array(ai.items.prefix(3))
        let proposals = Array(assistant.proposals.prefix(max(0, 3 - items.count)))
        return VStack(alignment: .leading, spacing: Space.xs) {
            HStack {
                Text(L("待确认 %@", String(assistant.pendingCount))).typography(.secondary, .semibold)
                Spacer()
                if assistant.pendingCount > items.count + proposals.count {
                    QuietButton(title: L("全部")) { assistant.openToday() }
                }
            }
            ForEach(items) { item in
                PendingRow(symbol: item.isEvent ? "calendar" : "checklist", title: item.title, detail: item.origin,
                           confirm: item.isEvent ? L("加到日历") : L("加到提醒"), working: ai.working.contains("item:" + item.id),
                           problem: ai.problems["item:" + item.id],
                           onConfirm: { Task { await ai.add(item) } }, onIgnore: { Task { await ai.ignore(item) } })
            }
            ForEach(proposals) { proposal in
                PendingRow(symbol: proposal.kind.contains("calendar") ? "calendar" : "checklist", title: proposal.title, detail: proposal.what,
                           confirm: L("批准"), working: ai.working.contains("proposal:" + proposal.id),
                           problem: ai.problems["proposal:" + proposal.id],
                           onConfirm: { Task { await ai.approve(proposal) } }, onIgnore: { Task { await ai.reject(proposal) } })
            }
        }
    }

    // MARK: Footer

    private var footer: some View {
        HStack(spacing: Space.xxs) {
            ChromeButton(systemImage: "square.and.pencil", help: L("快速记一笔")) { assistant.quick(.note) }
            ChromeButton(systemImage: "magnifyingglass", help: L("搜索")) { assistant.quick(.search) }
            ChromeButton(systemImage: "text.cursor", help: L("帮我写")) { assistant.compose() }
            Spacer(minLength: 0)
            if let notice = assistant.notice {
                Text(notice).typography(.secondary).foregroundStyle(.secondary).transition(.opacity)
            } else if assistant.voiceEnabled {
                Text(L("按住球或快捷键说话")).typography(.caption).foregroundStyle(.tertiary)
            }
        }
        .animation(Motion.snappy, value: assistant.notice)
    }
}

private struct PendingRow: View {
    let symbol: String
    let title: String
    let detail: String
    let confirm: String
    let working: Bool
    let problem: String?
    let onConfirm: () -> Void
    let onIgnore: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: Space.xxs) {
            HStack(alignment: .firstTextBaseline, spacing: Space.xs) {
                Image(systemName: symbol).typography(.secondary).foregroundStyle(.secondary)
                VStack(alignment: .leading, spacing: 1) {
                    Text(title).typography(.body).lineLimit(2)
                    if !detail.isEmpty { Text(detail).typography(.caption).foregroundStyle(.secondary).lineLimit(1) }
                }
                Spacer(minLength: Space.xxs)
                if working {
                    ProgressView().controlSize(.small)
                } else {
                    QuietButton(title: L("忽略"), action: onIgnore)
                    QuietButton(title: confirm, emphasized: true, action: onConfirm)
                }
            }
            if let problem { InlineMessage(kind: .error, text: problem) }
        }
    }
}

private struct FloatingPanelHeight: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = max(value, nextValue()) }
}
