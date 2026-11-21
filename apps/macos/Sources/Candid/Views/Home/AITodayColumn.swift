import SwiftUI
import CandidLocalization

/// The AI page's left column 「今天」 (agent-platform-v1 §4.2, mockup ④): 日程, 提醒, 待你处理, 回顾, and a footer line for
/// jobs. Agenda items are read-only context; the agent proposes changes for the user to approve.
struct AITodayColumn: View {
    @Bindable var model: AIPageModel
    @ViewState private var jobsOpen = false
    @ViewState private var reviewOpen = false

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: Space.s) {
                    scheduleCard
                    remindersCard
                    waitingCard
                    reviewCard
                    if let problem = model.problems["core"] { InlineMessage(kind: .error, text: problem) }
                }
                .padding(.bottom, Space.s)
            }
            .scrollIndicators(.hidden)
            jobsLine
        }
    }

    // MARK: Cards

    private func card<Content: View>(_ title: String, source: String? = nil, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: Space.xxs) {
            HStack(spacing: Space.xxs) {
                Text(title).typography(.caption, .semibold).foregroundStyle(.secondary)
                Spacer()
                if let source { Text(source).typography(.caption).foregroundStyle(.tertiary) }
            }
            .padding(.bottom, 2)
            content()
        }
        .padding(.horizontal, Space.s).padding(.vertical, Space.s - 2)
        .frame(maxWidth: .infinity, alignment: .leading)
        .panelSurface(radius: Radius.large)
    }

    static let time: DateFormatter = { let f = DateFormatter(); f.dateFormat = "HH:mm"; return f }()

    private func accessLine(_ kind: AgendaKind, _ access: AgendaAccess) -> some View {
        HStack(spacing: Space.xs) {
            if access == .notDetermined {
                Text(kind == .calendar ? L("允许读取日历后在这里显示今天的日程") : L("允许读取提醒事项后在这里显示今天和逾期的提醒"))
                    .typography(.secondary).foregroundStyle(.secondary)
                Spacer(minLength: 0)
                ChipButton(title: L("允许"), emphasized: true) { Task { await model.requestAccess(kind) } }
            } else {
                InlineMessage(kind: .note, text: kind == .calendar ? L("没有日历权限：在 系统设置 → 隐私与安全性 → 日历 里允许 Candid")
                                                                   : L("没有提醒事项权限：在 系统设置 → 隐私与安全性 → 提醒事项 里允许 Candid"))
            }
        }
    }

    // MARK: 日程

    private var scheduleCard: some View {
        card(L("日程"), source: L("日历")) {
            if model.calendarAccess != .granted {
                accessLine(.calendar, model.calendarAccess)
            } else {
                if model.day.rows.isEmpty && model.day.allDay.isEmpty {
                    Text(L("今天没有日程")).typography(.secondary).foregroundStyle(.secondary)
                }
                ForEach(model.day.allDay) { event in eventRow(event, leading: L("全天"), phase: .later) }
                ForEach(Array(model.day.rows.enumerated()), id: \.element.id) { index, row in
                    if index > 0 || !model.day.allDay.isEmpty { HairlineDivider() }
                    eventRow(row.event, leading: Self.time.string(from: row.event.start), phase: row.phase, detail: Self.eventDetail(row, now: model.currentTime))
                }
            }
            if let problem = model.problems["agenda"] { InlineMessage(kind: .error, text: problem) }
        }
    }

    private func eventRow(_ event: AgendaEvent, leading: String, phase: AgendaDay.Phase, detail: String? = nil) -> some View {
        VStack(alignment: .leading, spacing: Space.xxs) {
            AgendaItemRow(leading: leading, title: event.title, detail: detail ?? event.calendar, dimmed: phase == .past, highlighted: phase == .now,
                          ask: { model.ask(Self.context(event)) },
                          prepare: phase == .now ? { Task { await model.prepare(Self.context(event)) } } : nil,
                          preparing: model.isRunning(.prepare(Self.context(event).id)), cancel: { model.cancelRun() })
            if let problem = model.problems["run:" + AIRun.prepare(Self.context(event).id).key] { InlineMessage(kind: .error, text: problem) }

        }
    }

    static func eventDetail(_ row: AgendaDay.Row, now: Date) -> String {
        var parts: [String] = []
        switch row.phase {
        case .past: parts.append(L("已结束"))
        case .now:
            let minutes = Int(row.event.start.timeIntervalSince(now) / 60)
            parts.append(minutes > 0 ? L("%@ 分钟后", minutes) : L("进行中，到 ") + time.string(from: row.event.end))
        case .later: parts.append(L("到 ") + time.string(from: row.event.end))
        }
        if let location = row.event.location, !location.isEmpty { parts.append(location) }
        return parts.joined(separator: " · ")
    }

    static func context(_ event: AgendaEvent) -> AIContextItem {
        AIContextItem(id: "event:" + event.id, kind: L("日程"), title: (event.isAllDay ? L("全天 ") : time.string(from: event.start) + " ") + event.title,
                      detail: [event.isAllDay ? nil : time.string(from: event.start) + "–" + time.string(from: event.end), event.location, event.calendar]
                        .compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: L("，")))
    }

    // MARK: 提醒

    private var remindersCard: some View {
        let count = model.reminders.items.count + model.commitments.count
        return card(L("提醒") + (count > 0 ? " · \(count)" : ""), source: L("提醒事项")) {
            if model.remindersAccess != .granted {
                accessLine(.reminders, model.remindersAccess)
            }
            ForEach(model.reminders.items) { item in reminderRow(item) }
            ForEach(model.commitments) { commitment in candidateRow(commitment) }
            if model.reminders.tomorrow > 0 {
                Text(L("＋ %@ 条明天到期", model.reminders.tomorrow)).typography(.caption).foregroundStyle(.tertiary).padding(.top, 2)
            }
            if model.remindersAccess == .granted && count == 0 {
                Text(L("今天没有到期的提醒")).typography(.secondary).foregroundStyle(.secondary)
            }
        }
    }

    private func reminderRow(_ item: ReminderGroups.Item) -> some View {
        let r = item.reminder
        let detail = (item.overdueDays > 0 ? L("逾期 %@ 天", item.overdueDays) : L("今天到期")) + (Calendar.current.component(.hour, from: r.due) == 0 ? "" : " " + Self.time.string(from: r.due))
        return VStack(alignment: .leading, spacing: Space.xxs) {
            AgendaItemRow(title: r.title, detail: detail, detailTint: Palette.warning, dimmed: false,
                          ask: { model.ask(AIContextItem(id: "reminder:" + r.id, kind: L("提醒"), title: r.title, detail: detail + L("，列表 ") + r.list)) })

        }
    }

    private func candidateRow(_ c: AgendaCommitment) -> some View {
        let key = "commitment:" + c.id
        let origin = [L("AI 提的"), c.source.map { Self.sourceLabel($0) }].compactMap { $0 }.joined(separator: " · ")
        return VStack(alignment: .leading, spacing: Space.xxs) {
            HStack(alignment: .firstTextBaseline, spacing: Space.xs) {
                Image(systemName: "circle.dashed").foregroundStyle(Palette.ai).accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 2) {
                    Text(c.text).typography(.body)
                    Text(origin + (AgendaDate.parse(c.due).map { L(" · 到期 ") + Self.dayTime($0) } ?? "")).typography(.caption).foregroundStyle(.tertiary)
                }
            }
            HStack(spacing: Space.xxs) {
                ChipButton(title: L("问 AI"), systemImage: "sparkle") { model.ask(AIContextItem(id: "commitment:" + c.id, kind: L("承诺"), title: c.text, detail: origin)) }
                Spacer(minLength: 0)
                ChipButton(title: L("忽略")) { Task { await model.ignore(c) } }
                PrimaryButton(title: L("加到提醒")) { Task { await model.addToReminders(c) } }
            }
            .disabled(model.working.contains(key))
            if let problem = model.problems[key] { InlineMessage(kind: .error, text: problem) }
        }
        .padding(Space.xs)
        .background(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous).fill(Palette.ai.opacity(0.07)))
        .overlay(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous)
            .strokeBorder(style: StrokeStyle(lineWidth: 1, dash: [4, 3])).foregroundStyle(Palette.ai.opacity(0.6)))
    }

    static func sourceLabel(_ s: AgendaCommitment.Source) -> String {
        AIRecordRef(eventID: s.event_id, kind: s.kind, start: s.start, title: s.title).label.replacingOccurrences(of: "^", with: "来自", options: .regularExpression)
    }

    static func dayTime(_ date: Date) -> String {
        let f = DateFormatter(); f.locale = Locale(identifier: "zh_CN"); f.dateFormat = L("M月d日 HH:mm")
        return f.string(from: date)
    }

    // MARK: 待你处理

    private var waitingCard: some View {
        card(L("待你处理") + (model.waitingCount > 0 ? " · \(model.waitingCount)" : "")) {
            if model.waitingCount == 0 { Text(L("没有要你处理的事")).typography(.secondary).foregroundStyle(.secondary) }
            ForEach(Array(model.waiting.enumerated()), id: \.element.id) { index, proposal in
                if index > 0 { HairlineDivider() }
                ProposalRow(model: model, proposal: proposal, compact: false)
            }
            PeopleCandidateRows(model: model.people)
            ForEach(model.speakers) { speaker in
                let key = "speaker:" + speaker.id
                VStack(alignment: .leading, spacing: Space.xxs) {
                    HStack(alignment: .firstTextBaseline, spacing: Space.xs) {
                        Text("?").typography(.body, .bold).foregroundStyle(.tertiary).frame(width: 16).accessibilityHidden(true)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(L("%@ 可能是%@", speaker.key, speaker.name)).typography(.body)
                            Text(speaker.recording + (speaker.percent.map { " · \($0)%" } ?? "")).typography(.caption).foregroundStyle(.tertiary)
                        }
                    }
                    HStack(spacing: Space.xxs) {
                        ChipButton(title: L("问 AI"), systemImage: "sparkle") { model.ask(AIContextItem(id: key, kind: L("说话人"), title: L("%@ 可能是%@", speaker.key, speaker.name), detail: speaker.recording)) }
                        Spacer(minLength: 0)
                        SecondaryButton(title: L("是%@", speaker.name)) { Task { await model.confirm(speaker) } }
                    }
                    .disabled(model.working.contains(key))
                    if let problem = model.problems[key] { InlineMessage(kind: .error, text: problem) }
                }
            }
        }
    }

    // MARK: 回顾

    private var reviewCard: some View {
        card(L("回顾"), source: model.review.flatMap { AgendaDate.parse($0.created) }.map { Self.dayTime($0) }) {
            if let review = model.review {
                AIMarkdownView(text: review.text, label: model.recordLabel)
                    .lineLimit(reviewOpen ? nil : 4)
                    .frame(maxHeight: reviewOpen ? nil : 96, alignment: .top).clipped()
                HStack(spacing: Space.xxs) {
                    Text(review.producer.hasPrefix("pi") ? "Pi 草稿" : "AI 草稿").typography(.caption, .semibold).foregroundStyle(Palette.ai)
                    Spacer(minLength: 0)
                    ChipButton(title: reviewOpen ? L("收起") : L("展开"), systemImage: reviewOpen ? "chevron.up" : "chevron.down") { withAnimation(Motion.spring) { reviewOpen.toggle() } }
                    ChipButton(title: L("问 AI"), systemImage: "sparkle") { model.ask(AIContextItem(id: "review:" + review.digest_id, kind: L("回顾"), title: String(review.text.prefix(30)), detail: review.digest_id)) }
                }
                HStack { Spacer(minLength: 0); reviewButton(again: true) }
            } else {
                HStack {
                    Text(L("还没有日回顾")).typography(.secondary).foregroundStyle(.secondary)
                    Spacer(minLength: 0)
                    reviewButton(again: false)
                }
            }
            if let problem = model.problems["run:" + AIRun.skill("daily-review").key] { InlineMessage(kind: .error, text: problem) }
        }
    }

    private func reviewButton(again: Bool) -> some View {
        RunButton(title: again ? L("重新回顾今天") : L("回顾今天"), systemImage: "text.append", runningTitle: L("正在回顾今天…"),
                  running: model.isRunning(.skill("daily-review")),
                  start: { Task { await model.reviewToday() } }, cancel: { model.cancelRun() })
            .disabled(model.turnRunning && !model.isRunning(.skill("daily-review")))
    }

    // MARK: Jobs

    private var jobsLine: some View {
        let active = model.activeJobs, failed = model.failedJobs
        return VStack(alignment: .leading, spacing: Space.xs) {
            HStack(spacing: Space.xs) {
                if active.isEmpty && failed.isEmpty {
                    StatusDot(state: .off, text: L("没有正在处理的任务"))
                } else {
                    StatusDot(state: failed.isEmpty ? .on : .attention,
                              text: (active.isEmpty ? "" : L("正在处理 %@ 项", active.count) + (active.first.map { " · " + $0.title } ?? ""))
                                + (failed.isEmpty ? "" : (active.isEmpty ? "" : L("，")) + L("%@ 项失败", failed.count)))
                }
                Spacer(minLength: 0)
                if !(active.isEmpty && failed.isEmpty) {
                    ChipButton(title: jobsOpen ? L("收起") : L("详情"), systemImage: jobsOpen ? "chevron.down" : "chevron.up") { withAnimation(Motion.spring) { jobsOpen.toggle() } }
                }
            }
            if jobsOpen && !(active.isEmpty && failed.isEmpty) {
                InlinePanel {
                    VStack(alignment: .leading, spacing: Space.xs) {
                        ForEach(Array((active + failed).enumerated()), id: \.element.id) { index, job in
                            if index > 0 { HairlineDivider() }
                            HStack(spacing: Space.xs) {
                                VStack(alignment: .leading, spacing: 1) {
                                    Text(job.title).typography(.body)
                                    Text(model.workflows.sourceName(job) + (job.state == "failed" ? " · " + job.status : ""))
                                        .typography(.caption).foregroundStyle(job.state == "failed" ? Palette.warning : .secondary).lineLimit(2)
                                }
                                Spacer(minLength: 0)
                                if job.state == "failed" { ChipButton(title: L("重试"), systemImage: "arrow.clockwise", emphasized: true) { Task { await model.retry(job) } } }
                                else { HStack(spacing: Space.xxs) { ProgressView().controlSize(.mini); ChipButton(title: L("取消")) { Task { await model.cancel(job) } } }.help(L("取消后不再保存返回结果。正在运行的模型可能继续处理。")) }
                            }
                        }
                    }
                }
            }
        }
        .padding(.top, Space.xs)
    }
}

