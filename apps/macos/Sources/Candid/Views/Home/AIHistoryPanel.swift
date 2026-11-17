import SwiftUI
import CandidLocalization

/// 「历史对话」 (agent-platform-v1 §4.2): conversations kept on this Mac, grouped 今天 / 昨天 / 更早, searchable. Each row
/// shows the title (the first question until renamed), when, the last answer in one line and how many records it
/// cited. Rename and delete from the context menu or the row's ⋯; opening one restores it with its cards.
struct AIHistoryPanel: View {
    @Bindable var model: AIPageModel
    let close: () -> Void
    @ViewState private var query = ""

    var body: some View {
        let groups = AIHistoryGroups.make(model.history, now: model.currentTime, query: query)
        InlinePanel {
            VStack(alignment: .leading, spacing: Space.s) {
                HStack(spacing: Space.xs) {
                    Text(L("历史对话")).typography(.headline)
                    Text(L("只存在这台 Mac 上")).typography(.caption).foregroundStyle(.tertiary)
                    Spacer()
                    ChipButton(title: L("收起"), systemImage: "chevron.up", action: close)
                }
                SearchField(prompt: L("搜索标题和内容"), text: $query)
                if groups.groups.isEmpty {
                    Text(model.history.isEmpty ? L("还没有保存的对话。问一个问题后会出现在这里。") : L("没有找到「%@」", query))
                        .typography(.secondary).foregroundStyle(.secondary).padding(.vertical, Space.xs)
                }
                VStack(alignment: .leading, spacing: Space.s) {
                    ForEach(groups.groups) { group in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(group.title + " · \(group.items.count)").typography(.caption, .semibold).foregroundStyle(.secondary)
                                .padding(.leading, Space.xs).padding(.bottom, 2)
                            ForEach(group.items.prefix(query.isEmpty ? 6 : 30)) { item in
                                AIHistoryRow(model: model, conversation: item, current: item.id == model.conversation.id, close: close)
                            }
                            if query.isEmpty && group.items.count > 6 {
                                Text(L("还有 %@ 条，用上面的搜索找", group.items.count - 6)).typography(.caption).foregroundStyle(.tertiary).padding(.leading, Space.xs)
                            }
                        }
                    }
                }
            }
        }
    }
}

struct AIHistoryRow: View {
    @Bindable var model: AIPageModel
    let conversation: AIConversation
    let current: Bool
    let close: () -> Void
    @ViewState private var hovering = false
    @ViewState private var renaming = false
    @ViewState private var name = ""
    @ViewState private var confirmingDelete = false
    @FocusState private var nameFocused: Bool

    static let time: DateFormatter = { let f = DateFormatter(); f.dateFormat = "HH:mm"; return f }()
    static let day: DateFormatter = { let f = DateFormatter(); f.locale = Locale(identifier: "zh_CN"); f.dateFormat = L("M月d日 HH:mm"); return f }()

    private var when: String {
        Calendar.current.isDate(conversation.updated, inSameDayAs: model.currentTime) || Calendar.current.isDate(conversation.updated, inSameDayAs: model.currentTime.addingTimeInterval(-86_400))
            ? Self.time.string(from: conversation.updated) : Self.day.string(from: conversation.updated)
    }

    var body: some View {
        HStack(alignment: .top, spacing: Space.s) {
            AIMark().padding(.top, 3)
            VStack(alignment: .leading, spacing: 3) {
                if renaming {
                    HStack(spacing: Space.xxs) {
                        TextField(L("对话名称"), text: $name).textFieldStyle(.plain).typography(.body, .semibold)
                            .focused($nameFocused)
                            .onSubmit(save)
                            .padding(.horizontal, Space.xs).frame(height: ControlHeight.button)
                            .background(RoundedRectangle(cornerRadius: Radius.small, style: .continuous).fill(Surface.fill.color))
                        ChipButton(title: L("取消")) { renaming = false }
                        ChipButton(title: L("保存"), emphasized: true, action: save)
                    }
                } else {
                    HStack(spacing: Space.xs) {
                        Text(conversation.title).typography(.body, .semibold).lineLimit(1)
                        if current { Tag(text: L("当前"), tint: Palette.accent) }
                        Spacer(minLength: Space.xs)
                        Text(when).typography(.caption, digits: true).foregroundStyle(.tertiary)
                    }
                }
                Text(conversation.preview.isEmpty ? L("（没有回答）") : conversation.preview).typography(.secondary).foregroundStyle(.secondary).lineLimit(1)
                HStack(spacing: Space.xxs) {
                    let questions = conversation.messages.filter { $0.role == .user }.count
                    Tag(text: L("%@ 问", questions))
                    if conversation.recordCount > 0 { Tag(text: L("引用 %@ 条记录", conversation.recordCount)) }
                    let cards = conversation.messages.flatMap(\.actions).count
                    if cards > 0 { Tag(text: L("%@ 项写入或提案", cards)) }
                    Spacer(minLength: 0)
                    if confirmingDelete {
                        Text(L("删除后不能恢复")).typography(.caption).foregroundStyle(.secondary)
                        ChipButton(title: L("不删")) { confirmingDelete = false }
                        ChipButton(title: L("确认删除"), systemImage: "trash", role: .destructive) {
                            confirmingDelete = false
                            Task { await model.deleteConversation(conversation.id) }
                        }
                    } else if hovering && !renaming {
                        ChipButton(title: L("打开"), systemImage: "arrow.right", emphasized: true, action: open)
                        ChromeMenu(systemImage: "ellipsis", help: L("更多")) { menu }
                    }
                }
            }
        }
        .padding(Space.xs)
        .background(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous)
            .fill(current ? Surface.selected.color : hovering ? Surface.hover.color : .clear))
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .contextMenu { menu }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(L("%@，%@", conversation.title, when))
        .accessibilityAction(named: L("打开"), open)
    }

    @ViewBuilder private var menu: some View {
        Button(L("打开"), action: open)
        Button(L("重命名…")) { name = conversation.title; renaming = true; nameFocused = true }
        Divider() // style-exempt: separator inside a system context menu
        Button(L("删除…"), role: .destructive) { confirmingDelete = true }
    }

    private func open() {
        Task { await model.open(conversation); model.show(.chat); close() }
    }

    private func save() {
        model.rename(conversation.id, to: name)
        renaming = false
    }
}
