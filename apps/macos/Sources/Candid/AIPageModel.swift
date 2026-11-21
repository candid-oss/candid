import AIProviders
import CandidLocalization
import Foundation
import Observation
import CandidClient

// The AI page (agent-platform-v1 §4.2 ai-settings, §4.3 commitments): the 「今天」 column (system calendar and
// Reminders through `SystemAgenda`, proposals, commitment candidates, speakers to confirm, the latest day review, jobs)
// and the chat with Pi. Writes outside the App happen only here, after the user approves a proposal: the core records
// the approval first, then EventKit writes, then the receipt (or the failure) goes back onto the proposal.

// MARK: Core shapes

struct AgendaProposal: Decodable, Identifiable, Equatable, Sendable {
    struct Payload: Decodable, Equatable, Sendable {
        var title: String?
        var due: String?
        var start: String?
        var end: String?
        var notes: String?
        var memory_id: String?
        /// The system item an update, complete or delete acts on.
        var item_id: String?
    }
    struct Outcome: Decodable, Equatable, Sendable {
        var receipt: [String: String]?
        var error: String?
    }
    var proposal_id: String
    var client_id: String
    var client_name: String
    var kind: String
    var payload: Payload
    var evidence: [String]
    var status: String
    var created: String
    var decided: String?
    var executed: String?
    var result: Outcome?

    var id: String { proposal_id }
    var title: String { payload.title ?? payload.item_id.map { _ in L("已有的一条") } ?? L("提案") }
    static let executableKinds = ["create_reminder", "update_reminder", "complete_reminder", "delete_reminder",
                                  "create_calendar_event", "update_calendar_event", "delete_calendar_event"]
    /// The App knows how to carry these out (§4.1 propose kinds).
    var executable: Bool { Self.executableKinds.contains(kind) }
    var isDelete: Bool { kind.hasPrefix("delete_") }
    static func label(_ kind: String) -> String {
        ["create_reminder": L("加到提醒事项"), "update_reminder": L("修改提醒"), "complete_reminder": L("完成提醒"), "delete_reminder": L("删除提醒"),
         "create_calendar_event": L("加到日历"), "update_calendar_event": L("修改日程"), "delete_calendar_event": L("删除日程")][kind] ?? kind
    }
    /// Waits for the user: pending, or failed (retry or reject), or approved but never finished.
    var open: Bool { ["pending", "approved", "failed"].contains(status) }
    var what: String { Self.label(kind) }
    var fromPi: Bool { client_id == "candid-pi" }
}

struct AgendaCommitment: Decodable, Identifiable, Equatable, Sendable {
    struct Source: Decodable, Equatable, Sendable { var event_id: String; var kind: String; var start: String?; var title: String? }
    var memory_id: String
    var text: String
    var due: String?
    var producer: String
    var confidence: Double?
    var source: Source?
    var id: String { memory_id }
}

/// A date, reminder or promise the core found in a conversation and the user has not decided on yet.
struct AgendaItem: Decodable, Identifiable, Equatable, Sendable {
    struct Person: Decodable, Equatable, Sendable { var name: String?; var key: String? }
    struct Conversation: Decodable, Equatable, Sendable { var title: String?; var app: String? }
    var item_id: String
    /// `event`, `reminder` or `commitment`.
    var kind: String
    /// `self` (I promised) or `other` (waiting for them).
    var owner: String
    var title: String
    var when_text: String?
    var start: String?
    var end: String?
    var all_day: Bool?
    var person: Person?
    var conversation: Conversation?
    var id: String { item_id }
    var isEvent: Bool { kind == "event" }
    /// 「张三 · 周六下午三点」 style second line: who, when (their words), where it was said.
    var origin: String {
        [person?.name ?? conversation?.title, when_text ?? AgendaDate.parse(start).map { AITodayColumn.dayTime($0) }, owner == "other" ? L("等对方") : nil]
            .compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " · ")
    }
}

/// A fact about a person or a matter that came up in a conversation (`memories(kind=fact)`, candidate).
struct AgendaMemo: Decodable, Identifiable, Equatable, Sendable {
    var memory_id: String
    var text: String
    var person: AgendaItem.Person?
    var id: String { memory_id }
}

struct AgendaReview: Decodable, Equatable, Sendable {
    var digest_id: String
    var status: String
    var producer: String
    var start: String
    var created: String
    var text: String
}

struct AgendaSnapshot: Decodable, Sendable {
    var proposals: [AgendaProposal]
    var commitments: [AgendaCommitment]
    var review: AgendaReview?
    var items: [AgendaItem]?
    var memos: [AgendaMemo]?
}

/// A speaker the voice library thinks might be someone (「说话人 4 可能是李娜 · 68%」).
struct SpeakerToConfirm: Identifiable, Equatable, Sendable {
    var recordingID: UUID
    var recording: String
    var at: Date?
    var key: String
    var name: String
    var voiceID: String
    var percent: Int?
    var id: String { recordingID.uuidString + "|" + key }
}