/// One line of the 日程 or 提醒 card: time or a check circle, title, detail; 「问 AI」 on hover (always on the current
/// event, with 「会前要点」). Clicking the title opens its editor.
struct AgendaItemRow: View {
    var leading: String?
    var check: (() -> Void)?
    let title: String
    let detail: String
    var detailTint: Color?
    let dimmed: Bool
    var highlighted = false
    var open: (() -> Void)?
    let ask: () -> Void
    var prepare: (() -> Void)?
    var preparing = false
    var cancel: () -> Void = {}
    @ViewState private var hovering = false
    @ViewState private var checked = false

    init(leading: String? = nil, check: (() -> Void)? = nil, title: String, detail: String, detailTint: Color? = nil, dimmed: Bool, highlighted: Bool = false,
         open: (() -> Void)? = nil, ask: @escaping () -> Void, prepare: (() -> Void)? = nil, preparing: Bool = false, cancel: @escaping () -> Void = {}) {
        self.leading = leading; self.check = check; self.title = title; self.detail = detail; self.detailTint = detailTint
        self.dimmed = dimmed; self.highlighted = highlighted; self.open = open; self.ask = ask; self.prepare = prepare
        self.preparing = preparing; self.cancel = cancel
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: Space.xs) {
            if let leading { Text(leading).typography(.secondary, digits: true).foregroundStyle(highlighted ? .primary : .secondary).frame(width: 38, alignment: .leading) }
            if let check {
                Button { withAnimation(Motion.snappy) { checked = true }; check() } label: {
                    Image(systemName: checked ? "checkmark.circle.fill" : "circle").foregroundStyle(checked ? Palette.accent : .secondary)
                }
                .buttonStyle(.plain).help(L("完成")).accessibilityLabel(L("完成 %@", title))
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(title).typography(.body, highlighted ? .semibold : nil).lineLimit(2)
                    .strikethrough(checked)
                if !detail.isEmpty { Text(detail).typography(.caption).foregroundStyle(detailTint ?? .secondary).lineLimit(1) }
                if highlighted, let prepare {
                    HStack(spacing: Space.xxs) {
                        RunButton(title: L("会前要点"), systemImage: "list.bullet", runningTitle: L("正在整理会前要点…"), running: preparing, start: prepare, cancel: cancel)
                        if !preparing { ChipButton(title: L("问 AI"), systemImage: "sparkle", action: ask) }
                    }
                    .padding(.top, 2)
                }
            }
            Spacer(minLength: 0)
            if hovering {
                HStack(spacing: Space.xxs) {
                    if let open { ChipButton(title: L("编辑"), systemImage: "pencil", action: open) }
                    if !(highlighted && prepare != nil) { ChipButton(title: L("问 AI"), systemImage: "sparkle", action: ask) }
                }
                .transition(.opacity)
            }
        }
        .padding(.vertical, Space.xs - 2).padding(.horizontal, highlighted ? Space.xs : 0)
        .background(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous).fill(highlighted ? Palette.accent.opacity(0.08) : .clear))
        .padding(.horizontal, highlighted ? -Space.xs : 0)
        .opacity(dimmed ? 0.55 : 1)
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
    }
}


