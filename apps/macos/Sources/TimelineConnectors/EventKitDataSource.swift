import Foundation
import EventKit

/// Real EventKit implementation. macOS 14 offers only "full access" for reading; this type never calls any
/// save/remove API, so the connector stays read-only in behaviour. Needs `NSCalendarsFullAccessUsageDescription` and
/// `NSRemindersFullAccessUsageDescription` in the host app's Info.plist (and the calendars/reminders entitlements when sandboxed).
public final class EventKitDataSource: CalendarDataSource, @unchecked Sendable {
    private let store = EKEventStore()
    public init() {}

    private static func map(_ s: EKAuthorizationStatus) -> ConnectorAuthState {
        switch s {
        case .notDetermined: return .notDetermined
        case .restricted: return .restricted
        case .denied: return .denied
        case .fullAccess, .authorized: return .authorized
        case .writeOnly: return .denied      // cannot read
        @unknown default: return .denied
        }
    }
    public func eventsAuthorization() -> ConnectorAuthState { Self.map(EKEventStore.authorizationStatus(for: .event)) }
    public func remindersAuthorization() -> ConnectorAuthState { Self.map(EKEventStore.authorizationStatus(for: .reminder)) }

    public func requestEventsAccess() async throws -> ConnectorAuthState {
        _ = try await store.requestFullAccessToEvents()
        return eventsAuthorization()
    }
    public func requestRemindersAccess() async throws -> ConnectorAuthState {
        _ = try await store.requestFullAccessToReminders()
        return remindersAuthorization()
    }

    public func calendars() async throws -> [SourceCalendar] {
        store.calendars(for: .event).map { SourceCalendar(id: $0.calendarIdentifier, name: $0.title) }
    }
    public func reminderLists() async throws -> [SourceCalendar] {
        store.calendars(for: .reminder).map { SourceCalendar(id: $0.calendarIdentifier, name: $0.title) }
    }

    public func events(calendarId: String, range: DateInterval) async throws -> [CalendarEventRecord] {
        guard let cal = store.calendars(for: .event).first(where: { $0.calendarIdentifier == calendarId }) else { return [] }
        // EventKit rejects spans over four years in one predicate; walk the range in one-year slices.
        var out: [CalendarEventRecord] = []
        var from = range.start
        while from < range.end {
            let to = min(range.end, Calendar.current.date(byAdding: .year, value: 1, to: from) ?? range.end)
            let p = store.predicateForEvents(withStart: from, end: to, calendars: [cal])
            for e in store.events(matching: p) {
                let recurring = e.hasRecurrenceRules || e.occurrenceDate != nil && e.recurrenceRules?.isEmpty == false
                out.append(CalendarEventRecord(
                    itemId: e.calendarItemExternalIdentifier ?? e.eventIdentifier ?? UUID().uuidString,
                    occurrenceStart: recurring ? e.occurrenceDate : nil,
                    calendarId: cal.calendarIdentifier, calendarName: cal.title, title: e.title ?? "",
                    location: e.location, notes: e.notes, start: e.startDate, end: e.endDate, isAllDay: e.isAllDay,
                    timeZoneId: e.timeZone?.identifier,
                    attendees: (e.attendees ?? []).map { a in
                        let mail = a.url.scheme == "mailto" ? a.url.absoluteString.replacingOccurrences(of: "mailto:", with: "") : nil
                        return AttendeeRecord(name: a.name == mail ? nil : a.name, email: mail)
                    },
                    status: Self.status(e.status), isRecurring: recurring, lastModified: e.lastModifiedDate))
            }
            from = to
        }
        return out
    }

    private static func status(_ s: EKEventStatus) -> String {
        switch s {
        case .none: return "none"
        case .confirmed: return "confirmed"
        case .tentative: return "tentative"
        case .canceled: return "canceled"
        @unknown default: return "none"
        }
    }

    public func reminders(listId: String) async throws -> [ReminderRecord] {
        guard let list = store.calendars(for: .reminder).first(where: { $0.calendarIdentifier == listId }) else { return [] }
        let predicate = store.predicateForReminders(in: [list])
        let name = list.title
        return await withCheckedContinuation { (cont: CheckedContinuation<[ReminderRecord], Never>) in
            store.fetchReminders(matching: predicate) { items in
                let cal = Calendar.current
                let mapped = (items ?? []).map { r in
                    ReminderRecord(
                        itemId: r.calendarItemExternalIdentifier ?? r.calendarItemIdentifier, listId: listId,
                        listName: name, title: r.title ?? "", notes: r.notes,
                        due: r.dueDateComponents.flatMap { cal.date(from: $0) }, completed: r.completionDate,
                        priority: r.priority, created: r.creationDate, lastModified: r.lastModifiedDate)
                }
                cont.resume(returning: mapped)
            }
        }
    }
}