enum AgendaDate {
    /// The core's ISO times, Pi's payload times (with or without seconds/fraction) and plain dates.
    static func parse(_ text: String?) -> Date? {
        guard let text, !text.isEmpty else { return nil }
        let full = ISO8601DateFormatter()
        full.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let d = full.date(from: text) { return d }
        full.formatOptions = [.withInternetDateTime]
        if let d = full.date(from: text) { return d }
        for format in ["yyyy-MM-dd'T'HH:mm:ss", "yyyy-MM-dd'T'HH:mm", "yyyy-MM-dd HH:mm", "yyyy-MM-dd"] {
            let f = DateFormatter()
            f.locale = Locale(identifier: "en_US_POSIX")
            f.timeZone = .current
            f.dateFormat = format
            if let d = f.date(from: text) { return d }
        }
        return nil
    }
}

// MARK: Layout

enum AIPageTab: String, Hashable, Sendable { case today, chat }

enum AIPageLayout {
    /// Narrower than this, the two columns become the 「今天 / 对话」 switch (no drawer).
    static let splitWidth: CGFloat = 900
    static let todayWidth: CGFloat = 300
    static func isSplit(_ width: CGFloat) -> Bool { width >= splitWidth }
}

// MARK: Chat agent

/// What the page needs from the attached agent; `PiAgent` in the App, a scripted fake in unit tests.
@MainActor protocol AIChatAgent: AnyObject {
    var onEvent: ((PiEvent) -> Void)? { get set }
    var isRunning: Bool { get }
    func start(_ config: PiRunConfig) async throws
    func prompt(_ text: String) throws
    func cancel()
    func stop() async
}

extension PiAgent: AIChatAgent {
    var isRunning: Bool { run != nil }
}

/// What started a turn, so the control that started it can show progress and 取消.
enum AIRun: Equatable, Sendable {
    case chat
    case skill(String)
    /// 会前要点 for an event (the context item id).
    case prepare(String)
    var key: String {
        switch self { case .chat: "chat"; case .skill(let id): "skill:" + id; case .prepare(let id): "prepare:" + id }
    }
}

// MARK: Model

@MainActor @Observable final class AIPageModel {
    // Today
    private(set) var calendarAccess: AgendaAccess = .notDetermined
    private(set) var remindersAccess: AgendaAccess = .notDetermined
    private(set) var day = AgendaDay(allDay: [], rows: [])
    private(set) var reminders = ReminderGroups(items: [], tomorrow: 0)
    private(set) var proposals: [AgendaProposal] = []
    private(set) var commitments: [AgendaCommitment] = []
    /// Dates, reminders and promises found in conversations, waiting for a decision (待你处理: 日程、提醒), and memos (备忘).
    private(set) var items: [AgendaItem] = []
    private(set) var memos: [AgendaMemo] = []
    private(set) var speakers: [SpeakerToConfirm] = []
    private(set) var review: AgendaReview?
    private(set) var loading = false
    /// Per item (`proposal:<id>`, `commitment:<id>`, `speaker:<id>`, `agenda`, `core`): what went wrong there.
    var problems: [String: String] = [:]
    private(set) var working: Set<String> = []

    // Layout
    var tab: AIPageTab = .today
    /// The chat answered while the narrow layout showed 「今天」.
    private(set) var chatUnread = false
    var isSplit = true

    // Chat
    private(set) var conversation = AIConversation()
    private(set) var history: [AIConversation] = []
    var draft = ""
    private(set) var context: [AIContextItem] = []
    private(set) var turnRunning = false
    private(set) var agentProblem: String?

    let workflows: WorkflowControlModel
    /// Skills Pi can run (agents/skills).
    private let skillDefinitions: [AISkill]
    let automation: AutomationModel
    var skills: [AISkill] {
        skillDefinitions.map { original in
            var skill = original
            if let definition = automation.catalog.first(where: { $0.id == skill.id }) { skill.title = definition.title }
            if let policy = automation.config.skills[skill.id] {
                switch policy.trigger {
                case "daily": skill.trigger = .timed(policy.time)
                case "recording_sealed": skill.trigger = .event(L("录音保存后"))
                case "episode_ended": skill.trigger = .event(L("片段结束后"))
                default: skill.trigger = .manual
                }
                skill.enabled = policy.enabled && policy.automatic && automation.passed
            }
            return skill
        }
    }
    func canRunSkill(_ id: String) -> Bool { automation.config.skills[id]?.enabled ?? true }
    func skillStatus(_ skill: AISkill) -> String {
        if !canRunSkill(skill.id) { return L("已关闭") }
        if automation.config.skills[skill.id]?.automatic == true { return automation.passed ? L("自动运行已开") : L("自动运行等待评估") }
        return L("手动可用")
    }
    /// What the running (or last) turn was started by.
    private(set) var run: AIRun?
    /// A skill waiting for the user's input in the chat before it runs.
    var pendingSkill: AISkill?
    private let agenda: SystemAgenda
    private let channel: () -> LocalChannel?
    private let agent: AIChatAgent
    private let store: AIConversationStore
    private let now: () -> Date
    private let excludedCalendars: () -> Set<String>
    private let remoteAllowed: @MainActor () -> Bool
    private let mediaList: () async throws -> [MediaLibraryRecord]
    private let confirmSpeaker: (SpeakerToConfirm) async throws -> Void
    private var finished: [String] = []
    private var streaming = ""
    private var skillRun: AutomationRun?
    private var skillError: String?
    private var skillHeartbeat: Task<Void, Never>?
    private var pendingArguments: [String: String] = [:]
    /// Records Pi has looked at in this conversation, for the chips in its answers.
    private(set) var records: [String: AIRecordRef] = [:]
    /// Suggestions for 「@」 in the input.
    private(set) var mentions: [AIRecordRef] = []
    private var mentionTask: Task<Void, Never>?
    /// Opens the timeline at a record's time (set by the page).
    var showInTimeline: ((Date) -> Void)?

