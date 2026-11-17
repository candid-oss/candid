import SwiftUI
import CandidLocalization

// The mail pane's scope, rules and stored-mail sections (agent-platform-v1 §2.0.2). Composed from the shared controls only.

/// Which accounts, folders and how far back to import, with a count preview taken from mail headers only.
struct MailScopePanel: View {
    let model: ConnectorsModel

    var body: some View {
        if model.appleMail?.connected == true, let options = model.scopeOptions {
            SettingsGroup(title: L("导入范围")) {
                if options.accounts.count > 1 {
                    FoldRow(title: L("账户"), detail: accountSummary(options)) {
                        note(L("全部打开等于导入所有账户"))
                        ForEach(accountChoices(options)) { row($0) }
                    }
                    HairlineDivider()
                }
                FoldRow(title: L("文件夹"), detail: folderSummary(options)) {
                    note(L("垃圾邮件、废纸篓、草稿永远不读；同名文件夹跨账户合并计数"))
                    ForEach(folderChoices(options)) { row($0) }
                }
                HairlineDivider()
                SettingRow(title: L("往前导入"), detail: nil) {
                    PillSwitcher(items: MailScope.backfillChoices, selection: Binding(get: { model.scopeDraft.backfillDays }, set: { model.scopeDraft.backfillDays = $0 }), scale: .secondary)
                }
                HairlineDivider()
                previewBlock
            }
            .task(id: model.scopeDraft) {
                guard model.scopeLoaded else { return }
                try? await Task.sleep(for: .milliseconds(300))
                if !Task.isCancelled { await model.previewScope() }
            }
        }
    }

    private struct Choice: Identifiable { var id: String; var title: String; var detail: String; var on: Bool; var set: (Bool) -> Void }

    private func note(_ text: String) -> some View {
        Text(text).typography(.caption).foregroundStyle(.secondary).padding(.bottom, Space.xxs)
    }

    private func accountSummary(_ options: MailScopeOptions) -> String {
        let chosen = model.scopeDraft.accounts
        return chosen.isEmpty ? L("全部 %@ 个", options.accounts.count) : L("已选 %@ / %@", chosen.count, options.accounts.count)
    }

    private func folderSummary(_ options: MailScopeOptions) -> String {
        let on = options.folders.filter { model.scopeDraft.folders.contains($0.pattern) }
        let names = on.prefix(3).map(\.title).joined(separator: L("、"))
        return on.isEmpty ? L("未选") : names + (on.count > 3 ? L(" 等 %@ 个", on.count) : "") + L(" · %@ 封", on.reduce(0) { $0 + $1.count })
    }

    /// One account or folder per line: full name, what it is and how many messages, and a switch.
    private func row(_ c: Choice) -> some View {
        HStack(spacing: Space.s) {
            VStack(alignment: .leading, spacing: 1) {
                Text(verbatim: c.title).typography(.body).lineLimit(1).truncationMode(.middle).help(c.title)
                Text(c.detail).typography(.caption, digits: true).foregroundStyle(.secondary)
            }
            Spacer(minLength: Space.s)
            Toggle(c.title, isOn: Binding(get: { c.on }, set: c.set)).toggleStyle(.switch).labelsHidden().controlSize(.small)
        }
        .padding(.vertical, 5)
    }

    private func accountChoices(_ options: MailScopeOptions) -> [Choice] {
        options.accounts.map { account in
            let all = model.scopeDraft.accounts.isEmpty
            return Choice(id: account.id, title: account.title, detail: L("%@ · %@ 封", account.kind, account.count), on: all || model.scopeDraft.accounts.contains(account.id)) { on in
                var chosen = all ? Set(options.accounts.map(\.id)) : model.scopeDraft.accounts
                if on { chosen.insert(account.id) } else if chosen.count > 1 { chosen.remove(account.id) }
                model.scopeDraft.accounts = chosen.count == options.accounts.count ? [] : chosen
            }
        }
    }

    private func folderChoices(_ options: MailScopeOptions) -> [Choice] {
        options.folders.map { folder in
            Choice(id: folder.pattern, title: folder.title, detail: L("%@ 封", folder.count), on: model.scopeDraft.folders.contains(folder.pattern)) { on in
                if on { if !model.scopeDraft.folders.contains(folder.pattern) { model.scopeDraft.folders.append(folder.pattern) } }
                else if model.scopeDraft.folders.count > 1 { model.scopeDraft.folders.removeAll { $0 == folder.pattern } }
            }
        }
    }

