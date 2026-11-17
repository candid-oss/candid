import EventKit
import Foundation

// The AI page's system calendar and Reminders (agent-platform-v1 §4.2 ai-settings): read through EventKit each time the
// page opens, never stored. Writes happen only when the user approves a proposal (§4.3). The page talks to
// `SystemAgenda`, so tests use a fake and never touch the user's calendars.

enum AgendaKind: String, Sendable { case calendar, reminders }

enum AgendaAccess: Equatable, Sendable { case notDetermined, granted, denied }

struct AgendaEvent: Identifiable, Equatable, Sendable {
    /// `<calendarItemIdentifier>@<start seconds>`: one occurrence, also what Pi gets as `item_id`.
    var id: String
    var title: String
    var start: Date
    var end: Date
    var isAllDay = false
    var location: String?
    var calendarID: String
    var calendar: String
    var notes: String?

    static func id(item: String, start: Date) -> String { item + "@" + String(Int64(start.timeIntervalSince1970)) }
    /// The item identifier and occurrence start encoded in an id.
    static func parse(_ id: String) -> (item: String, start: Date?) {
        guard let at = id.lastIndex(of: "@"), let seconds = Int64(id[id.index(after: at)...]) else { return (id, nil) }
        return (String(id[..<at]), Date(timeIntervalSince1970: TimeInterval(seconds)))
    }
}

struct AgendaReminder: Identifiable, Equatable, Sendable {
    var id: String
    var title: String
    /// Reminders without a due date are not shown on the page (it lists today's and overdue ones).
    var due: Date
    var listID: String
    var list: String
    var notes: String?
}

/// What the App writes for an approved proposal (payload fields of `create_reminder` / `create_calendar_event`).
struct AgendaDraft: Equatable, Sendable {
    var title: String
    var due: Date?
    var start: Date?
    var end: Date?
    var notes: String?
    /// For updates: drop the reminder's due date.
    var clearDue = false
}

/// The system's answer to a write: recorded back onto the proposal (§4.3 step 3).
struct AgendaReceipt: Equatable, Sendable {
    var itemID: String
    var container: String
    var json: [String: String] { ["item_id": itemID, "container": container] }
}

enum AgendaWriteError: LocalizedError, Equatable {
    case noAccess(AgendaKind)
    case incomplete(String)
    case noContainer(AgendaKind)
    case missing(AgendaKind)

    var errorDescription: String? {
        switch self {
        case .noAccess(.calendar): "没有日历权限，请在系统设置 → 隐私与安全性 → 日历里允许 Candid"
        case .noAccess(.reminders): "没有提醒事项权限，请在系统设置 → 隐私与安全性 → 提醒事项里允许 Candid"
        case .incomplete(let what): "提案缺少\(what)"
        case .noContainer(.calendar): "系统里没有可写入的日历"
        case .noContainer(.reminders): "系统里没有可写入的提醒事项列表"
        case .missing(.calendar): "日历里找不到这条日程，可能已被改动或删除"
        case .missing(.reminders): "提醒事项里找不到这条提醒，可能已被完成或删除"
        }
    }
}

protocol SystemAgenda: AnyObject, Sendable {
    func access(_ kind: AgendaKind) -> AgendaAccess
    func requestAccess(_ kind: AgendaKind) async -> AgendaAccess
    /// Events overlapping `[start, end)` in every calendar except `excluding` (calendar IDs).
    func events(from start: Date, to end: Date, excluding: Set<String>) async throws -> [AgendaEvent]
    /// Incomplete reminders due before `end` (so overdue ones too), in every list except `excluding`.
    func reminders(dueBefore end: Date, excluding: Set<String>) async throws -> [AgendaReminder]
    func createReminder(_ draft: AgendaDraft) async throws -> AgendaReceipt
    func createEvent(_ draft: AgendaDraft) async throws -> AgendaReceipt
    /// Changes only the fields the draft sets (title, due/start/end, notes).
    func updateReminder(_ id: String, _ draft: AgendaDraft) async throws -> AgendaReceipt
    func completeReminder(_ id: String) async throws -> AgendaReceipt
    func deleteReminder(_ id: String) async throws -> AgendaReceipt
    func updateEvent(_ id: String, _ draft: AgendaDraft) async throws -> AgendaReceipt
    func deleteEvent(_ id: String) async throws -> AgendaReceipt
}

/// The real system calendar and Reminders.
final class EventKitAgenda: SystemAgenda, @unchecked Sendable {
    private let store = EKEventStore()