    init(agenda: SystemAgenda = EventKitAgenda(),
         channel: @escaping () -> LocalChannel? = { TimelineService.shared.channel },
         agent: AIChatAgent? = nil,
         store: AIConversationStore = .standard,
         workflows: WorkflowControlModel? = nil,
         now: @escaping () -> Date = Date.init,
         excludedCalendars: @escaping () -> Set<String> = { Set(UserDefaults.standard.stringArray(forKey: "ai.agenda.excludedCalendars") ?? []) },
         remoteAllowed: @escaping @MainActor () -> Bool = { AIPageModel.qaAgentIsRemote() },
         skills: [AISkill]? = nil,
         mediaList: (() async throws -> [MediaLibraryRecord])? = nil,
         confirmSpeaker: ((SpeakerToConfirm) async throws -> Void)? = nil) {
        self.agenda = agenda
        self.channel = channel
        self.agent = agent ?? PiAgent(channel: channel)
        self.store = store
        self.workflows = workflows ?? WorkflowControlModel()
        self.now = now
        self.excludedCalendars = excludedCalendars
        self.remoteAllowed = remoteAllowed
        self.mediaList = mediaList ?? {
            guard let c = channel() else { return [] }
            return try await MediaLibraryClient(channel: c).list(query: "")
        }
        self.confirmSpeaker = confirmSpeaker ?? { speaker in
            guard let c = channel() else { throw PiAgentError.noCore }
            _ = try await MediaLibraryClient(channel: c).labelSpeaker(speaker.recordingID, speaker: speaker.key, as: .voice(speaker.voiceID, name: speaker.name))
        }
        self.automation = AutomationModel(channel: channel)
        self.skillDefinitions = skills ?? AISkill.load(from: AISkill.folders(for: .resolved()))
        self.agent.onEvent = { [weak self] in self?.receive($0) }
        history = store.list()
    }

    /// The `agent_chat` role's destination: a cloud or self-hosted model may be used because the user chose it for the role.
    static func qaAgentIsRemote() -> Bool { (qaAgentDestination() ?? .onDevice) != .onDevice }

    static func qaAgentDestination() -> Destination? {
        let config = AIRuntime.shared.config()
        return config.assignment("agent_chat")?.model.flatMap { config.destination(of: $0) }
    }

    // MARK: Today

    /// Everything the left column shows. System calendar items are read fresh, never stored.
    func refresh() async {
        loading = true
        defer { loading = false }
        async let system: Void = refreshSystem()
        async let core: Void = refreshCore()
        async let people: Void = refreshSpeakers()
        _ = await (system, core, people)
        await workflows.refresh()
    }

    func refreshSystem() async {
        calendarAccess = agenda.access(.calendar)
        remindersAccess = agenda.access(.reminders)
        let today = Calendar.current.startOfDay(for: now())
        let tomorrow = Calendar.current.date(byAdding: .day, value: 1, to: today)!
        let after = Calendar.current.date(byAdding: .day, value: 2, to: today)!
        let excluded = excludedCalendars()
        do {
            let events = calendarAccess == .granted ? try await agenda.events(from: today, to: tomorrow, excluding: excluded) : []
            day = AgendaDay.make(events, now: now())
            let due = remindersAccess == .granted ? try await agenda.reminders(dueBefore: after, excluding: excluded) : []
            reminders = ReminderGroups.make(due, now: now())
            problems["agenda"] = nil
        } catch {
            problems["agenda"] = L("读取系统日历失败：%@", error.localizedDescription)
        }
    }

    /// Asks for calendar or Reminders access once the user taps 「允许」 next to the card.
    func requestAccess(_ kind: AgendaKind) async {
        _ = await agenda.requestAccess(kind)
        await refreshSystem()
    }

    func refreshCore() async {
        await automation.refresh(includeRuns: false)
        guard let channel = channel() else { problems["core"] = L("时间线库尚未打开"); return }
        do {
            let since = Int64(Calendar.current.startOfDay(for: now()).timeIntervalSince1970 * 1000)
            let data = try await channel.send("POST", "/local/agenda", body: JSONSerialization.data(withJSONObject: ["action": "today", "since_ms": since])).checked()
            let snapshot = try JSONDecoder().decode(AgendaSnapshot.self, from: data)
            proposals = snapshot.proposals
            commitments = snapshot.commitments
            items = snapshot.items ?? []
            memos = snapshot.memos ?? []
            review = snapshot.review
            problems["core"] = nil
            syncActionCards()
        } catch {
            problems["core"] = L("读取待处理事项失败：%@", error.localizedDescription)
        }
    }

