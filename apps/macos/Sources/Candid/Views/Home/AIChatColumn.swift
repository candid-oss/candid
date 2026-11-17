import AIProviders
import CandidLocalization
import SwiftUI

/// The AI page's chat with Pi (agent-platform-v1 §4.2, mockup ④): your messages as bubbles on the right, Pi's answers
/// unboxed on the left with a steps line (what it looked up, expandable to each call), Markdown with record chips,
/// writes inside the store as done cards with 撤销, and anything outside the App as proposal cards that wait for 批准.
struct AIChatColumn: View {
    @Bindable var model: AIPageModel
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: Space.s) {
            if !model.context.isEmpty { contextRow }
            ScrollViewReader { reader in
                ScrollView {
                    // A plain stack: a lazy one keeps the height a row had while streaming (or before Markdown was
                    // laid out), which left blank blocks hundreds of points tall under the steps line.
                    VStack(alignment: .leading, spacing: Space.m) {
                        if model.conversation.messages.isEmpty { empty }
                        ForEach(model.conversation.messages) { message in
                            AIMessageView(model: model, message: message, live: model.turnRunning && message.id == model.conversation.messages.last?.id)
                                .id(message.id)
                        }
                    }
                    .padding(.vertical, Space.xs)
                }
                .scrollIndicators(.hidden)
                .onChange(of: model.conversation.messages.last?.text) { _, _ in
                    if let last = model.conversation.messages.last { reader.scrollTo(last.id, anchor: .bottom) }
                }
                .onChange(of: model.conversation.messages.count) { _, _ in
                    if let last = model.conversation.messages.last { withAnimation(Motion.spring) { reader.scrollTo(last.id, anchor: .bottom) } }
                }
            }
            if let problem = model.agentProblem { InlineMessage(kind: .warning, text: problem) }
            composer
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .environment(\.openURL, OpenURLAction { url in
            guard url.scheme == AIMarkdown.recordScheme, let id = url.host() else { return .systemAction }
            Task { await model.openRecord(id) }
            return .handled
        })
    }

    private var contextRow: some View {
        HStack(spacing: Space.xxs) {
            Text(L("上下文")).typography(.caption).foregroundStyle(.tertiary)
            ForEach(model.context) { item in
                HStack(spacing: 3) {
                    Text(item.chip).typography(.caption, .medium).lineLimit(1)
                    Button { model.removeContext(item) } label: { Image(systemName: "xmark").typography(.caption, .bold).foregroundStyle(.secondary) }
                        .buttonStyle(.plain).accessibilityLabel(L("移除上下文 %@", item.title))
                }
                .padding(.horizontal, Space.xs).frame(height: ControlHeight.chip)
                .background(Capsule().fill(Surface.fill.color))
            }
            Spacer(minLength: 0)
        }
    }

    private var empty: some View {
        VStack(alignment: .leading, spacing: Space.xs) {
            HStack(spacing: Space.xs) {
                AIMark()
                Text(L("问点什么，或者让它整理")).typography(.section)
            }
            Text(L("它能查你的时间线、改标题和备注、标资料；写进提醒事项或日历前会先给你批准。左边任何一条点「问 AI」可以带进来，输入 @ 可以引用一条记录。"))
                .typography(.secondary).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
        .padding(.top, Space.l)
    }

    private var composer: some View {
        VStack(alignment: .leading, spacing: Space.xs) {
            if !model.mentions.isEmpty {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(model.mentions, id: \.eventID) { ref in
                        Button { model.pickMention(ref) } label: {
                            HStack(spacing: Space.xs) {
                                Image(systemName: "plus.circle.fill").foregroundStyle(Palette.accent).accessibilityHidden(true)
                                Text(ref.label).typography(.secondary, .medium).foregroundStyle(Palette.accent)
                                Text(ref.title ?? "").typography(.secondary).foregroundStyle(.secondary).lineLimit(1)
                                Spacer(minLength: 0)
                            }
                            .padding(.horizontal, Space.xs).frame(height: ControlHeight.field)
                            .background(RoundedRectangle(cornerRadius: Radius.small, style: .continuous).fill(Surface.wash.color))
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain).padding(.bottom, 2)
                    }
                }
                .transition(Motion.unfold)
            }
            if let skill = model.pendingSkill {
                HStack(spacing: Space.xxs) {
                    Tag(text: L("技能 · ") + skill.title, tint: Palette.accent)
                    Text(L("写下") + (skill.input ?? L("要做什么")) + L("，或在左栏点「问 AI」带上一条，再发送")).typography(.caption).foregroundStyle(.secondary)
                    Spacer(minLength: 0)
                    ChipButton(title: L("不用技能"), systemImage: "xmark") { model.pendingSkill = nil }
                }
            }
            TextField(L("问点什么，或者让它整理…　输入 @ 引用一条记录"), text: $model.draft, axis: .vertical)
                .textFieldStyle(.plain).typography(.body).lineLimit(2...6)
                .focused($focused)
                .onSubmit { Task { await model.send() } }
                .onChange(of: model.draft) { _, _ in model.draftChanged() }
            HStack(spacing: Space.xxs) {
                chip("Pi · " + (AIPageModel.qaAgentDestination()?.label ?? L("未选模型")))
                chip(L("范围：允许给 AI 的全部来源"))
                Spacer(minLength: Space.xs)
                if model.turnRunning {
                    RunButton(title: L("发送"), runningTitle: model.liveCall.map { PiToolResult.live($0.tool, arguments: $0.arguments) } ?? L("正在回答…"),
                              running: true, start: {}, cancel: { model.cancelRun() })
                } else {
                    PrimaryButton(title: L("发送")) { Task { await model.send() } }
                        .disabled(model.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && model.context.isEmpty)
                }
            }
        }
        .padding(Space.s)
        .floatingSurface(radius: Radius.large)
        .selectionEdge(focused, radius: Radius.large, glow: true)
    }

    private func chip(_ text: String) -> some View {
        Text(text).typography(.caption, .medium).foregroundStyle(.secondary)
            .padding(.horizontal, Space.xs).frame(height: ControlHeight.chip)
            .background(Capsule().fill(Surface.fill.color))
    }
}

