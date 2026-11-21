import AppKit
import Foundation
import SwiftUI
import CandidClient
import Testing
@testable import Candid

/// The AI page (agent-platform-v1 §4.2, §4.3): agenda grouping, the narrow-layout switch, Pi's tool results as cards,
/// proposal approval through a fake EventKit against a real core, and a chat turn with the pinned Pi and the scripted
/// model (agents/pi-extension/test/mock-model.mjs). No real model, calendar or Reminders; synthetic data only.

/// Stands in for EventKit: records writes, can be told to fail or to deny access.
final class FakeAgenda: SystemAgenda, @unchecked Sendable {
    var calendar: AgendaAccess = .granted
    var remindersAccess: AgendaAccess = .granted
    var grantOnRequest = true
    var events: [AgendaEvent] = []
    var reminderItems: [AgendaReminder] = []
    var failWrites: Error?
    private(set) var written: [(kind: String, draft: AgendaDraft)] = []
    private(set) var excludedSeen: Set<String> = []

    func access(_ kind: AgendaKind) -> AgendaAccess { kind == .calendar ? calendar : remindersAccess }
    func requestAccess(_ kind: AgendaKind) async -> AgendaAccess {
        let result: AgendaAccess = grantOnRequest ? .granted : .denied
        if kind == .calendar { calendar = result } else { remindersAccess = result }
        return result
    }
    func events(from start: Date, to end: Date, excluding: Set<String>) async throws -> [AgendaEvent] {
        excludedSeen = excluding
        return events.filter { $0.end > start && $0.start < end && !excluding.contains($0.calendarID) }
    }
    func reminders(dueBefore end: Date, excluding: Set<String>) async throws -> [AgendaReminder] {
        reminderItems.filter { $0.due < end && !excluding.contains($0.listID) }
    }
    func createReminder(_ draft: AgendaDraft) async throws -> AgendaReceipt {
        if let failWrites { throw failWrites }
        written.append(("reminder", draft))
        return AgendaReceipt(itemID: "fake-reminder-\(written.count)", container: "合成列表")
    }
    func createEvent(_ draft: AgendaDraft) async throws -> AgendaReceipt {
        if let failWrites { throw failWrites }
        guard draft.start != nil else { throw AgendaWriteError.incomplete("开始时间") }
        written.append(("event", draft))
        return AgendaReceipt(itemID: "fake-event-\(written.count)", container: "合成日历")
    }
    func updateReminder(_ id: String, _ draft: AgendaDraft) async throws -> AgendaReceipt {
        if let failWrites { throw failWrites }
        guard let i = reminderItems.firstIndex(where: { $0.id == id }) else { throw AgendaWriteError.missing(.reminders) }
        if !draft.title.isEmpty { reminderItems[i].title = draft.title }
        if let due = draft.due { reminderItems[i].due = due }
        if let notes = draft.notes { reminderItems[i].notes = notes }
        written.append(("update-reminder", draft))
        return AgendaReceipt(itemID: id, container: "提醒")
    }
    func completeReminder(_ id: String) async throws -> AgendaReceipt {
        guard let i = reminderItems.firstIndex(where: { $0.id == id }) else { throw AgendaWriteError.missing(.reminders) }
        reminderItems.remove(at: i)
        written.append(("complete-reminder", AgendaDraft(title: id)))
        return AgendaReceipt(itemID: id, container: "提醒")
    }
    func deleteReminder(_ id: String) async throws -> AgendaReceipt {
        guard let i = reminderItems.firstIndex(where: { $0.id == id }) else { throw AgendaWriteError.missing(.reminders) }
        reminderItems.remove(at: i)
        written.append(("delete-reminder", AgendaDraft(title: id)))
        return AgendaReceipt(itemID: id, container: "提醒")
    }
    func updateEvent(_ id: String, _ draft: AgendaDraft) async throws -> AgendaReceipt {
        guard let i = events.firstIndex(where: { $0.id == id }) else { throw AgendaWriteError.missing(.calendar) }
        if !draft.title.isEmpty { events[i].title = draft.title }
        if let start = draft.start { events[i].start = start; events[i].end = draft.end ?? start.addingTimeInterval(1800) }
        if let notes = draft.notes { events[i].notes = notes }
        written.append(("update-event", draft))
        return AgendaReceipt(itemID: events[i].id, container: "工作")
    }
    func deleteEvent(_ id: String) async throws -> AgendaReceipt {
        guard let i = events.firstIndex(where: { $0.id == id }) else { throw AgendaWriteError.missing(.calendar) }
        events.remove(at: i)
        written.append(("delete-event", AgendaDraft(title: id)))
        return AgendaReceipt(itemID: id, container: "工作")
    }
}

/// A scripted agent for chat tests that do not need Pi itself.
@MainActor final class FakeChatAgent: AIChatAgent {
    var onEvent: ((PiEvent) -> Void)?
    var isRunning = false
    var started: [PiRunConfig] = []
    var prompts: [String] = []
    var startError: Error?
    var script: [PiEvent] = []

    func start(_ config: PiRunConfig) async throws {
        if let startError { throw startError }
        started.append(config); isRunning = true
    }
    func prompt(_ text: String) throws {
        prompts.append(text)
        for event in script { onEvent?(event) }
    }
    func cancel() { onEvent?(.settled(aborted: true)) }
    func stop() async { isRunning = false }
}