    func refreshSpeakers() async {
        guard let records = try? await mediaList() else { return }
        let since = Calendar.current.date(byAdding: .day, value: -7, to: now()) ?? now()
        speakers = records.filter { ($0.occurredAt ?? .distantPast) >= since }.flatMap { record in
            (record.speakers ?? []).filter { $0.status == .maybe && $0.dismissed != true }.compactMap { s -> SpeakerToConfirm? in
                guard let name = s.name, let voice = s.voiceId else { return nil }
                return SpeakerToConfirm(recordingID: record.id, recording: record.displayTitle, at: record.occurredAt, key: s.key, name: name, voiceID: voice, percent: s.percent)
            }
        }
    }

    /// The page's clock (fixed in tests).
    var currentTime: Date { now() }

    /// Proposals that wait for the user (待你处理).
    var waiting: [AgendaProposal] { proposals.filter { $0.open && !($0.payload.memory_id != nil && $0.status == "approved") } }
    var waitingCount: Int { waiting.count + speakers.count + items.count + memos.count }
    var eventItems: [AgendaItem] { items.filter(\.isEvent) }
    var reminderItems: [AgendaItem] { items.filter { !$0.isEvent } }
    /// Jobs still running or failed, for the footer line.
    var activeJobs: [WorkflowControlModel.Job] { workflows.jobs.filter { ["queued", "running"].contains($0.state) } }
    var failedJobs: [WorkflowControlModel.Job] { workflows.jobs.filter { $0.state == "failed" } }

    // MARK: Decisions

    private func agendaCall(_ body: [String: Any]) async throws -> Data {
        guard let channel = channel() else { throw PiAgentError.noCore }
        return try await channel.send("POST", "/local/agenda", body: JSONSerialization.data(withJSONObject: body)).checked()
    }

    private func decide(_ id: String, _ decision: String, extra: [String: Any] = [:]) async throws -> AgendaProposal {
        var body: [String: Any] = ["action": "proposal", "proposal_id": id, "decision": decision]
        body.merge(extra) { $1 }
        return try JSONDecoder().decode(AgendaProposal.self, from: try await agendaCall(body))
    }

    private func replace(_ proposal: AgendaProposal) {
        if let i = proposals.firstIndex(where: { $0.id == proposal.id }) { proposals[i] = proposal } else { proposals.insert(proposal, at: 0) }
        syncActionCards()
    }

    /// 批准: the core records the approval, EventKit writes, the receipt (or the failure) goes back onto the proposal.
    func approve(_ proposal: AgendaProposal) async {
        let key = "proposal:" + proposal.id
        guard !working.contains(key) else { return }
        working.insert(key); defer { working.remove(key) }
        problems[key] = nil
        let approved: AgendaProposal
        do { approved = try await decide(proposal.id, "approve") } catch {
            problems[key] = L("没能批准：%@", error.localizedDescription)
            return
        }
        replace(approved)
        do {
            let receipt = try await execute(proposal)
            replace(try await decide(proposal.id, "executed", extra: ["receipt": receipt.json]))
        } catch {
            let message = error.localizedDescription
            problems[key] = message
            if let failed = try? await decide(proposal.id, "failed", extra: ["error": message]) { replace(failed) }
        }
        await refreshSystem()
    }

    /// Carries out an approved proposal through EventKit.
    func execute(_ proposal: AgendaProposal) async throws -> AgendaReceipt {
        let p = proposal.payload
        let draft = AgendaDraft(title: p.title ?? "", due: AgendaDate.parse(p.due), start: AgendaDate.parse(p.start), end: AgendaDate.parse(p.end), notes: p.notes)
        func item() throws -> String {
            guard let id = p.item_id, !id.isEmpty else { throw AgendaWriteError.incomplete(L("要改的那一条（item_id）")) }
            // Pi may echo the context reference as `reminder:<id>` / `event:<id>`.
            for prefix in ["reminder:", "event:"] where id.hasPrefix(prefix) { return String(id.dropFirst(prefix.count)) }
            return id
        }
        switch proposal.kind {
        case "create_reminder":
            guard !draft.title.isEmpty else { throw AgendaWriteError.incomplete(L("标题")) }
            return try await agenda.createReminder(draft)
        case "create_calendar_event":
            guard !draft.title.isEmpty else { throw AgendaWriteError.incomplete(L("标题")) }
            return try await agenda.createEvent(draft)
        case "update_reminder": return try await agenda.updateReminder(try item(), draft)
        case "complete_reminder": return try await agenda.completeReminder(try item())
        case "delete_reminder": return try await agenda.deleteReminder(try item())
        case "update_calendar_event": return try await agenda.updateEvent(try item(), draft)
        case "delete_calendar_event": return try await agenda.deleteEvent(try item())
        default: throw AgendaWriteError.incomplete(L("App 能执行的种类"))
        }
    }

