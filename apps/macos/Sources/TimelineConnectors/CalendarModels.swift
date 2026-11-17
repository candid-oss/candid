import Foundation

public struct SourceCalendar: Sendable, Hashable {
    public var id: String
    public var name: String
    public init(id: String, name: String) { self.id = id; self.name = name }
}

public struct AttendeeRecord: Sendable, Hashable {
    public var name: String?
    public var email: String?
    public init(name: String? = nil, email: String? = nil) { self.name = name; self.email = email }
}

public struct CalendarEventRecord: Sendable, Hashable {
    /// Stable across devices where available (`calendarItemExternalIdentifier`), else `eventIdentifier`.
    public var itemId: String
    /// Original start of this occurrence of a recurring event; nil for non-recurring events.
    public var occurrenceStart: Date?
    public var calendarId: String
    public var calendarName: String
    public var title: String
    public var location: String?
    public var notes: String?
    public var start: Date
    public var end: Date
    public var isAllDay: Bool
    /// IANA identifier; nil means "floating" local time.
    public var timeZoneId: String?
    public var attendees: [AttendeeRecord]
    public var status: String            // none | confirmed | tentative | canceled
    public var isRecurring: Bool
    public var lastModified: Date?
    public init(itemId: String, occurrenceStart: Date? = nil, calendarId: String, calendarName: String, title: String,
                location: String? = nil, notes: String? = nil, start: Date, end: Date, isAllDay: Bool = false,
                timeZoneId: String? = nil, attendees: [AttendeeRecord] = [], status: String = "confirmed",
                isRecurring: Bool = false, lastModified: Date? = nil) {
        self.itemId = itemId; self.occurrenceStart = occurrenceStart; self.calendarId = calendarId
        self.calendarName = calendarName; self.title = title; self.location = location; self.notes = notes
        self.start = start; self.end = end; self.isAllDay = isAllDay; self.timeZoneId = timeZoneId
        self.attendees = attendees; self.status = status; self.isRecurring = isRecurring; self.lastModified = lastModified
    }
}

public struct ReminderRecord: Sendable, Hashable {
    public var itemId: String
    public var listId: String
    public var listName: String
    public var title: String
    public var notes: String?
    public var due: Date?
    public var completed: Date?
    public var priority: Int?
    public var created: Date?
    public var lastModified: Date?
    public init(itemId: String, listId: String, listName: String, title: String, notes: String? = nil, due: Date? = nil,
                completed: Date? = nil, priority: Int? = nil, created: Date? = nil, lastModified: Date? = nil) {
        self.itemId = itemId; self.listId = listId; self.listName = listName; self.title = title; self.notes = notes
        self.due = due; self.completed = completed; self.priority = priority; self.created = created
        self.lastModified = lastModified
    }
}

/// Seam between the connectors and EventKit. The real implementation is `EventKitDataSource`; tests use a fake.
public protocol CalendarDataSource: Sendable {
    func eventsAuthorization() -> ConnectorAuthState
    func remindersAuthorization() -> ConnectorAuthState
    func requestEventsAccess() async throws -> ConnectorAuthState
    func requestRemindersAccess() async throws -> ConnectorAuthState
    func calendars() async throws -> [SourceCalendar]
    func reminderLists() async throws -> [SourceCalendar]
    func events(calendarId: String, range: DateInterval) async throws -> [CalendarEventRecord]
    func reminders(listId: String) async throws -> [ReminderRecord]
}