/// A proposal that waits for the user: in 待你处理 and as a card under Pi's answer. Deleting asks once more.
struct ProposalRow: View {
    @Bindable var model: AIPageModel
    let proposal: AgendaProposal
    let compact: Bool
    @ViewState private var confirmingDelete = false

    var body: some View {
        let key = "proposal:" + proposal.id
        VStack(alignment: .leading, spacing: Space.xxs) {
            HStack(alignment: compact ? .center : .firstTextBaseline, spacing: Space.xs) {
                if compact {
                    AIActionIcon(symbol: proposal.status == "executed" ? "checkmark" : "exclamationmark", tint: proposal.status == "executed" ? Palette.success : Palette.warning)
                } else {
                    Text("!").typography(.body, .bold).foregroundStyle(Palette.warning).frame(width: 16).accessibilityHidden(true)
                }
                VStack(alignment: .leading, spacing: 2) {
                    Text(compact ? L("%@：%@", proposal.what, proposal.title) : proposal.title).typography(.body)
                    Text(subtitle).typography(.caption).foregroundStyle(.tertiary)
                }
                if compact { Spacer(minLength: Space.xs); buttons }
            }
            if !compact {
                HStack(spacing: Space.xxs) {
                    ChipButton(title: L("问 AI"), systemImage: "sparkle") { model.ask(AIContextItem(id: "proposal:" + proposal.id, kind: L("提案"), title: proposal.title, detail: proposal.what)) }
                    Spacer(minLength: 0)
                    buttons
                }
            }
            if let problem = model.problems[key] ?? (proposal.status == "failed" ? proposal.result?.error : nil) {
                InlineMessage(kind: .error, text: problem)
            }
        }
        .disabled(model.working.contains(key))
    }