@MainActor @Suite(.serialized) struct AIPageTests {
    static let noon: Date = {
        var c = DateComponents(); c.year = 2030; c.month = 1; c.day = 2; c.hour = 13; c.minute = 40
        return Calendar.current.date(from: c)!
    }()
    static func at(_ hour: Int, _ minute: Int = 0, day: Int = 0) -> Date {
        Calendar.current.date(byAdding: .day, value: day, to: Calendar.current.date(bySettingHour: hour, minute: minute, second: 0, of: noon)!)!
    }
    static func event(_ id: String, _ start: Date, _ end: Date, allDay: Bool = false, calendar: String = "work") -> AgendaEvent {
        AgendaEvent(id: id, title: "【合成】" + id, start: start, end: end, isAllDay: allDay, location: nil, calendarID: calendar, calendar: "工作")
    }
    static func folder(_ name: String) -> URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("ai-page-\(name)-" + UUID().uuidString, isDirectory: true)
    }

    // MARK: Grouping and layout

    @Test func agendaGroupsPastNowAndLaterAndKeepsAllDayApart() {
        let day = AgendaDay.make([
            Self.event("评审", Self.at(16, 30), Self.at(17, 30)),
            Self.event("站会", Self.at(9, 30), Self.at(9, 45)),
            Self.event("报价", Self.at(14), Self.at(14, 30)),
            Self.event("假期", Self.at(0), Self.at(0, day: 1), allDay: true),
            Self.event("进行中", Self.at(13), Self.at(14)),
        ], now: Self.noon)
        #expect(day.allDay.map(\.id) == ["假期"])
        #expect(day.rows.map(\.id) == ["站会", "进行中", "报价", "评审"])
        #expect(day.rows.map(\.phase) == [.past, .now, .now, .later], "starting within 30 minutes counts as now")
        #expect(AITodayColumn.eventDetail(day.rows[2], now: Self.noon) == "20 分钟后")
        #expect(AITodayColumn.eventDetail(day.rows[0], now: Self.noon) == "已结束")
    }

    @Test func remindersPutOverdueFirstAndOnlyCountTomorrow() {
        func r(_ id: String, _ due: Date) -> AgendaReminder { AgendaReminder(id: id, title: id, due: due, listID: "l", list: "提醒") }
        let groups = ReminderGroups.make([r("今天", Self.at(18)), r("前天", Self.at(9, day: -2)), r("明天", Self.at(9, day: 1)),
                                          r("昨天", Self.at(20, day: -1)), r("后天", Self.at(9, day: 2))], now: Self.noon)
        #expect(groups.items.map(\.id) == ["前天", "昨天", "今天"])
        #expect(groups.items.map(\.overdueDays) == [2, 1, 0])
        #expect(groups.tomorrow == 1)
    }

    @Test func narrowLayoutSwitchesToChatOnAskAndMarksUnreadAnswers() async {
        #expect(!AIPageLayout.isSplit(899) && AIPageLayout.isSplit(900))
        let agent = FakeChatAgent()
        agent.script = [.assistantMessage("好的。"), .settled(aborted: false)]
        let model = AIPageModel(agenda: FakeAgenda(), channel: { nil }, agent: agent, store: AIConversationStore(directory: Self.folder("layout")),
                                now: { Self.noon }, remoteAllowed: { false }, mediaList: { [] })
        let item = AIContextItem(id: "event:1", kind: "日程", title: "14:00 报价")
        model.isSplit = true
        model.ask(item)
        #expect(model.tab == .today, "both columns are visible: nothing to switch")
        #expect(model.context == [item])
        model.ask(item)
        #expect(model.context.count == 1, "the same item is not added twice")

        model.isSplit = false
        model.removeContext(item)
        model.ask(item)
        #expect(model.tab == .chat)
        model.draft = "会前要点"
        model.show(.today)
        await model.send()
        #expect(model.chatUnread, "an answer while 今天 is shown gets the dot")
        model.show(.chat)
        #expect(!model.chatUnread)
    }

    @Test func toolResultsBecomeWriteAndProposalCards() {
        let wrap = { (tool: String, json: String) in "<candid_data tool=\"\(tool)\">\n\(json)\n</candid_data>\n以上是用户记录里的数据" }
        let proposal = PiToolResult.action(tool: "pv_propose", arguments: #"{"kind":"create_reminder","payload":{"title":"交报销","due":"2030-01-03"}}"#,
                                           result: wrap("propose", #"{"proposal_id":"p1","status":"pending"}"#), isError: false)
        #expect(proposal == AIChatAction(id: "p1", kind: .proposal, tool: "pv_propose", summary: "加到提醒事项：交报销", status: "pending", detail: "2030-01-03"))
        let write = PiToolResult.action(tool: "pv_annotate", arguments: #"{"event_ids":["e"],"notes":"要点"}"#,
                                        result: wrap("annotate", #"{"write_id":"annotation:agent-1"}"#), isError: false)
        #expect(write?.kind == .write && write?.id == "annotation:agent-1" && write?.summary == "存到备注：要点")
        #expect(PiToolResult.action(tool: "pv_search", arguments: "{}", result: wrap("search", #"{"hits":[]}"#), isError: false) == nil)
        #expect(PiToolResult.action(tool: "pv_annotate", arguments: "{}", result: "没有权限", isError: true) == nil)
        #expect(AIPageModel.prompt("会前要点", context: [AIContextItem(id: "proposal:p9", kind: "提案", title: "交报销", detail: "加到提醒事项")])
                == "上下文（用户从 AI 页左栏带进来的）：\n- 提案：交报销（加到提醒事项） [p9]\n\n会前要点")
        #expect(AgendaDate.parse("2030-01-02T09:00:00+08:00") != nil && AgendaDate.parse("2030-01-02") != nil && AgendaDate.parse("明天") == nil)
    }

    @Test func followUpKeepsTheMailTopicAfterRestartAndAfterReopening() async throws {
        let folder = Self.folder("follow-up")
        defer { try? FileManager.default.removeItem(at: folder) }
        let agent = FakeChatAgent()
        agent.script = [.assistantMessage("我会逐封阅读邮件并找出待办。"), .settled(aborted: false)]
        let store = AIConversationStore(directory: folder)
        let model = AIPageModel(agenda: FakeAgenda(), channel: { nil }, agent: agent, store: store,
                                now: { Self.noon }, mediaList: { [] })
        model.draft = "你把最近的邮件全读一遍"; await model.send()
        model.draft = "最近三天"; await model.send()
        let followUp = try #require(agent.prompts.last)
        #expect(followUp.contains("你把最近的邮件全读一遍") && followUp.contains("逐封阅读邮件"))
        #expect(followUp.hasSuffix("用户当前请求：\n最近三天"))
        #expect(followUp.contains("当前本地时间：") && followUp.contains("candid_conversation"))
        await model.newConversation()
        model.draft = "你好"; await model.send()
        #expect(agent.prompts.last?.contains("邮件") == false)
        let past = try #require(store.list().first { $0.messages.contains { $0.text == "最近三天" } })
        await model.open(past); model.draft = "里面有哪些待办"; await model.send()
        #expect(agent.prompts.last?.contains("最近三天") == true)
        let long = [AIChatMessage(role: .user, text: String(repeating: "合成", count: 30000))]
        #expect(AIPageModel.turnPrompt("现在的问题", history: long, now: Self.noon).count < 25000)
    }

    @Test func chatTurnsAreKeptLocallyWithContextAndFailuresShown() async throws {
        let folder = Self.folder("history")
        defer { try? FileManager.default.removeItem(at: folder) }
        let agent = FakeChatAgent()
        agent.script = [.toolStarted(id: "c1", name: "pv_search", arguments: "{}"), .toolFinished(id: "c1", name: "pv_search", result: "", isError: false),
                        .textDelta("上次"), .assistantMessage("上次定了 85 万。"), .settled(aborted: false)]
        let model = AIPageModel(agenda: FakeAgenda(), channel: { nil }, agent: agent, store: AIConversationStore(directory: folder),
                                now: { Self.noon }, remoteAllowed: { true }, mediaList: { [] })
        model.ask(AIContextItem(id: "event:1", kind: "日程", title: "14:00 报价", detail: "14:00–14:30"))
        model.draft = "会前要点"
        await model.send()
        #expect(agent.started.first?.allowRemote == true)
        #expect(agent.started.first?.writes == ["annotate", "reference", "commitment", "draft"])
        #expect(agent.prompts.first?.contains("- 日程：14:00 报价（14:00–14:30）") == true)
        #expect(model.context.isEmpty && model.draft.isEmpty)
        let answer = model.conversation.messages.last
        #expect(answer?.role == .assistant && answer?.text == "上次定了 85 万。" && answer?.steps == ["搜索"])
        #expect(model.conversation.title == "14:00 报价")
        #expect(model.history.map(\.id) == [model.conversation.id], "kept on this Mac")
        let reopened = AIConversationStore(directory: folder).list()
        #expect(reopened.first?.messages.first?.context.first?.title == "14:00 报价")

        await model.newConversation()
        #expect(model.conversation.messages.isEmpty && model.history.count == 1)
        await model.open(reopened[0])
        #expect(model.conversation.messages.count == 2)

        // Starting Pi can fail (no model for the role): the page says what to do.
        let failing = FakeChatAgent()
        failing.startError = CoreError.service(status: 409, code: "waiting_model", message: "x")
        let other = AIPageModel(agenda: FakeAgenda(), channel: { nil }, agent: failing, store: AIConversationStore(directory: folder),
                                now: { Self.noon }, remoteAllowed: { false }, mediaList: { [] })
        other.draft = "你好"
        await other.send()
        #expect(other.agentProblem?.contains("agent_chat") == true)
        #expect(other.conversation.messages.map(\.role) == [.user, .note])
        #expect(!other.turnRunning)
    }

    // MARK: Against a real core

    private func post(_ channel: LocalChannel, _ path: String, _ body: [String: Any]) async throws -> [String: Any] {
        let data = try await channel.send("POST", path, body: JSONSerialization.data(withJSONObject: body)).checked()
        return (try JSONSerialization.jsonObject(with: data) as? [String: Any]) ?? [:]
    }

    @Test func approvingWritesThroughEventKitAndRecordsTheReceipt() async throws {
        let root = Self.folder("approve")
        let channel = try await TestCore.ready(root, cutover: false).channel!
        defer { TestCore.finish(root) }
        func propose(_ kind: String, _ payload: [String: Any]) async throws -> String {
            try await post(channel, "/v1/propose", ["kind": kind, "payload": payload])["proposal_id"] as! String
        }
        let reminder = try await propose("create_reminder", ["title": "【合成】交差旅报销", "due": "2030-01-02T18:00:00+08:00", "notes": "合成"])
        let event = try await propose("create_calendar_event", ["title": "【合成】预算复核", "start": "2030-01-03T14:00:00+08:00", "end": "2030-01-03T14:30:00+08:00"])
        let rejected = try await propose("create_reminder", ["title": "【合成】不要的"])
        let unknown = try await propose("send_email", ["to": "nobody"])
        let update = try await propose("update_reminder", ["item_id": "reminder:r1", "title": "【合成】交差旅报销（改）"])
        let complete = try await propose("complete_reminder", ["item_id": "r1"])
        let delete = try await propose("delete_calendar_event", ["item_id": AgendaEvent.id(item: "ev-1", start: Self.at(9, 30))])
        let noItem = try await propose("delete_reminder", ["title": "哪一条？"])

        let agenda = FakeAgenda()
        agenda.events = [Self.event(AgendaEvent.id(item: "ev-1", start: Self.at(9, 30)), Self.at(9, 30), Self.at(9, 45)), Self.event("私人", Self.at(10), Self.at(11), calendar: "home")]
        agenda.reminderItems = [AgendaReminder(id: "r1", title: "【合成】交报销", due: Self.at(9, day: -1), listID: "l", list: "提醒")]
        var clock = Self.noon
        let model = AIPageModel(agenda: agenda, channel: { channel }, agent: FakeChatAgent(), store: AIConversationStore(directory: root.appendingPathComponent("chats")),
                                now: { clock }, excludedCalendars: { ["home"] }, remoteAllowed: { false }, mediaList: { [] })
        await model.refresh()
        #expect(model.day.rows.map(\.id) == [AgendaEvent.id(item: "ev-1", start: Self.at(9, 30))], "excluded calendars are left out")
        #expect(agenda.excludedSeen == ["home"])
        #expect(model.reminders.items.first?.overdueDays == 1)
        #expect(Set(model.waiting.map(\.id)) == [reminder, event, rejected, unknown, update, complete, delete, noItem])
        #expect(model.proposal(unknown)?.executable == false)

        // Approve: the receipt of the system write is recorded on the proposal.
        await model.approve(model.proposal(reminder)!)
        #expect(agenda.written.count == 1 && agenda.written[0].kind == "reminder")
        #expect(agenda.written[0].draft.title == "【合成】交差旅报销" && agenda.written[0].draft.due != nil && agenda.written[0].draft.notes == "合成")
        #expect(model.proposal(reminder)?.status == "executed")
        #expect(model.proposal(reminder)?.result?.receipt?["item_id"] == "fake-reminder-1")
        #expect(!model.waiting.contains { $0.id == reminder })

        // Failure: the reason is kept on the proposal; a retry after fixing it succeeds.
        agenda.failWrites = AgendaWriteError.noAccess(.calendar)
        await model.approve(model.proposal(event)!)
        #expect(model.proposal(event)?.status == "failed")
        #expect(model.proposal(event)?.result?.error?.contains("日历") == true)
        #expect(model.problems["proposal:" + event] != nil)
        agenda.failWrites = nil
        await model.approve(model.proposal(event)!)
        #expect(model.proposal(event)?.status == "executed" && agenda.written.last?.kind == "event")

        // Reject: nothing is written.
        await model.reject(model.proposal(rejected)!)
        #expect(model.proposal(rejected)?.status == "rejected" && agenda.written.count == 2)

        // A kind the App cannot carry out is refused by the core; nothing reaches EventKit.
        await model.approve(model.proposal(unknown)!)
        #expect(model.problems["proposal:" + unknown] != nil && agenda.written.count == 2)

        // Pi's edits of existing items: each approved one goes to EventKit with its receipt; a missing item_id fails.
        await model.approve(model.proposal(update)!)
        #expect(model.proposal(update)?.status == "executed" && agenda.reminderItems.first?.title == "【合成】交差旅报销（改）")
        await model.approve(model.proposal(complete)!)
        #expect(model.proposal(complete)?.status == "executed" && agenda.reminderItems.isEmpty)
        await model.approve(model.proposal(delete)!)
        #expect(model.proposal(delete)?.status == "executed" && model.proposal(delete)?.result?.receipt?["container"] == "工作")
        await model.approve(model.proposal(noItem)!)
        #expect(model.proposal(noItem)?.status == "failed" && model.proposal(noItem)?.result?.error?.contains("item_id") == true)
        await model.reject(model.proposal(noItem)!)

        // What the core now says after a fresh read: today's decided ones are still listed (the chat cards show them).
        clock = Date()
        await model.refreshCore()
        #expect(model.waiting.map(\.id) == [unknown])
        let executed = model.proposals.filter { $0.status == "executed" }.map(\.id)
        #expect(Set(executed) == [reminder, event, update, complete, delete])

        // An unknown commitment is reported in place, not thrown.
        await model.addToReminders(AgendaCommitment(memory_id: "missing", text: "x", due: nil, producer: "t", confidence: nil, source: nil))
        #expect(model.problems["commitment:missing"] != nil)
        // The same for an item and a memo found in conversations; the snapshot carries none for an empty library.
        await model.add(AgendaItem(item_id: "it-missing", kind: "event", owner: "self", title: "x"))
        #expect(model.problems["item:it-missing"] != nil)
        await model.decide(AgendaMemo(memory_id: "mem-missing", text: "x"), keep: true)
        #expect(model.problems["memo:mem-missing"] != nil)
        #expect(model.items.isEmpty && model.memos.isEmpty && model.waitingCount == 1)
    }

    @Test func conversationItemsDecodeAndGroupIntoAgendaAndReminders() throws {
        let json = """
        {"proposals":[],"commitments":[],"review":null,
         "items":[{"item_id":"it-1","kind":"event","owner":"self","title":"和张三见面聊报价","when_text":"周六下午三点","start":"2027-01-16T15:00:00.000+08:00","all_day":false,"person":{"name":"张三","key":"handle:wechat/张三"},"conversation":{"title":"张三","app":"微信"}},
                  {"item_id":"it-2","kind":"commitment","owner":"other","title":"对方发报价单","when_text":null,"start":null,"person":null}],
         "memos":[{"memory_id":"mem-1","text":"张三下午三点后才有空","person":{"name":"张三"}}]}
        """
        let snapshot = try JSONDecoder().decode(AgendaSnapshot.self, from: Data(json.utf8))
        #expect(snapshot.items?.map(\.id) == ["it-1", "it-2"] && snapshot.memos?.first?.person?.name == "张三")
        #expect(snapshot.items?[0].isEvent == true && snapshot.items?[0].origin.hasPrefix("张三 · 周六下午三点") == true)
        #expect(snapshot.items?[1].origin == "等对方")
        // Older cores send neither list.
        let old = try JSONDecoder().decode(AgendaSnapshot.self, from: Data(#"{"proposals":[],"commitments":[]}"#.utf8))
        #expect(old.items == nil && old.memos == nil)
    }

    @Test func deniedCalendarAccessShowsAHintInsteadOfTheList() async {
        let agenda = FakeAgenda()
        agenda.calendar = .denied
        agenda.remindersAccess = .notDetermined
        agenda.events = [Self.event("站会", Self.at(9, 30), Self.at(9, 45))]
        let model = AIPageModel(agenda: agenda, channel: { nil }, agent: FakeChatAgent(), store: AIConversationStore(directory: Self.folder("denied")),
                                now: { Self.noon }, remoteAllowed: { false }, mediaList: { [] })
        await model.refreshSystem()
        #expect(model.calendarAccess == .denied && model.day.rows.isEmpty)
        #expect(model.remindersAccess == .notDetermined)
        await model.requestAccess(.reminders)
        #expect(model.remindersAccess == .granted)
        agenda.grantOnRequest = false
        agenda.calendar = .notDetermined
        await model.requestAccess(.calendar)
        #expect(model.calendarAccess == .denied && model.day.rows.isEmpty)
    }

    // MARK: Markdown, citations, steps

    @Test func markdownBecomesBlocksAndCitationsBecomeChips() {
        let blocks = AIMarkdown.blocks("## 要点\n上次**定了**预算。\n\n1. 第一\n2. 第二\n   续行\n- 子项\n\n> 引用\n\n```swift\nlet x = 1\n```\n| a | b |\n|---|---|\n| 1 | 2 |\n---")
        #expect(blocks == [.heading(level: 2, text: "要点"), .paragraph("上次**定了**预算。"), .listItem(marker: "1.", depth: 0, text: "第一"),
                           .listItem(marker: "2.", depth: 0, text: "第二\n续行"), .listItem(marker: "•", depth: 0, text: "子项"), .quote("引用"),
                           .code(language: "swift", text: "let x = 1"), .table([["a", "b"], ["1", "2"]]), .rule])
        let id = "01a1c33c-1111-7222-8333-444455556666"
        let label: (String) -> String = { $0 == id ? "录音 11月10日 10:14" : "记录" }
        let scheme = AIMarkdown.recordScheme
        #expect(AIMarkdown.citations("见 [[\(id)]]。", label: label) == "见 [录音 11月10日 10:14](\(scheme)://\(id))。")
        #expect(AIMarkdown.citations("event_id: `\(id)`", label: label) == "[录音 11月10日 10:14](\(scheme)://\(id))", "a labelled id loses its label")
        #expect(AIMarkdown.citations("裸 \(id) 结束", label: label) == "裸 [录音 11月10日 10:14](\(scheme)://\(id)) 结束")
        #expect(AIMarkdown.citations("[这次会](record:\(id))", label: label) == "[录音 11月10日 10:14](\(scheme)://\(id))")
        #expect(AIMarkdown.citations("代码 `\(id)` 和 `let a = 1`\n```\n\(id)\n```", label: label) == "代码 [录音 11月10日 10:14](\(scheme)://\(id)) 和 `let a = 1`\n```\n\(id)\n```", "an id alone in inline code is a citation; other code stays")
        let styled = AIMarkdownView.styled("**粗** `码` [录音](\(scheme)://\(id))")
        #expect(String(styled.characters) == "粗 码 录音")
        #expect(styled.runs.contains { $0.link?.absoluteString == "\(scheme)://\(id)" })
        #expect(styled.runs.contains { $0.inlinePresentationIntent?.contains(.stronglyEmphasized) == true })
        #expect(styled.runs.contains { $0.inlinePresentationIntent?.contains(.code) == true })
    }

    @Test func toolCallsBecomeAStepsLineAndALiveState() {
        let result = "<candid_data tool=\"search\">\n" + #"{"hits":[{"event_id":"e1","kind":"speech.segment","start":"2030-01-02T15:30:00+08:00","title":"周会"},{"event_id":"e2","kind":"mail.message","start":"2030-01-02T09:00:00+08:00"},{"event_id":"e3","kind":"mail.message"},{"event_id":"e1","kind":"speech.segment"}]}"# + "\n</candid_data>"
        let refs = PiToolResult.records(result)
        #expect(refs.map(\.eventID) == ["e1", "e2", "e3"])
        #expect(refs[0].kindLabel == "录音" && refs[1].kindLabel == "邮件")
        #expect(PiToolResult.outcome(result, isError: false) == "4 条")
        var message = AIChatMessage(role: .assistant, text: "")
        message.calls = [AIToolCall(id: "1", tool: "pv_search", arguments: #"{"q":"预算"}"#, state: .done, outcome: "4 条", records: refs)]
        #expect(message.stepsSummary?.hasPrefix("查了 3 条记录 · 录音 ") == true)
        #expect(message.stepsSummary?.hasSuffix("、邮件 2 条") == true)
        message.calls = [AIToolCall(id: "1", tool: "pv_status", arguments: "{}", state: .done)]
        #expect(message.stepsSummary == "用了 状态")
        #expect(PiToolResult.live("pv_search", arguments: #"{"q":"预算"}"#) == "正在搜索「预算」…")
        #expect(PiToolResult.live("pv_timeline", arguments: "{}") == "正在翻时间线…")
        // Conversations saved before calls were kept still open.
        let old = #"{"id":"6F9619FF-8B86-D011-B42D-00C04FC964FF","role":"assistant","text":"x","at":"2030-01-02T00:00:00Z","context":[],"steps":["搜索"],"actions":[]}"#
        let decoder = JSONDecoder(); decoder.dateDecodingStrategy = .iso8601
        let decoded = try? decoder.decode(AIChatMessage.self, from: Data(old.utf8))
        #expect(decoded?.calls == nil && decoded?.stepsSummary == "查了 搜索")
    }

    // MARK: Editing

    @Test func systemAgendaIDsPreserveItemIdentity() {
        #expect(AgendaEvent.parse(AgendaEvent.id(item: "abc@x", start: Self.at(9))) == ("abc@x", Self.at(9)))
    }

    static let skillsFolder = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("../../../../agents/skills").standardizedFileURL

    @Test func skillsAreListedWithTriggerAndState() {
        let skills = AISkill.load(from: [Self.folder("none"), Self.skillsFolder])
        #expect(Array(skills.map(\.id).prefix(2)) == ["daily-review", "references-replies"], "the two the page names come first")
        #expect(Set(skills.map(\.id)).isSuperset(of: ["commitment-tracker", "meeting-review", "project-history"]))
        let daily = skills[0]
        #expect(daily.title == "日回顾" && daily.trigger == .timed("21:00") && !daily.enabled)
        #expect(daily.triggerLabel == "定时 · 每天 21:00" && skills[1].triggerLabel == "手动")
        #expect(skills[1].title == "资料与回复检索" && skills[1].trigger == .manual && skills[1].enabled && skills[1].input != nil)
        #expect(skills.allSatisfy { !$0.summary.isEmpty && $0.body.contains("pv_") })
        let prompt = daily.prompt(today: Self.noon, context: [AIContextItem(id: "event:x@1", kind: "日程", title: "报价")], extra: "重点看预算")
        #expect(prompt.hasPrefix("运行技能「日回顾」。今天是 2030-01-02"))
        #expect(prompt.contains("pv_save_draft") && prompt.contains("- 日程：报价 [event:x@1]") && prompt.hasSuffix("用户补充：重点看预算"))
        #expect(AISkill.parse(id: "x", text: "no front matter") == nil)
    }

    @Test func longRunsShowProgressAndReallyCancel() async {
        let skills = AISkill.load(from: [Self.skillsFolder])
        let agent = FakeChatAgent()
        let model = AIPageModel(agenda: FakeAgenda(), channel: { nil }, agent: agent, store: AIConversationStore(directory: Self.folder("runs")),
                                now: { Self.noon }, remoteAllowed: { false }, skills: skills, mediaList: { [] })
        // 回顾今天 runs the 日回顾 skill through Pi; its control shows the run until it ends or is cancelled.
        await model.reviewToday()
        #expect(model.isRunning(.skill("daily-review")) && !model.isRunning(.chat))
        #expect(agent.prompts.last?.hasPrefix("运行技能「日回顾」") == true)
        #expect(model.conversation.messages.first?.text == "运行「日回顾」")
        await model.runSkill(skills[1])
        #expect(model.problems["run:skill:references-replies"] == nil, "a skill with input waits for it instead")
        #expect(model.pendingSkill?.id == "references-replies")
        model.pendingSkill = nil
        model.cancelRun()
        #expect(!model.turnRunning && !model.isRunning(.skill("daily-review")))
        #expect(model.conversation.messages.last?.stopped == true || model.conversation.messages.last?.role == .user)

        // A skill that needs input runs with what the user writes.
        await model.runSkill(skills[1])
        #expect(model.pendingSkill != nil && model.tab == .chat)
        model.draft = "怎么回复请假"
        await model.send()
        #expect(agent.prompts.last?.contains("用户补充：怎么回复请假") == true && model.isRunning(.skill("references-replies")))
        model.cancelRun()

        // 会前要点 is a run of its own.
        let event = AIContextItem(id: "event:e@1", kind: "日程", title: "14:00 报价")
        await model.prepare(event)
        #expect(model.isRunning(.prepare("event:e@1")) && agent.prompts.last?.contains("[event:e@1]") == true)
        await model.reviewToday()
        #expect(model.problems["run:skill:daily-review"] == "上一轮还没结束", "one run at a time")
        model.cancelRun()

        // Starting Pi can fail: the reason shows where the run was started.
        agent.isRunning = false
        agent.startError = CoreError.service(status: 409, code: "waiting_model", message: "x")
        await model.reviewToday()
        #expect(model.problems["run:skill:daily-review"]?.contains("agent_chat") == true && !model.turnRunning)
        // Without the skill files the button says what is missing.
        let bare = AIPageModel(agenda: FakeAgenda(), channel: { nil }, agent: FakeChatAgent(), store: AIConversationStore(directory: Self.folder("bare")),
                               remoteAllowed: { false }, skills: [], mediaList: { [] })
        await bare.reviewToday()
        #expect(bare.problems["run:skill:daily-review"]?.contains("找不到日回顾技能") == true)
    }

    /// Everything clickable on the AI page is a design-system button, chip or icon button: no text-only buttons at rest,
    /// no tap gestures on plain text.
    @Test func agendaIsManagedThroughAgentProposalsWithoutManualForms() throws {
        let file = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources/Candid/Views/Home/AITodayColumn.swift")
        let source = try String(contentsOf: file, encoding: .utf8)
        #expect(!source.contains("AgendaEditor") && !source.contains("DatePicker") && !source.contains("new-event") && !source.contains("new-reminder"))
        #expect(source.contains("ProposalRow") && source.contains("model.ask"))
    }

    @Test func aiPageHasNoPlainTextControls() throws {
        let views = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("../../Sources/Candid/Views/Home").standardizedFileURL
        for file in ["AIHomeView.swift", "AITodayColumn.swift", "AIChatColumn.swift", "AISkillsPanel.swift"] {
            let text = try String(contentsOf: views.appendingPathComponent(file), encoding: .utf8)
            for banned in ["QuietButton(", "QuietMenu(", ".onTapGesture", "ConfirmButton("] {
                #expect(!text.contains(banned), "\(file) uses \(banned)")
            }
        }
    }

    /// A reply is as tall as its content: no reserved block between the steps line and the text, while streaming or after.
    @Test func repliesAreAsTallAsTheirContent() async throws {
        let agent = FakeChatAgent()
        let model = AIPageModel(agenda: FakeAgenda(), channel: { nil }, agent: agent, store: AIConversationStore(directory: Self.folder("height")),
                                now: { Self.noon }, remoteAllowed: { false }, skills: [], mediaList: { [] })
        let wrap = "<candid_data tool=\"overview\">\n{\"groups\":[]}\n</candid_data>"
        let long = String(repeating: "很长的中间输出。\n", count: 60)
        agent.script = [.toolStarted(id: "o", name: "pv_overview", arguments: "{}"), .toolFinished(id: "o", name: "pv_overview", result: wrap, isError: false),
                        .textDelta(long), .assistantMessage("今天微信切到前台 **12 次**，共 48 分钟。"), .settled(aborted: false)]
        model.draft = "我今天看了多少次微信"
        await model.send()
        let answer = try #require(model.conversation.messages.last)
        #expect(answer.text == "今天微信切到前台 **12 次**，共 48 分钟。")
        func height(_ message: AIChatMessage) -> CGFloat {
            let host = NSHostingController(rootView: AIMessageView(model: model, message: message, live: false).frame(width: 640))
            return host.sizeThatFits(in: CGSize(width: 640, height: 4000)).height
        }
        let short = height(answer)
        #expect(short > 30 && short < 110, "steps line plus one line of text, got \(short)")
        var streaming = answer; streaming.text = long
        #expect(height(streaming) > 600, "a long text is long")
        #expect(height(answer) == short, "and the finished one shrinks back")
        let source = try String(contentsOf: URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("../../Sources/Candid/Views/Home/AIChatColumn.swift"), encoding: .utf8)
        #expect(!source.contains("LazyVStack("), "the message list keeps no estimated heights")
    }

    // MARK: History

    @Test func historyIsGroupedSearchableRenamableAndDeletable() async throws {
        let folder = Self.folder("hist")
        let store = AIConversationStore(directory: folder)
        func conversation(_ title: String, _ updated: Date, answer: String, calls: [AIToolCall] = []) -> AIConversation {
            var a = AIChatMessage(role: .assistant, text: answer); a.calls = calls
            a.actions = [AIChatAction(id: "annotation:1", kind: .write, tool: "pv_annotate", summary: "存到备注", status: "done")]
            return AIConversation(title: title, created: updated, updated: updated, messages: [AIChatMessage(role: .user, text: title), a])
        }
        let ref = AIRecordRef(eventID: "e1", kind: "speech.segment", start: nil, title: nil)
        try store.save(conversation("会前要点", Self.at(11), answer: "上次 [[e1]] 定了 **85 万**。\n- 一\n- 二 [[e2]]",
                                    calls: [AIToolCall(id: "1", tool: "pv_search", arguments: "{}", state: .done, records: [ref])]))
        try store.save(conversation("昨天的预算", Self.at(20, day: -1), answer: "预算复核"))
        try store.save(conversation("上周的事", Self.at(9, day: -6), answer: "旧事"))
        let model = AIPageModel(agenda: FakeAgenda(), channel: { nil }, agent: FakeChatAgent(), store: store,
                                now: { Self.noon }, remoteAllowed: { false }, skills: [], mediaList: { [] })
        let groups = AIHistoryGroups.make(model.history, now: Self.noon)
        #expect(groups.groups.map(\.title) == ["今天", "昨天", "更早"])
        let first = try #require(groups.groups.first?.items.first)
        #expect(first.preview == "上次 〔记录〕 定了 85 万。 一 二 〔记录〕")
        #expect(first.recordCount == 2)
        #expect(AIHistoryGroups.make(model.history, now: Self.noon, query: "预算").groups.flatMap(\.items).map(\.title) == ["昨天的预算"])
        #expect(AIHistoryGroups.make(model.history, now: Self.noon, query: "85 万").groups.flatMap(\.items).map(\.title) == ["会前要点"])

        model.rename(first.id, to: "  和同事对报价  ")
        #expect(model.history.first { $0.id == first.id }?.title == "和同事对报价")
        model.rename(first.id, to: "   ")
        #expect(model.history.first { $0.id == first.id }?.title == "和同事对报价", "an empty name is ignored")

        // Opening one restores its cards and the labels of the records it cited.
        await model.open(try #require(model.history.first { $0.id == first.id }))
        #expect(model.conversation.messages.last?.actions.first?.summary == "存到备注")
        #expect(model.recordLabel("e1") == "录音")
        await model.deleteConversation(first.id)
        #expect(model.conversation.messages.isEmpty && !model.history.contains { $0.id == first.id })
        #expect(AIConversationStore(directory: folder).list().count == 2)
        try? FileManager.default.removeItem(at: folder)
    }

    // MARK: Pi end to end

    @Test func piProposalsAndWritesShowAsCardsThatApproveAndUndo() async throws {
        guard let node = PiAgentTests.nodeForTests, FileManager.default.fileExists(atPath: PiAgentTests.extensionDir.appendingPathComponent("node_modules/@earendil-works/pi-coding-agent/package.json").path) else {
            Issue.record("跳过：没有 Node 或未在 agents/pi-extension 运行 npm ci")
            return
        }
        let root = Self.folder("pi")
        let channel = try await TestCore.ready(root, cutover: false).channel!
        let (mock, port) = try PiAgentTests.startMockModel(node)
        defer { mock.terminate(); TestCore.finish(root) }
        let source = try await post(channel, "/local/sources", ["kind": "mac.region", "display_name": "合成笔记"])["source_id"] as! String
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        _ = try await post(channel, "/local/ingest", ["source_id": source, "events": [[
            "source_key": "n1", "kind": "screen.text", "start_ms": now - 60_000, "time_basis": "observed", "actor": "self", "sensitivity": "normal",
            "payload_version": 1, "payload": [:], "blobs": [], "app_bundle_id": "com.example.notes", "app_name": "Notes", "title": "季度预算",
            "text": [["field": "body", "content": "【合成样例】季度预算需要复核", "visible": true, "focus_distance": 0]],
        ]]])
        let hits = try await post(channel, "/v1/search", ["q": "预算"])["hits"] as? [[String: Any]] ?? []
        let eventID = try #require(hits.first?["event_id"] as? String)
        _ = try await post(channel, "/local/models", ["action": "save", "config": [
            "version": 1,
            "providers": [["provider_id": "mock", "name": "模拟模型", "api": "openai-chat", "base_url": "http://127.0.0.1:\(port)/v1", "destination": "on_device", "enabled": true, "headers_json": [:]]],
            "models": [["provider_id": "mock", "model_id": "mock-agent"]],
            "roles": [["role": "agent_chat", "model": ["provider_id": "mock", "model_id": "mock-agent"]]],
        ]])
        let pi = PiAgent(channel: { channel }, install: PiInstall(node: node, home: PiAgentTests.extensionDir),
                         directory: root.appendingPathComponent("pi", isDirectory: true), prepare: nil)
        let agenda = FakeAgenda()
        let model = AIPageModel(agenda: agenda, channel: { channel }, agent: pi, store: AIConversationStore(directory: root.appendingPathComponent("chats")),
                                remoteAllowed: { false }, skills: AISkill.load(from: [Self.skillsFolder]), mediaList: { [] })
        func turn(_ text: String) async throws {
            model.draft = text
            await model.send()
            let deadline = ContinuousClock.now.advanced(by: .seconds(60))
            while model.turnRunning {
                guard ContinuousClock.now < deadline else { Issue.record("没有等到这一轮结束"); return }
                try await Task.sleep(for: .milliseconds(20))
            }
        }

        // A proposal: a card that waits for 批准, then the system write and its receipt.
        try await turn("[提醒] 记一下交报销")
        let answer = try #require(model.conversation.messages.last)
        #expect(answer.text == "已提出提醒，等你批准。")
        let card = try #require(answer.actions.first)
        #expect(card.kind == .proposal && card.status == "pending" && card.summary == "加到提醒事项：【合成】交差旅报销")
        await model.refreshCore()
        let proposal = try #require(model.proposal(card.id))
        #expect(proposal.fromPi && proposal.status == "pending")
        await model.approve(proposal)
        #expect(agenda.written.first?.draft.title == "【合成】交差旅报销")
        #expect(model.proposal(card.id)?.status == "executed")
        #expect(model.conversation.messages.last?.actions.first?.status == "executed", "the chat card follows the proposal")

        // A write inside the store: done at once, undone by the App.
        try await turn("[备注:\(eventID)] 存一下要点")
        let write = try #require(model.conversation.messages.last?.actions.first)
        #expect(write.kind == .write && write.status == "done" && write.summary == "存到备注：【合成】会前要点")
        let annotated = try await post(channel, "/local/records", ["action": "annotation_get", "event_ids": [eventID]])
        #expect((annotated["annotation"] as? [String: Any])?["notes"] as? String == "【合成】会前要点")
        await model.undo(write)
        #expect(model.conversation.messages.last?.actions.first?.status == "undone")
        let restored = try await post(channel, "/local/records", ["action": "annotation_get", "event_ids": [eventID]])
        #expect((restored["annotation"] as? [String: Any])?["notes"] as? String == nil)

        // A cited record: the steps line names it, and the answer's [[id]] becomes a chip labelled with it.
        try await turn("[引用] 预算在哪")
        let cited = try #require(model.conversation.messages.last)
        #expect(cited.calls?.first?.tool == "pv_search" && cited.calls?.first?.state == .done)
        #expect(cited.stepsSummary?.hasPrefix("查了 1 条记录 · 屏幕 ") == true)
        #expect(cited.text.contains("[[\(eventID)]]"))
        #expect(AIMarkdown.citations(cited.text, label: model.recordLabel).contains("[" + model.recordLabel(eventID) + "](\(AIMarkdown.recordScheme)://\(eventID))"))
        #expect(model.recordLabel(eventID).hasPrefix("屏幕 "))

        // An edit of a system reminder brought in as context: a proposal that updates it once approved.
        agenda.reminderItems = [AgendaReminder(id: "r-ctx", title: "【合成】交差旅报销", due: Date(), listID: "l", list: "提醒")]
        model.ask(AIContextItem(id: "reminder:r-ctx", kind: "提醒", title: "【合成】交差旅报销"))
        try await turn("[改提醒:reminder:r-ctx] 改个名字")
        let edit = try #require(model.conversation.messages.last?.actions.first)
        #expect(edit.kind == .proposal && edit.summary.hasPrefix("修改提醒"))
        await model.refreshCore()
        await model.approve(try #require(model.proposal(edit.id)))
        #expect(agenda.reminderItems.first?.title == "【合成】交差旅报销（改）")
        #expect(model.proposal(edit.id)?.result?.receipt?["item_id"] == "r-ctx")

        // 回顾今天 (it did nothing before: the core's recording-only review failed silently): Pi runs the 日回顾 skill, saves
        // a day draft, and the 回顾 card shows it.
        #expect(model.review == nil)
        await model.reviewToday()
        #expect(model.isRunning(.skill("daily-review")))
        let reviewDeadline = ContinuousClock.now.advanced(by: .seconds(60))
        while model.turnRunning { guard ContinuousClock.now < reviewDeadline else { Issue.record("回顾没有结束"); break }; try await Task.sleep(for: .milliseconds(20)) }
        await model.refreshCore()
        #expect(model.review?.text == "【合成回顾】今天复核了季度预算 [[\(eventID)]]。")
        #expect(model.review?.producer == "pi")
        #expect(model.conversation.messages.last?.actions.first?.summary == "保存回顾草稿")

        // 取消 really stops a run: a slow answer is abandoned and nothing more arrives.
        model.draft = "[慢] 慢一点"
        await model.send()
        try await Task.sleep(for: .milliseconds(400))
        let started = ContinuousClock.now
        model.cancelRun()
        while model.turnRunning { guard ContinuousClock.now - started < .seconds(4) else { Issue.record("取消没有停下"); break }; try await Task.sleep(for: .milliseconds(20)) }
        #expect(model.conversation.messages.last?.stopped == true || model.conversation.messages.last?.role == .user)

        #expect(AIConversationStore(directory: root.appendingPathComponent("chats")).list().first?.messages.count ?? 0 >= 10)
        await model.newConversation()
    }
}

/// Renders the AI page with synthetic data at the split and the narrow width, in light and dark, so the layout can be
/// looked at without running the App (`CANDID_RENDER_DIR`, default a temporary folder).
@MainActor @Test func aiPageRendersBothLayouts() async throws {
    let root = AIPageTests.folder("render")
    let channel = try await TestCore.ready(root, cutover: false).channel!
    defer { TestCore.finish(root) }
    let out = URL(fileURLWithPath: ProcessInfo.processInfo.environment["CANDID_RENDER_DIR"] ?? root.appendingPathComponent("png").path)
    try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
    var proposalIDs: [String] = []
    for (kind, payload) in [("create_reminder", ["title": "【合成】把预算周会结论发给同事", "due": "2030-01-02T18:00:00+08:00"]),
                            ("create_calendar_event", ["title": "【合成】预算复核", "start": "2030-01-03T14:00:00+08:00"])] {
        let data = try await channel.send("POST", "/v1/propose", body: JSONSerialization.data(withJSONObject: ["kind": kind, "payload": payload])).checked()
        proposalIDs.append((try JSONSerialization.jsonObject(with: data) as? [String: Any])?["proposal_id"] as? String ?? "")
    }
    let agenda = FakeAgenda()
    agenda.events = [AIPageTests.event("站会", AIPageTests.at(9, 30), AIPageTests.at(9, 45)),
                     AIPageTests.event("和同事对报价", AIPageTests.at(14), AIPageTests.at(14, 30)),
                     AIPageTests.event("产品评审", AIPageTests.at(16, 30), AIPageTests.at(17, 30))]
    agenda.reminderItems = [AgendaReminder(id: "r1", title: "【合成】交差旅报销", due: AIPageTests.at(18), listID: "l", list: "提醒"),
                            AgendaReminder(id: "r2", title: "【合成】回复例会时间", due: AIPageTests.at(9, day: -1), listID: "l", list: "提醒"),
                            AgendaReminder(id: "r3", title: "明天的", due: AIPageTests.at(9, day: 1), listID: "l", list: "提醒")]
    let agent = FakeChatAgent()
    let wrap = { (json: String) in "<candid_data tool=\"x\">\n\(json)\n</candid_data>" }
    let rec = "0190aaaa-1111-7222-8333-444455556666"
    agent.script = [.toolStarted(id: "a", name: "pv_search", arguments: #"{"q":"预算周会"}"#),
                    .toolFinished(id: "a", name: "pv_search", result: wrap(#"{"hits":[{"event_id":"\#(rec)","kind":"speech.segment","start":"2030-01-01T15:30:00+08:00","title":"预算周会"},{"event_id":"m1","kind":"mail.message","start":"2030-01-01T22:10:00+08:00"},{"event_id":"m2","kind":"mail.message"}]}"#), isError: false),
                    .toolStarted(id: "b", name: "pv_annotate", arguments: #"{"event_ids":["e"],"notes":"会前要点"}"#),
                    .toolFinished(id: "b", name: "pv_annotate", result: wrap(#"{"write_id":"annotation:agent-x"}"#), isError: false),
                    .toolStarted(id: "c", name: "pv_propose", arguments: #"{"kind":"create_reminder","payload":{"title":"【合成】把预算周会结论发给同事"}}"#),
                    .toolFinished(id: "c", name: "pv_propose", result: wrap(#"{"proposal_id":"\#(proposalIDs[0])","status":"pending"}"#), isError: false),
                    .assistantMessage("上次 [[\(rec)]] 定了 **Q4 按 85 万上报**，对方答应周五前给报价单。之后：\n\n1. 报价单初稿硬件比上次高 6 万 [[m1]]\n2. 设备采购还在等回复，没有新消息\n\n可以先问：`硬件为什么涨`、能不能拆到 Q1。"),
                    .settled(aborted: false)]
    let model = AIPageModel(agenda: agenda, channel: { channel }, agent: agent, store: AIConversationStore(directory: root.appendingPathComponent("chats")),
                            now: { AIPageTests.noon }, remoteAllowed: { false }, skills: AISkill.load(from: [AIPageTests.skillsFolder]), mediaList: { [] })
    await model.refresh()
    model.ask(AIContextItem(id: "event:x", kind: "日程", title: "14:00 和同事对报价"))
    model.draft = "会前要点"
    await model.send()
    for (title, days) in [("昨天：预算周会结论", -1), ("上周：设备采购", -6)] {
        var past = AIConversation(title: title, created: AIPageTests.at(10, day: days), updated: AIPageTests.at(10, day: days))
        past.messages = [AIChatMessage(role: .user, text: title), AIChatMessage(role: .assistant, text: "【合成】整理好了 [[\(rec)]]，**三条**要点。")]
        try AIConversationStore(directory: root.appendingPathComponent("chats")).save(past)
    }
    await model.newConversation()
    await model.open(model.history.first { $0.title.hasPrefix("14:00") } ?? model.history[0])
    _ = NSApplication.shared
    let session = DesktopSession(dataRoot: root.appendingPathComponent("session"))
    for (name, width) in [("split", 1180.0), ("narrow", 760.0), ("skills", 1180.0), ("history", 1180.0)] {
        for dark in [false, true] {
            let content = AIHomeView(session: session, model: model, showingSkills: name == "skills", showingHistory: name == "history")
                .frame(width: width, height: 820)
                .environment(\.colorScheme, dark ? .dark : .light)
                .background(dark ? Color.black : Color(white: 0.95))
            let view = NSHostingView(rootView: content)
            view.frame = NSRect(x: 0, y: 0, width: width, height: 820)
            let window = NSWindow(contentRect: view.frame, styleMask: [.borderless], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            window.contentView = view; view.layoutSubtreeIfNeeded()
            try await Task.sleep(for: .milliseconds(150))
            view.layoutSubtreeIfNeeded()
            let bitmap = try #require(view.bitmapImageRepForCachingDisplay(in: view.bounds))
            view.cacheDisplay(in: view.bounds, to: bitmap)
            let png = try #require(bitmap.representation(using: .png, properties: [:]))
            #expect(png.count > 10_000)
            try png.write(to: out.appendingPathComponent("ai-\(name)-\(dark ? "dark" : "light").png"))
            window.close()
        }
    }
    #expect(model.isSplit, "the last render was wide")
}