    func access(_ kind: AgendaKind) -> AgendaAccess {
        switch EKEventStore.authorizationStatus(for: kind == .calendar ? .event : .reminder) {
        case .notDetermined: .notDetermined
        case .fullAccess, .authorized: .granted
        default: .denied
        }
    }

    func requestAccess(_ kind: AgendaKind) async -> AgendaAccess {
        _ = try? await (kind == .calendar ? store.requestFullAccessToEvents() : store.requestFullAccessToReminders())
        return access(kind)
    }

    func events(from start: Date, to end: Date, excluding: Set<String>) async throws -> [AgendaEvent] {
        guard access(.calendar) == .granted else { return [] }
        let calendars = store.calendars(for: .event).filter { !excluding.contains($0.calendarIdentifier) }
        guard !calendars.isEmpty else { return [] }
        return store.events(matching: store.predicateForEvents(withStart: start, end: end, calendars: calendars)).map { e in
            AgendaEvent(id: AgendaEvent.id(item: e.calendarItemIdentifier, start: e.startDate),
                        title: e.title ?? "", start: e.startDate, end: e.endDate, isAllDay: e.isAllDay, location: e.location,
                        calendarID: e.calendar.calendarIdentifier, calendar: e.calendar.title, notes: e.notes)
        }
    }

    func reminders(dueBefore end: Date, excluding: Set<String>) async throws -> [AgendaReminder] {
        guard access(.reminders) == .granted else { return [] }
        let lists = store.calendars(for: .reminder).filter { !excluding.contains($0.calendarIdentifier) }
        guard !lists.isEmpty else { return [] }
        let predicate = store.predicateForIncompleteReminders(withDueDateStarting: nil, ending: end, calendars: lists)
        let found: [AgendaReminder] = await withCheckedContinuation { continuation in
            store.fetchReminders(matching: predicate) { reminders in
                let items = (reminders ?? []).compactMap { r -> AgendaReminder? in
                    guard let due = r.dueDateComponents.flatMap({ Calendar.current.date(from: $0) }) else { return nil }
                    return AgendaReminder(id: r.calendarItemIdentifier, title: r.title ?? "", due: due,
                                          listID: r.calendar.calendarIdentifier, list: r.calendar.title, notes: r.notes)
                }
                continuation.resume(returning: items)
            }
        }
        return found
    }

    func createReminder(_ draft: AgendaDraft) async throws -> AgendaReceipt {
        guard access(.reminders) == .granted else { throw AgendaWriteError.noAccess(.reminders) }
        guard let list = store.defaultCalendarForNewReminders() else { throw AgendaWriteError.noContainer(.reminders) }
        let reminder = EKReminder(eventStore: store)
        reminder.title = draft.title
        reminder.notes = draft.notes
        reminder.calendar = list
        if let due = draft.due {
            reminder.dueDateComponents = Calendar.current.dateComponents([.year, .month, .day, .hour, .minute], from: due)
        }
        try store.save(reminder, commit: true)
        return AgendaReceipt(itemID: reminder.calendarItemIdentifier, container: list.title)
    }

    func createEvent(_ draft: AgendaDraft) async throws -> AgendaReceipt {
        guard access(.calendar) == .granted else { throw AgendaWriteError.noAccess(.calendar) }
        guard let start = draft.start else { throw AgendaWriteError.incomplete("开始时间") }
        guard let calendar = store.defaultCalendarForNewEvents else { throw AgendaWriteError.noContainer(.calendar) }
        let event = EKEvent(eventStore: store)
        event.title = draft.title
        event.notes = draft.notes
        event.calendar = calendar
        event.startDate = start
        event.endDate = draft.end.map { max($0, start) } ?? start.addingTimeInterval(30 * 60)
        try store.save(event, span: .thisEvent, commit: true)
        return AgendaReceipt(itemID: AgendaEvent.id(item: event.calendarItemIdentifier, start: start), container: calendar.title)
    }

    private func reminder(_ id: String) throws -> EKReminder {
        guard access(.reminders) == .granted else { throw AgendaWriteError.noAccess(.reminders) }
        guard let r = store.calendarItem(withIdentifier: id) as? EKReminder else { throw AgendaWriteError.missing(.reminders) }
        return r
    }

    /// The occurrence the id names (a repeating event's other occurrences stay as they are).
    private func event(_ id: String) throws -> EKEvent {
        guard access(.calendar) == .granted else { throw AgendaWriteError.noAccess(.calendar) }
        let (item, start) = AgendaEvent.parse(id)
        if let start {
            let found = store.events(matching: store.predicateForEvents(withStart: start.addingTimeInterval(-60), end: start.addingTimeInterval(86_400), calendars: nil))
            if let e = found.first(where: { $0.calendarItemIdentifier == item && abs($0.startDate.timeIntervalSince(start)) < 1 }) { return e }
        }
        guard let e = store.calendarItem(withIdentifier: item) as? EKEvent else { throw AgendaWriteError.missing(.calendar) }
        return e
    }