    @ViewBuilder private var buttons: some View {
        switch proposal.status {
        case "pending", "failed":
            if confirmingDelete {
                ChipButton(title: L("不删")) { confirmingDelete = false }
                ChipButton(title: L("确认删除"), systemImage: "trash", role: .destructive) { confirmingDelete = false; Task { await model.approve(proposal) } }
            } else {
                SecondaryButton(title: L("拒绝")) { Task { await model.reject(proposal) } }
                if proposal.executable {
                    PrimaryButton(title: proposal.status == "failed" ? L("重试") : L("批准")) {
                        if proposal.isDelete { withAnimation(Motion.snappy) { confirmingDelete = true } } else { Task { await model.approve(proposal) } }
                    }
                }
            }
        case "approved":
            Text(L("正在执行…")).typography(.caption).foregroundStyle(.secondary)
        case "executed":
            Tag(text: L("已完成") + (proposal.result?.receipt?["container"].map { L("「%@」", $0) } ?? ""), tint: Palette.success)
        case "rejected":
            Tag(text: L("已拒绝"))
        default:
            Tag(text: proposal.status)
        }
    }

    private var subtitle: String {
        var parts = [compact ? L("App 外 · 要你批准") : L("提案 · ") + proposal.what]
        if let due = AgendaDate.parse(proposal.payload.due ?? proposal.payload.start) { parts.append(AITodayColumn.dayTime(due)) }
        parts.append(proposal.fromPi ? L("来自 AI 对话") : proposal.payload.memory_id != nil ? L("来自承诺") : proposal.client_name.replacingOccurrences(of: "Candid ", with: "来自 "))
        return parts.joined(separator: " · ")
    }
}