    func reject(_ proposal: AgendaProposal) async {
        let key = "proposal:" + proposal.id
        working.insert(key); defer { working.remove(key) }
        do { replace(try await decide(proposal.id, "reject")); problems[key] = nil } catch { problems[key] = L("没能拒绝：%@", error.localizedDescription) }
    }

    /// 加到提醒: confirming the candidate files a `create_reminder` proposal, which this click also approves.
    func addToReminders(_ commitment: AgendaCommitment) async {
        let key = "commitment:" + commitment.id
        working.insert(key); defer { working.remove(key) }
        do {
            struct Answer: Decodable { var proposal: AgendaProposal? }
            let answer = try JSONDecoder().decode(Answer.self, from: try await agendaCall(["action": "commitment", "memory_id": commitment.id, "decision": "confirm"]))
            commitments.removeAll { $0.id == commitment.id }
            problems[key] = nil
            if let proposal = answer.proposal {
                replace(proposal)
                await approve(proposal)
            }
        } catch { problems[key] = L("没能加到提醒：%@", error.localizedDescription) }
    }

    func ignore(_ commitment: AgendaCommitment) async {
        let key = "commitment:" + commitment.id
        working.insert(key); defer { working.remove(key) }
        do {
            _ = try await agendaCall(["action": "commitment", "memory_id": commitment.id, "decision": "ignore"])
            commitments.removeAll { $0.id == commitment.id }
            problems[key] = nil
        } catch { problems[key] = L("没能忽略：%@", error.localizedDescription) }
    }

    /// 加到日历 / 加到提醒: confirming files the matching create proposal, which this click also approves (and EventKit writes).
    func add(_ item: AgendaItem) async {
        let key = "item:" + item.id
        guard !working.contains(key) else { return }
        working.insert(key); defer { working.remove(key) }
        do {
            struct Answer: Decodable { var proposal: AgendaProposal? }
            let answer = try JSONDecoder().decode(Answer.self, from: try await agendaCall(["action": "item", "item_id": item.id, "decision": "confirm"]))
            items.removeAll { $0.id == item.id }
            problems[key] = nil
            if let proposal = answer.proposal {
                replace(proposal)
                await approve(proposal)
            }
        } catch { problems[key] = L("没能加入：%@", error.localizedDescription) }
    }

    func ignore(_ item: AgendaItem) async {
        let key = "item:" + item.id
        working.insert(key); defer { working.remove(key) }
        do {
            _ = try await agendaCall(["action": "item", "item_id": item.id, "decision": "ignore"])
            items.removeAll { $0.id == item.id }
            problems[key] = nil
        } catch { problems[key] = L("没能忽略：%@", error.localizedDescription) }
    }

    /// 记下 / 忽略 a memo.
    func decide(_ memo: AgendaMemo, keep: Bool) async {
        let key = "memo:" + memo.id
        working.insert(key); defer { working.remove(key) }
        do {
            _ = try await agendaCall(["action": "memo", "memory_id": memo.id, "decision": keep ? "confirm" : "ignore"])
            memos.removeAll { $0.id == memo.id }
            problems[key] = nil
        } catch { problems[key] = L("没能处理这条备忘：%@", error.localizedDescription) }
    }

    func confirm(_ speaker: SpeakerToConfirm) async {
        let key = "speaker:" + speaker.id
        working.insert(key); defer { working.remove(key) }
        do {
            try await confirmSpeaker(speaker)
            speakers.removeAll { $0.id == speaker.id }
            problems[key] = nil
        } catch { problems[key] = L("没能确认：%@", error.localizedDescription) }
    }

    func cancel(_ job: WorkflowControlModel.Job) async { await workflows.command("cancel", job: job) }
    func retry(_ job: WorkflowControlModel.Job) async { await workflows.command("restart", job: job) }

    // MARK: Chat

    /// 「问 AI」: brings an item into the chat; in the narrow layout this switches to 「对话」.
    func ask(_ item: AIContextItem) {
        if !context.contains(item) { context.append(item) }
        if !isSplit { tab = .chat }
        chatUnread = false
    }

    func removeContext(_ item: AIContextItem) { context.removeAll { $0 == item } }

    func show(_ tab: AIPageTab) {
        self.tab = tab
        if tab == .chat { chatUnread = false }
    }

    /// What Pi receives: the context lines, then the user's words.
    static func prompt(_ text: String, context: [AIContextItem]) -> String {
        guard !context.isEmpty else { return text }
        return L("上下文（用户从 AI 页左栏带进来的）：\n") + context.map(\.promptLine).joined(separator: "\n") + "\n\n" + text
    }