/// The AI gradient mark beside Pi's answers.
struct AIMark: View {
    var body: some View {
        Image(systemName: "sparkle").typography(.secondary, .semibold).foregroundStyle(Palette.ai).accessibilityHidden(true)
    }
}

struct AIMessageView: View {
    @Bindable var model: AIPageModel
    let message: AIChatMessage
    let live: Bool
    @ViewState private var stepsOpen = false

    var body: some View {
        switch message.role {
        case .user:
            VStack(alignment: .trailing, spacing: Space.xxs) {
                if !message.context.isEmpty {
                    HStack(spacing: Space.xxs) { ForEach(message.context) { Tag(text: $0.chip) } }
                }
                Text(message.text).typography(.body).textSelection(.enabled)
                    .padding(.horizontal, Space.s).padding(.vertical, Space.xs)
                    .background(UnevenRoundedRectangle(topLeadingRadius: Radius.large, bottomLeadingRadius: Radius.large,
                                                       bottomTrailingRadius: 4, topTrailingRadius: Radius.large, style: .continuous)
                        .fill(Palette.accent.opacity(0.13)))
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
            .padding(.leading, Space.xxl)
        case .note:
            InlineMessage(kind: .warning, text: message.text)
        case .assistant:
            HStack(alignment: .top, spacing: Space.xs) {
                AIMark().padding(.top, 3)
                VStack(alignment: .leading, spacing: Space.xs) {
                    steps
                    if let call = model.liveCall, live {
                        HStack(spacing: Space.xs) {
                            ProgressView().controlSize(.mini)
                            Text(PiToolResult.live(call.tool, arguments: call.arguments)).typography(.secondary).foregroundStyle(.secondary)
                        }
                        .transition(.opacity)
                    } else if message.text.isEmpty && live {
                        HStack(spacing: Space.xs) { ProgressView().controlSize(.mini); Text(L("正在想…")).typography(.secondary).foregroundStyle(.secondary) }
                    }
                    if !message.text.isEmpty {
                        AIMarkdownView(text: message.text, label: model.recordLabel)
                    }
                    ForEach(message.actions) { action in AIActionCard(model: model, action: action) }
                    if message.stopped == true { Text(L("已停止")).typography(.caption).foregroundStyle(.tertiary) }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.trailing, Space.xl)
        }
    }

    @ViewBuilder private var steps: some View {
        if let summary = message.stepsSummary {
            VStack(alignment: .leading, spacing: Space.xxs) {
                Button { withAnimation(Motion.snappy) { stepsOpen.toggle() } } label: {
                    HStack(spacing: 3) {
                        Text(summary).typography(.caption).lineLimit(1)
                        if !(message.calls ?? []).isEmpty { Image(systemName: stepsOpen ? "chevron.up" : "chevron.down").typography(.caption) }
                    }
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, Space.xs).frame(height: ControlHeight.chip)
                    .background(Capsule().fill(Surface.fill.color)).contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(summary + (stepsOpen ? L("，收起") : L("，展开每次调用")))
                if stepsOpen {
                    VStack(alignment: .leading, spacing: Space.xxs) {
                        ForEach(message.calls ?? []) { call in
                            HStack(alignment: .firstTextBaseline, spacing: Space.xs) {
                                Image(systemName: call.state == .failed ? "xmark.circle" : call.state == .running ? "circle.dotted" : "checkmark.circle")
                                    .typography(.caption).foregroundStyle(call.state == .failed ? Palette.warning : .secondary)
                                Text(PiToolResult.step(call.tool)).typography(.caption, .semibold)
                                Text(call.arguments == "{}" ? "" : call.arguments).typography(.caption).monospaced().foregroundStyle(.secondary).lineLimit(2)
                                Spacer(minLength: 0)
                                if let outcome = call.outcome { Text(outcome).typography(.caption).foregroundStyle(.tertiary).lineLimit(1) }
                            }
                        }
                    }
                    .padding(Space.xs)
                    .background(RoundedRectangle(cornerRadius: Radius.small, style: .continuous).fill(Surface.wash.color))
                    .transition(Motion.unfold)
                }
            }
        }
    }
}

/// Pi's Markdown: headings, paragraphs, lists, quotes, code blocks, tables and rules; record citations as chips.
struct AIMarkdownView: View {
    let text: String
    let label: (String) -> String

    var body: some View {
        let blocks = AIMarkdown.blocks(AIMarkdown.citations(text, label: label))
        VStack(alignment: .leading, spacing: Space.xs) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in view(block) }
        }
        .textSelection(.enabled)
        .fixedSize(horizontal: false, vertical: true)
    }