    func updateReminder(_ id: String, _ draft: AgendaDraft) async throws -> AgendaReceipt {
        let r = try reminder(id)
        if !draft.title.isEmpty { r.title = draft.title }
        if let notes = draft.notes { r.notes = notes }
        if draft.clearDue { r.dueDateComponents = nil }
        else if let due = draft.due { r.dueDateComponents = Calendar.current.dateComponents([.year, .month, .day, .hour, .minute], from: due) }
        try store.save(r, commit: true)
        return AgendaReceipt(itemID: r.calendarItemIdentifier, container: r.calendar.title)
    }

    func completeReminder(_ id: String) async throws -> AgendaReceipt {
        let r = try reminder(id)
        r.isCompleted = true
        try store.save(r, commit: true)
        return AgendaReceipt(itemID: r.calendarItemIdentifier, container: r.calendar.title)
    }

    func deleteReminder(_ id: String) async throws -> AgendaReceipt {
        let r = try reminder(id)
        let list = r.calendar.title
        try store.remove(r, commit: true)
        return AgendaReceipt(itemID: id, container: list)
    }

    func updateEvent(_ id: String, _ draft: AgendaDraft) async throws -> AgendaReceipt {
        let e = try event(id)
        if !draft.title.isEmpty { e.title = draft.title }
        if let notes = draft.notes { e.notes = notes }
        if let start = draft.start {
            let length = e.endDate.timeIntervalSince(e.startDate)
            e.startDate = start
            e.endDate = draft.end.map { max($0, start) } ?? start.addingTimeInterval(length)
        } else if let end = draft.end { e.endDate = max(end, e.startDate) }
        try store.save(e, span: .thisEvent, commit: true)
        return AgendaReceipt(itemID: AgendaEvent.id(item: e.calendarItemIdentifier, start: e.startDate), container: e.calendar.title)
    }

    func deleteEvent(_ id: String) async throws -> AgendaReceipt {
        let e = try event(id)
        let calendar = e.calendar.title
        try store.remove(e, span: .thisEvent, commit: true)
        return AgendaReceipt(itemID: id, container: calendar)
    }
}

// MARK: Grouping

/// The 日程 card: today's events in order, each marked past, now (running or starting within 30 minutes) or later.
struct AgendaDay: Equatable {
    enum Phase: Equatable { case past, now, later }
    struct Row: Equatable, Identifiable {
        var event: AgendaEvent
        var phase: Phase
        var id: String { event.id }
    }
    var allDay: [AgendaEvent]
    var rows: [Row]

    static func make(_ events: [AgendaEvent], now: Date, soon: TimeInterval = 30 * 60) -> AgendaDay {
        let sorted = events.sorted { ($0.start, $0.title) < ($1.start, $1.title) }
        let rows = sorted.filter { !$0.isAllDay }.map { e -> Row in
            let phase: Phase = e.end <= now ? .past : (e.start <= now.addingTimeInterval(soon) ? .now : .later)
            return Row(event: e, phase: phase)
        }
        return AgendaDay(allDay: sorted.filter(\.isAllDay), rows: rows)
    }
}

/// The 提醒 card: overdue first (oldest first), then due today; tomorrow's only counted for 「＋ n 条明天到期」.
struct ReminderGroups: Equatable {
    struct Item: Equatable, Identifiable {
        var reminder: AgendaReminder
        /// Whole days past due; 0 for today.
        var overdueDays: Int
        var id: String { reminder.id }
    }
    var items: [Item]
    var tomorrow: Int

    static func make(_ reminders: [AgendaReminder], now: Date, calendar: Calendar = .current) -> ReminderGroups {
        let today = calendar.startOfDay(for: now)
        let tomorrowStart = calendar.date(byAdding: .day, value: 1, to: today)!
        let afterTomorrow = calendar.date(byAdding: .day, value: 2, to: today)!
        let current = reminders.filter { $0.due < tomorrowStart }.sorted { ($0.due, $0.title) < ($1.due, $1.title) }
        let items = current.map { r in
            Item(reminder: r, overdueDays: max(0, calendar.dateComponents([.day], from: calendar.startOfDay(for: r.due), to: today).day ?? 0))
        }
        return ReminderGroups(items: items, tomorrow: reminders.filter { $0.due >= tomorrowStart && $0.due < afterTomorrow }.count)
    }
}