    @ViewBuilder private var previewBlock: some View {
        VStack(alignment: .leading, spacing: Space.s) {
            Text(L("预览（只看邮件头，不读正文）")).typography(.secondary, .medium).foregroundStyle(.secondary)
            if let p = model.preview {
                HStack(spacing: Space.s) {
                    PreviewCount(value: "\(p.more ? L("至少 ") : L("约 "))\(p.total)", label: L("封"))
                    PreviewCount(value: "\(p.metadata)", label: L("只存元数据"))
                    PreviewCount(value: "\(p.normal)", label: L("普通"))
                    PreviewCount(value: "\(p.important)", label: L("重点候选"))
                }
                if p.ignored > 0 { Text(L("另有 %@ 封被规则忽略，不入库。", p.ignored)).typography(.secondary).foregroundStyle(.secondary) }
                if p.stored > 0 { Text(p.stored >= p.total ? L("这个范围的邮件都已入库。") : L("其中 %@ 封已入库，导入只补其余的。", p.stored)).typography(.secondary).foregroundStyle(.secondary) }
            } else {
                Text(L("统计中…")).typography(.secondary).foregroundStyle(.secondary)
            }
            HStack {
                Spacer()
                if model.needsConfirmation {
                    PrimaryButton(title: model.busy == "scope" ? L("开始中…") : L("导入")) { Task { await model.saveScope(confirm: true) } }
                        .disabled(model.busy != nil || model.preview == nil)
                } else {
                    PrimaryButton(title: L("保存范围")) { Task { await model.saveScope(confirm: false) } }
                        .disabled(model.busy != nil || !model.scopeChanged)
                }
            }
        }
        .padding(.vertical, Space.row)
    }
}

private struct PreviewCount: View {
    let value: String
    let label: String
    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(value).typography(.section, digits: true)
            Text(label).typography(.secondary).foregroundStyle(.secondary)
        }
        .padding(.horizontal, Space.m).padding(.vertical, Space.s)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous).fill(Surface.wash.color))
    }
}

/// How mail is stored when no rule decides, and whether AI may read it. Both are the user's choice.
struct MailStoragePanel: View {
    let model: ConnectorsModel

    var body: some View {
        if !model.mailSources.isEmpty {
            SettingsGroup(title: L("入库方式")) {
                SettingRow(title: L("没有规则命中的邮件"), detail: model.defaultLevel == "metadata" ? L("只存发件人、主题和时间，正文打开时再读") : L("存正文，可以搜索")) {
                    CapsuleChoice(options: [("metadata", L("只存元数据")), ("normal", L("存正文"))],
                                  selection: Binding(get: { model.defaultLevel }, set: { level in Task { await model.setDefaultLevel(level) } }))
                }
                HairlineDivider()
                SettingRow(title: L("允许 AI 读取邮件"), detail: L("AI 对话和外部 agent 能查到这些邮件；关闭时只有你在 App 里能看到")) {
                    Toggle(L("允许 AI 读取邮件"), isOn: Binding(get: { model.aiCanReadMail }, set: { on in Task { await model.setAICanReadMail(on) } }))
                        .toggleStyle(.switch).labelsHidden().controlSize(.small)
                }
                .disabled(model.busy != nil)
            }
        }
    }
}

/// Ordered level rules: first match wins, drag to reorder, edit in place; the built-in rules and the fixed default follow.
struct MailRulesPanel: View {
    let model: ConnectorsModel
    @ViewState private var editing: MailRule?
    @ViewState private var isNew = false

    private let levels = MailLevel.forRules.map { ($0, $0.title) }

    var body: some View {
        SettingsGroup(title: L("规则")) {
            Text(L("按顺序，第一条命中的生效；拖动调整顺序。你的规则永远优先于内置规则。")).typography(.secondary).foregroundStyle(.secondary).padding(.top, Space.xxs)
            ForEach(Array(model.mailRules.enumerated()), id: \.element.id) { index, rule in
                HairlineDivider()
                row(index, rule)
                if editing?.id == rule.id, !isNew { editor }
            }
            if isNew, editing != nil { HairlineDivider(); editor }
            HairlineDivider()
            builtinRow(title: L("你回复过的线程、写过信的人发来的"), detail: L("普通，并列为重点候选"), on: model.builtinReplied) { on in Task { await model.setBuiltinReplied(on) } }
            HairlineDivider()
            builtinRow(title: L("群发、退订、自动回复、noreply"), detail: L("只存元数据"), on: model.builtinRules) { on in Task { await model.setBuiltinRules(on) } }
            HairlineDivider()
            SettingRow(title: L("其余所有邮件"), detail: L("只存元数据；正文打开时从「邮件」App 现读，不存进时间线")) { Tag(text: L("默认")) }
            HairlineDivider()
            SettingRow(title: L("由 AI 判断是否重要"), detail: L("只在「普通」和「重点」之间调整，不能推翻你的规则；调用模型前仍按模型授权确认")) {
                Toggle("", isOn: Binding(get: { model.aiJudge }, set: { on in Task { await model.setAIJudge(on) } })).toggleStyle(.switch).labelsHidden().controlSize(.small)
            }
            if model.relevelPending { relevelPrompt }
        } accessory: {
            HStack(spacing: Space.xxs) {
                QuietButton(title: L("重新定级")) { Task { await model.relevel() } }.disabled(model.busy != nil || model.mailSources.allSatisfy { $0.events == 0 })
                QuietButton(title: L("添加规则"), emphasized: true) {
                    withAnimation(Motion.spring) { editing = MailRule(conditions: [MailCondition(field: .from, value: "")], level: .important); isNew = true }
                }
                .disabled(isNew)
            }
        }
        .disabled(model.mailSources.isEmpty)
        .opacity(model.mailSources.isEmpty ? 0.5 : 1)
    }