    private func inline(_ s: String) -> Text { Text(Self.styled(s)) }

    /// Inline Markdown with chip links tinted (Text cannot hold views, so a chip is a tinted, shaded link run).
    static func styled(_ s: String) -> AttributedString {
        var a = AIMarkdown.inline(s)
        for run in a.runs where run.link?.scheme == AIMarkdown.recordScheme {
            a[run.range].foregroundColor = Palette.accent
            a[run.range].backgroundColor = Palette.accent.opacity(0.1)
        }
        return a
    }

    @ViewBuilder private func view(_ block: AIMarkdownBlock) -> some View {
        switch block {
        case .heading(let level, let t):
            inline(t).typography(level <= 2 ? .section : .headline).padding(.top, Space.xxs)
        case .paragraph(let t):
            inline(t).typography(.body)
        case .listItem(let marker, let depth, let t):
            HStack(alignment: .firstTextBaseline, spacing: Space.xs) {
                Text(marker).typography(.body, digits: true).foregroundStyle(.secondary).frame(minWidth: 14, alignment: .trailing)
                inline(t).typography(.body)
            }
            .padding(.leading, CGFloat(depth) * Space.m)
        case .quote(let t):
            HStack(spacing: Space.xs) {
                Capsule().fill(Surface.selected.color).frame(width: 3)
                inline(t).typography(.body).foregroundStyle(.secondary)
            }
        case .code(_, let t):
            ScrollView(.horizontal, showsIndicators: false) {
                Text(t).typography(.secondary).monospaced().padding(Space.s)
            }
            .background(RoundedRectangle(cornerRadius: Radius.small, style: .continuous).fill(Surface.fill.color))
        case .table(let rows):
            Grid(alignment: .leading, horizontalSpacing: Space.s, verticalSpacing: Space.xxs) {
                ForEach(Array(rows.enumerated()), id: \.offset) { r, row in
                    GridRow { ForEach(Array(row.enumerated()), id: \.offset) { _, cell in inline(cell).typography(.secondary, r == 0 ? .semibold : nil) } }
                    if r == 0 { HairlineDivider() }
                }
            }
        case .rule:
            HairlineDivider()
        }
    }
}