    /// Pi gets a fresh permission scope each turn, so carry the visible conversation with it.
    /// History is bounded and quoted separately from the current request; old writes are not new instructions.
    static func turnPrompt(_ request: String, history: [AIChatMessage], now: Date) -> String {
        let clock = ISO8601DateFormatter()
        clock.timeZone = .current
        let time = L("当前本地时间：") + clock.string(from: now) + L("（") + TimeZone.current.identifier + L("）")
        var turns: [[String: String]] = []
        var remaining = 24000
        for message in history.reversed() where message.role == .user || message.role == .assistant {
            let context = message.context.map(\.promptLine).joined(separator: "\n")
            let text = context.isEmpty ? message.text : context + "\n" + message.text
            guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { continue }
            let kept = String(text.prefix(min(6000, remaining)))
            turns.append(["role": message.role == .user ? "user" : "assistant", "text": kept])
            remaining -= kept.count
            if remaining <= 0 || turns.count >= 20 { break }
        }
        guard !turns.isEmpty, let encoded = try? JSONSerialization.data(withJSONObject: Array(turns.reversed()), options: [.sortedKeys]),
              let text = String(data: encoded, encoding: .utf8) else { return request + "\n\n" + time }
        return time + L("\n以下 JSON 是本次对话的历史，帮助理解追问，不重复执行历史操作：\n<candid_conversation>\n") + text + L("\n</candid_conversation>\n\n用户当前请求：\n") + request
    }

    func send() async {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        if let skill = pendingSkill {
            pendingSkill = nil
            let items = context
            await submit(display: L("运行「%@」", skill.title) + (text.isEmpty ? "" : L("：") + text), context: items,
                         prompt: skill.prompt(today: now(), context: items, extra: text), run: .skill(skill.id))
            return
        }
        guard !text.isEmpty || !context.isEmpty else { return }
        let words = text.isEmpty ? L("帮我看看这些") : text
        await submit(display: words, context: context, prompt: Self.prompt(words, context: context), run: .chat)
    }

    /// Runs a skill through Pi. One that needs input (a recording, a topic) and has none waits in the input first.
    func runSkill(_ skill: AISkill) async {
        guard canRunSkill(skill.id) else { problems["run:" + AIRun.skill(skill.id).key] = L("技能已关闭，可在 AI 功能设置中开启"); return }
        if skill.input != nil && context.isEmpty {
            pendingSkill = skill
            show(.chat)
            return
        }
        await submit(display: L("运行「%@」", skill.title), context: context, prompt: skill.prompt(today: now(), context: context), run: .skill(skill.id))
    }

    /// 「回顾今天」: the 日回顾 skill through Pi; its draft lands in the 回顾 card.
    func reviewToday() async {
        guard let skill = skills.first(where: { $0.id == "daily-review" }) else {
            problems["run:" + AIRun.skill("daily-review").key] = L("找不到日回顾技能（agents/skills/daily-review）")
            return
        }
        await runSkill(skill)
    }

    /// 「会前要点」 for an event in the 日程 card.
    func prepare(_ event: AIContextItem) async {
        let words = L("会前要点")
        await submit(display: words, context: [event], prompt: Self.prompt(words + L("：查上次相关的会议、邮件和待办，列出这次要谈的几点。"), context: [event]), run: .prepare(event.id))
    }

    /// Stops the running turn (Pi aborts; nothing more is written).
    func cancelRun() { agent.cancel() }

    func isRunning(_ run: AIRun) -> Bool { turnRunning && self.run == run }

    private func submit(display words: String, context items: [AIContextItem], prompt: String, run: AIRun) async {
        guard !turnRunning else { problems["run:" + run.key] = L("上一轮还没结束"); return }
        problems["run:" + run.key] = nil
        self.run = run
        let preparedPrompt = Self.turnPrompt(prompt, history: conversation.messages, now: now())
        conversation.messages.append(AIChatMessage(role: .user, text: words, context: items))
        if conversation.title == "新对话" { conversation.title = String((items.first?.title ?? words).prefix(24)) }
        conversation.messages.append(AIChatMessage(role: .assistant, text: ""))
        draft = ""
        context = []
        turnRunning = true
        agentProblem = nil
        finished = []; streaming = ""
        do {
            await agent.stop()
            var config = PiRunConfig(writes: ["annotate", "reference", "commitment", "draft"], allowRemote: remoteAllowed())
            if case .skill(let id) = run, let channel = channel() {
                let refs = items.compactMap { item -> [String: String]? in
                    guard item.id.hasPrefix("record:") else { return nil }
                    return ["event_id": String(item.id.dropFirst(7))]
                }
                let queued: AutomationRun = try await automationPost(channel, ["action": "run", "skill": id, "executor": "foreground", "context": items.map(\.promptLine).joined(separator: "\n"), "references": refs])
                let claimed: AutomationRun? = try await automationPost(channel, ["action": "claim", "id": queued.id])
                guard let claimed else { throw PiAgentError.busy }
                skillRun = claimed; skillError = nil
                config.runID = claimed.id; config.automationID = claimed.id; config.attempt = claimed.attempt
                skillHeartbeat = Task { [weak self] in
                    while !Task.isCancelled {
                        try? await Task.sleep(for: .seconds(10))
                        guard !Task.isCancelled, let self else { return }
                        do { let _: AutomationAck = try await automationPost(channel, ["action": "heartbeat", "id": claimed.id, "attempt": claimed.attempt]) }
                        catch { self.agent.cancel(); return }
                    }
                }
            }
            try await agent.start(config)
            try agent.prompt(preparedPrompt)
        } catch {
            turnRunning = false
            dropEmptyAnswer()
            agentProblem = Self.explain(error)
            finishSkill(state: "failed", error: agentProblem)
            problems["run:" + run.key] = agentProblem
            conversation.messages.append(AIChatMessage(role: .note, text: agentProblem ?? ""))
        }
        persist()
    }