    private func row(_ index: Int, _ rule: MailRule) -> some View {
        HStack(spacing: Space.s) {
            Image(systemName: "line.3.horizontal").typography(.secondary).foregroundStyle(.tertiary)
            Text("\(index + 1)").typography(.secondary, digits: true).foregroundStyle(.secondary).frame(width: 18, alignment: .leading)
            Text(rule.summary).typography(.body).lineLimit(2).frame(maxWidth: .infinity, alignment: .leading)
            MenuChoice(options: levels, selection: Binding(get: { rule.level }, set: { level in var next = rule; next.level = level; Task { await model.saveRule(next) } }))
            QuietButton(title: editing?.id == rule.id ? L("收起") : L("编辑")) {
                withAnimation(Motion.spring) { if editing?.id == rule.id { editing = nil } else { editing = rule; isNew = false } }
            }
            MenuButton(title: L("移动")) {
                Button(L("上移")) { Task { await model.moveRule(from: index, to: index - 1) } }.disabled(index == 0)
                Button(L("下移")) { Task { await model.moveRule(from: index, to: index + 1) } }.disabled(index == model.mailRules.count - 1)
            }
        }
        .padding(.vertical, Space.s)
        .contentShape(Rectangle())
        .draggable(rule.id.uuidString)
        .dropDestination(for: String.self) { items, _ in
            guard let id = items.first, let from = model.mailRules.firstIndex(where: { $0.id.uuidString == id }) else { return false }
            Task { await model.moveRule(from: from, to: index) }
            return true
        }
    }

    @ViewBuilder private var editor: some View {
        if let rule = editing {
            InlinePanel {
                VStack(alignment: .leading, spacing: Space.s) {
                    ForEach(Array(rule.conditions.enumerated()), id: \.element.id) { i, condition in
                        HStack(alignment: .bottom, spacing: Space.s) {
                            if i > 0 { Text(L("且")).typography(.secondary).foregroundStyle(.secondary).padding(.bottom, Space.xs) }
                            MenuChoice(options: MailField.allCases.map { ($0, $0.title) }, selection: Binding(get: { condition.field }, set: { editing?.conditions[i].field = $0 }))
                            if condition.field == .list {
                                MenuChoice(options: [("yes", L("带群发邮件头")), ("no", L("没有群发邮件头"))], selection: Binding(get: { condition.value == "no" ? "no" : "yes" }, set: { editing?.conditions[i].value = $0 }))
                                Spacer()
                            } else {
                                QuietField(title: "", text: Binding(get: { condition.value }, set: { editing?.conditions[i].value = $0 }), placeholder: condition.field.placeholder)
                            }
                            if rule.conditions.count > 1 { QuietButton(title: L("移除")) { editing?.conditions.remove(at: i) } }
                        }
                    }
                    HStack(spacing: Space.s) {
                        QuietButton(title: L("＋ 条件"), emphasized: true) { editing?.conditions.append(MailCondition(field: .subject_contains, value: "")) }
                        Spacer()
                        Text(L("定为")).typography(.secondary).foregroundStyle(.secondary)
                        MenuChoice(options: levels, selection: Binding(get: { rule.level }, set: { editing?.level = $0 }))
                    }
                    HStack {
                        if !isNew { QuietButton(title: L("删除规则"), role: .destructive) { let r = rule; closeEditor(); Task { await model.removeRule(r) } } }
                        Spacer()
                        QuietButton(title: L("取消")) { closeEditor() }
                        PrimaryButton(title: L("保存"), isDefault: true) { let r = rule; closeEditor(); Task { await model.saveRule(r) } }
                            .disabled(!rule.isValid)
                    }
                }
            }
            .padding(.bottom, Space.s)
        }
    }