/// A write (done, 撤销) or a proposal (拒绝 / 批准) under Pi's answer, as in mockup ④.
struct AIActionCard: View {
    @Bindable var model: AIPageModel
    let action: AIChatAction

    var body: some View {
        Group {
            if action.kind == .proposal, let proposal = model.proposal(action.id) {
                ProposalRow(model: model, proposal: proposal, compact: true)
            } else if action.kind == .proposal {
                row(symbol: "exclamationmark", tint: Palette.warning, text: action.summary, trailing: Tag(text: label(action.status)))
            } else {
                let key = "write:" + action.id
                VStack(alignment: .leading, spacing: Space.xxs) {
                    row(symbol: action.status == "undone" ? "arrow.uturn.backward" : "checkmark", tint: action.status == "undone" ? .secondary : Palette.success,
                        text: action.summary,
                        trailing: HStack(spacing: Space.xxs) {
                            Text(action.status == "undone" ? L("已撤销") : L("本库 · 已完成")).typography(.caption).foregroundStyle(.tertiary)
                            if action.status == "done" { ChipButton(title: L("撤销"), systemImage: "arrow.uturn.backward") { Task { await model.undo(action) } }.disabled(model.working.contains(key)) }
                        })
                    if let problem = model.problems[key] { InlineMessage(kind: .error, text: problem) }
                }
            }
        }
        .padding(.horizontal, Space.s).padding(.vertical, Space.xs)
        .frame(maxWidth: .infinity, alignment: .leading)
        .panelSurface(radius: Radius.medium)
        .overlay {
            if action.kind == .proposal && ["pending", "failed"].contains(model.proposal(action.id)?.status ?? action.status) {
                RoundedRectangle(cornerRadius: Radius.medium, style: .continuous).strokeBorder(Palette.warning.opacity(0.4), lineWidth: 1)
            }
        }
    }

    private func label(_ status: String) -> String {
        ["pending": L("等你批准"), "approved": L("正在执行"), "executed": L("已完成"), "failed": L("失败"), "rejected": L("已拒绝")][status] ?? status
    }

    private func row(symbol: String, tint: Color, text: String, trailing: some View) -> some View {
        HStack(spacing: Space.xs) {
            AIActionIcon(symbol: symbol, tint: tint)
            Text(text).typography(.body).lineLimit(2)
            Spacer(minLength: Space.xs)
            trailing
        }
    }
}

/// The small tinted square in front of an action card.
struct AIActionIcon: View {
    let symbol: String
    let tint: Color
    var body: some View {
        Image(systemName: symbol).typography(.caption, .bold).foregroundStyle(tint)
            .frame(width: 22, height: 22)
            .background(RoundedRectangle(cornerRadius: Radius.small, style: .continuous).fill(tint.opacity(0.15)))
            .accessibilityHidden(true)
    }
}