    func stopTurn() { agent.cancel() }

    func newConversation() async {
        if turnRunning { agent.cancel() }
        persist()
        await agent.stop()
        conversation = AIConversation()
        context = []
        turnRunning = false
        agentProblem = nil
        history = store.list()
    }

    func open(_ past: AIConversation) async {
        guard past.id != conversation.id else { return }
        await newConversation()
        conversation = past
        for r in past.messages.flatMap({ $0.calls ?? [] }).flatMap(\.records) { records[r.eventID] = r }
        syncActionCards()
    }

    /// Renames a conversation (the title is the first question until then). An empty name is refused.
    func rename(_ id: UUID, to title: String) {
        let name = title.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return }
        if conversation.id == id { conversation.title = String(name.prefix(60)); try? store.save(conversation) }
        else if var past = history.first(where: { $0.id == id }) { past.title = String(name.prefix(60)); try? store.save(past) }
        history = store.list()
    }

    /// Deletes a conversation from this Mac; deleting the open one starts a new one.
    func deleteConversation(_ id: UUID) async {
        store.delete(id)
        if conversation.id == id {
            if turnRunning { agent.cancel() }
            await agent.stop()
            conversation = AIConversation()
            turnRunning = false
        }
        history = store.list()
    }

    /// 撤销 on a write card: the App may undo any agent write (`/v1/undo_write` without a token).
    func undo(_ action: AIChatAction) async {
        let key = "write:" + action.id
        working.insert(key); defer { working.remove(key) }
        do {
            guard let channel = channel() else { throw PiAgentError.noCore }
            _ = try await channel.send("POST", "/v1/undo_write", body: JSONSerialization.data(withJSONObject: ["write_id": action.id])).checked()
            updateAction(action.id) { $0.status = "undone" }
            problems[key] = nil
            persist()
        } catch { problems[key] = L("没能撤销：%@", error.localizedDescription) }
    }

    /// What a chip says for a cited record.
    func recordLabel(_ id: String) -> String { records[id]?.label ?? L("记录") }

    /// The call running now, for the live line (「正在搜索「预算」…」).
    var liveCall: AIToolCall? { turnRunning ? conversation.messages.last?.calls?.last(where: { $0.state == .running }) : nil }

    /// A chip was tapped: open the timeline at the record's time (looked up in the core when not known yet).
    func openRecord(_ id: String) async {
        var start = AgendaDate.parse(records[id]?.start)
        if start == nil, let channel = channel(),
           let data = try? await channel.send("POST", "/v1/get_event", body: JSONSerialization.data(withJSONObject: ["event_id": id])).checked(),
           let refs = Optional(PiToolResult.records("<candid_data>" + (String(data: data, encoding: .utf8) ?? "") + "</candid_data>")),
           let ref = refs.first(where: { $0.eventID == id }) {
            records[id] = ref
            start = AgendaDate.parse(ref.start)
        }
        guard let start else { problems["record:" + id] = L("找不到这条记录，可能不在可读范围内"); return }
        showInTimeline?(start)
    }

    /// Typing 「@词」 at the end of the input looks up records to bring in as context.
    func draftChanged() {
        mentionTask?.cancel()
        guard let at = draft.lastIndex(of: "@"), !draft[draft.index(after: at)...].contains(where: \.isWhitespace) else { mentions = []; return }
        let query = String(draft[draft.index(after: at)...])
        mentionTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(250))
            guard let self, !Task.isCancelled, let channel = self.channel() else { return }
            var body: [String: Any] = ["limit": 6]
            if !query.isEmpty { body["q"] = query }
            let data = try? await channel.send("POST", "/v1/search", body: JSONSerialization.data(withJSONObject: body)).checked()
            guard !Task.isCancelled else { return }
            self.mentions = data.map { PiToolResult.records("<candid_data>" + (String(data: $0, encoding: .utf8) ?? "") + "</candid_data>") } ?? []
        }
    }

    /// Picks a suggestion: it becomes context, and 「@词」 leaves the input.
    func pickMention(_ ref: AIRecordRef) {
        if let at = draft.lastIndex(of: "@") { draft = String(draft[..<at]) }
        records[ref.eventID] = ref
        let item = AIContextItem(id: "record:" + ref.eventID, kind: ref.kindLabel, title: ref.label.dropFirst(ref.kindLabel.count).trimmingCharacters(in: .whitespaces) + (ref.title.map { " " + $0 } ?? ""))
        if !context.contains(item) { context.append(item) }
        mentions = []
    }

    /// The proposal behind a card, once the core has listed it.
    func proposal(_ id: String) -> AgendaProposal? { proposals.first { $0.id == id } }

    static func explain(_ error: Error) -> String {
        if case CoreError.service(_, let code, let message) = error {
            if code == "waiting_model" { return L("还没有给 AI 对话选模型：在 设置 → 模型 里给角色 agent_chat 选一个。") }
            if code == "remote_not_allowed" { return L("AI 对话用的是远程模型，但没有允许发往远程。") }
            let known = ["user_edited": L("这段已有你的修改，自动摘要会保留它"), "source_not_allowed": L("这条资料不在技能的读取范围内"), "source_unavailable": L("来源已不可用"), "configuration_changed": L("设置已改变，请重新运行"), "feature_disabled": L("这个技能已关闭，可在 AI 功能设置中开启"), "daily_run_budget": L("这个技能已到每日运行上限"), "source_policy_changed": L("来源权限已改变，请重新运行"), "model_configuration_changed": L("模型设置已改变，请重新运行"), "waiting_quality": L("自动运行还在等待质量评估")]
            return known[message] ?? message
        }
        return error.localizedDescription
    }

    // MARK: Pi events

    private var answerIndex: Int? { conversation.messages.lastIndex { $0.role == .assistant } }

    private func receive(_ event: PiEvent) {
        switch event {
        case .textDelta(let delta):
            streaming += delta
            setAnswer()
        case .assistantMessage(let text):
            finished.append(text); streaming = ""
            setAnswer()
        case .toolStarted(let id, let name, let arguments):
            pendingArguments[id] = arguments
            if let i = answerIndex {
                if !conversation.messages[i].steps.contains(PiToolResult.step(name)) { conversation.messages[i].steps.append(PiToolResult.step(name)) }
                conversation.messages[i].calls = (conversation.messages[i].calls ?? []) + [AIToolCall(id: id, tool: name, arguments: arguments)]
            }
        case .toolFinished(let id, let name, let result, let isError):
            let args = pendingArguments.removeValue(forKey: id) ?? "{}"
            let found = isError ? [] : PiToolResult.records(result)
            for r in found where records[r.eventID] == nil { records[r.eventID] = r }
            if let i = answerIndex, var calls = conversation.messages[i].calls, let c = calls.lastIndex(where: { $0.id == id }) {
                calls[c].state = isError ? .failed : .done
                calls[c].outcome = PiToolResult.outcome(result, isError: isError)
                calls[c].records = found
                conversation.messages[i].calls = calls
            }
            if let action = PiToolResult.action(tool: name, arguments: args, result: result, isError: isError), let i = answerIndex {
                conversation.messages[i].actions.append(action)
                Task { await refreshCore() }
            }
        case .settled(let aborted):
            turnRunning = false
            finishSkill(state: skillError != nil ? "failed" : aborted ? "cancelled" : "done", error: skillError)
            if aborted, let i = answerIndex, i == conversation.messages.count - 1 { conversation.messages[i].stopped = true }
            if case .skill = run { Task { await refreshCore() } }
            dropEmptyAnswer()
            if !isSplit && tab == .today { chatUnread = true }
            persist()
        case .error(let message):
            skillError = message
            conversation.messages.append(AIChatMessage(role: .note, text: message))
        case .restarting(_, let reason):
            agentProblem = L("AI 正在重启（%@）", reason)
        case .ready:
            agentProblem = nil
        case .turnStarted, .stopped:
            break
        }
    }

    private func finishSkill(state: String, error: String?) {
        skillHeartbeat?.cancel(); skillHeartbeat = nil
        guard let run = skillRun, let channel = channel() else { return }
        skillRun = nil
        let answer = conversation.messages.last(where: { $0.role == .assistant })
        let text = answer?.text ?? ""
        let meaningful = answer?.actions.contains(where: { $0.kind == .write && $0.status == "done" }) == true
        let data = try? JSONSerialization.data(withJSONObject: ["has_updates": meaningful, "summary": text])
        let result = data.flatMap { String(data: $0, encoding: .utf8) } ?? text
        Task {
            var body: [String: Any] = ["action": "finish", "id": run.id, "attempt": run.attempt, "state": state, "result": result]
            if let error { body["error"] = error }
            let _: AutomationRun? = try? await automationPost(channel, body)
        }
    }

    private func setAnswer() {
        guard let i = answerIndex else { return }
        conversation.messages[i].text = (finished + (streaming.isEmpty ? [] : [streaming])).joined(separator: "\n\n")
    }

    private func dropEmptyAnswer() {
        if let i = answerIndex, conversation.messages[i].text.isEmpty, conversation.messages[i].actions.isEmpty, i == conversation.messages.count - 1 {
            conversation.messages.remove(at: i)
        }
    }

    private func updateAction(_ id: String, _ change: (inout AIChatAction) -> Void) {
        for m in conversation.messages.indices {
            for a in conversation.messages[m].actions.indices where conversation.messages[m].actions[a].id == id {
                change(&conversation.messages[m].actions[a])
            }
        }
    }

    /// Proposal cards in the chat follow the proposal's status in the core.
    private func syncActionCards() {
        for p in proposals { updateAction(p.id) { if $0.kind == .proposal { $0.status = p.status; $0.detail = p.result?.error ?? $0.detail } } }
    }

    private func persist() {
        conversation.updated = now()
        try? store.save(conversation)
        history = store.list()
    }
}