    private func closeEditor() { withAnimation(Motion.spring) { editing = nil; isNew = false } }

    private func builtinRow(title: String, detail: String, on: Bool, _ set: @escaping (Bool) -> Void) -> some View {
        SettingRow(title: title, detail: detail) {
            HStack(spacing: Space.s) {
                Tag(text: L("内置"))
                Toggle("", isOn: Binding(get: { on }, set: set)).toggleStyle(.switch).labelsHidden().controlSize(.small)
            }
        }
    }

    private var relevelPrompt: some View {
        InlinePanel {
            HStack(spacing: Space.s) {
                Text(L("规则改了，已导入的 %@ 封要按新规则重新定级吗？手动改过级别的不会变。", model.mailSources.reduce(0) { $0 + $1.events })).typography(.secondary).fixedSize(horizontal: false, vertical: true)
                Spacer()
                QuietButton(title: L("暂不")) { model.relevelPending = false }
                PrimaryButton(title: model.busy == "relevel" ? L("定级中…") : L("重新定级")) { Task { await model.relevel() } }.disabled(model.busy != nil)
            }
        }
        .padding(.vertical, Space.s)
    }
}

/// The newest stored mail. Opening one reads its body from the Mail app now; metadata-only mail keeps nothing of it.
struct MailRecentPanel: View {
    let model: ConnectorsModel

    var body: some View {
        if !model.recentMail.isEmpty {
            SettingsGroup(title: L("最近邮件")) {
                ForEach(Array(model.recentMail.enumerated()), id: \.element.id) { index, message in
                    if index > 0 { HairlineDivider() }
                    Button { Task { if model.openedMail?.message.id == message.id { model.closeMail() } else { await model.open(message) } } } label: {
                        HStack(spacing: Space.s) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(message.title ?? L("（无主题）")).typography(.body, .medium).lineLimit(1)
                                Text([message.from_name ?? (message.direction == "out" ? L("我") : nil), message.start_ms.map { Date(timeIntervalSince1970: Double($0) / 1000).formatted(.dateTime.month().day().hour().minute()) }].compactMap { $0 }.joined(separator: " · "))
                                    .typography(.secondary).foregroundStyle(.secondary)
                            }
                            Spacer()
                            if message.important_candidate && message.mailLevel == .normal { Tag(text: L("重点候选"), tint: Palette.accent) }
                            Tag(text: message.mailLevel == .metadata ? L("元数据") : message.mailLevel.title, tint: message.mailLevel == .important ? Palette.accent : nil)
                        }
                        .padding(.vertical, Space.s).contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    if let opened = model.openedMail, opened.message.id == message.id { openedPanel(opened) }
                }
            }
        }
    }

    private func openedPanel(_ opened: OpenedMail) -> some View {
        InlinePanel {
            VStack(alignment: .leading, spacing: Space.s) {
                if opened.sourceDeleted {
                    InlineMessage(kind: .warning, text: L("来源已删除：这封邮件在「邮件」App 里已不存在，正文无法读取。"))
                } else {
                    Text(opened.body.state == "ok" ? opened.body.body : opened.body.state == "partial" ? opened.body.body + "\n（「邮件」App 只下载了一部分正文）" : L("「邮件」App 没有下载这封邮件的正文。"))
                        .typography(.body).textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
                    if opened.message.mailLevel == .metadata {
                        Text(L("正文从「邮件」App 现读，不存进时间线，不参与全文搜索")).typography(.secondary).foregroundStyle(.secondary)
                    }
                }
                HStack(spacing: Space.xs) {
                    Spacer()
                    if !opened.sourceDeleted {
                        if opened.message.mailLevel < .normal { SecondaryButton(title: L("存为普通")) { Task { await model.setLevel(.normal, thread: false) } } }
                        if opened.message.mailLevel != .important { SecondaryButton(title: L("设为重点")) { Task { await model.setLevel(.important, thread: false) } } }
                        MenuButton(title: L("整个线程…")) {
                            Button(L("设为普通")) { Task { await model.setLevel(.normal, thread: true) } }
                            Button(L("设为重点")) { Task { await model.setLevel(.important, thread: true) } }
                            Button(L("只存元数据")) { Task { await model.setLevel(.metadata, thread: true) } }
                        }
                    }
                    QuietButton(title: L("收起")) { model.closeMail() }
                }
                .disabled(model.busy != nil)
            }
        }
        .padding(.bottom, Space.s)
    }
}

extension MailLevel: Comparable {
    static func < (a: MailLevel, b: MailLevel) -> Bool { MailLevel.allCases.firstIndex(of: a)! < MailLevel.allCases.firstIndex(of: b)! }
}
