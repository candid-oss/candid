import Foundation
import Testing
@testable import TimelineConnectors

// SYNTHETIC calendars, people and addresses (example.invalid is a reserved non-existent domain).
final class FakeSource: CalendarDataSource, @unchecked Sendable {
    var eventsAuth = ConnectorAuthState.authorized, remindersAuth = ConnectorAuthState.authorized
    var cals = [SourceCalendar(id: "cal-1", name: "SYNTHETIC Work")]
    var lists = [SourceCalendar(id: "list-1", name: "SYNTHETIC List")]
    var eventRecords: [CalendarEventRecord] = []
    var reminderRecords: [ReminderRecord] = []
    var reads = 0
    func eventsAuthorization() -> ConnectorAuthState { eventsAuth }
    func remindersAuthorization() -> ConnectorAuthState { remindersAuth }
    func requestEventsAccess() async throws -> ConnectorAuthState { eventsAuth }
    func requestRemindersAccess() async throws -> ConnectorAuthState { remindersAuth }
    func calendars() async throws -> [SourceCalendar] { cals }
    func reminderLists() async throws -> [SourceCalendar] { lists }
    func events(calendarId: String, range: DateInterval) async throws -> [CalendarEventRecord] {
        reads += 1
        return eventRecords.filter { $0.calendarId == calendarId && $0.start < range.end && $0.end > range.start }
    }
    func reminders(listId: String) async throws -> [ReminderRecord] { reminderRecords.filter { $0.listId == listId } }
}

private let salt = Data("synthetic-salt".utf8)
private let t0 = Date(timeIntervalSince1970: 1_794_276_900)       // 2026-11-10T02:15:00Z
private let tokyo = TimeZone(identifier: "Asia/Tokyo")!

private func options(notes: Bool = false, policy: ConnectorPolicy = CalendarConnector.defaultPolicy) -> CalendarConnectorOptions {
    var o = CalendarConnectorOptions(emailSalt: salt, policy: policy)
    o.storeNotes = notes; o.timeZone = TimeZone(identifier: "Asia/Shanghai")!; o.now = { t0 }
    return o
}
private func meeting(_ id: String = "ev-1", start: Date = t0.addingTimeInterval(3600), title: String = "SYNTHETIC meeting") -> CalendarEventRecord {
    CalendarEventRecord(itemId: id, calendarId: "cal-1", calendarName: "SYNTHETIC Work", title: title,
                        location: "SYNTHETIC room", notes: "SYNTHETIC-NOTES", start: start, end: start.addingTimeInterval(1800),
                        attendees: [AttendeeRecord(name: "Synthetic Person", email: " Person@Example.invalid "),
                                    AttendeeRecord(name: nil, email: "other@example.invalid")])
}

@Test func calendarMapsEventAndReducesAttendees() async throws {
    let s = FakeSource(); s.eventRecords = [meeting()]
    let r = try await CalendarConnector(source: s, options: options()).sync(cursors: [:], options: SyncOptions())
    let e = try #require(r.events.first)
    #expect(e.kind == "calendar.event" && e.sourceKey == "ev-1" && e.title == "SYNTHETIC meeting")
    #expect(e.startMs == 1_794_280_500_000 && e.endMs == 1_794_282_300_000 && e.timeBasis == .source)
    #expect(e.tzOffsetMin == 480 && e.threadKey == nil && e.payload["all_day"]?.boolValue == false)
    #expect(e.payload["calendar_name"]?.stringValue == "SYNTHETIC Work" && e.payload["location"]?.stringValue == "SYNTHETIC room")
    let att = try #require(e.payload["attendees"]?.arrayValue)
    #expect(att.count == 2 && att[0]["name"]?.stringValue == "Synthetic Person" && att[1]["name"] == nil)
    let h0 = try #require(att[0]["email_hash"]?.stringValue)
    #expect(h0 == emailHash("person@example.invalid", salt: salt) && h0.count == 64)
    #expect(h0 != emailHash("person@example.invalid", salt: Data("other".utf8)))
    #expect(e.text.isEmpty)
    let blob = String(decoding: try JSONEncoder().encode(r.batches), as: UTF8.self)
    #expect(!blob.contains("example.invalid") && !blob.contains("SYNTHETIC-NOTES"))
    #expect(r.batches[0].cursorAfter.stream == "calendar:cal-1")
}

@Test func calendarNotesOnlyWhenEnabledAndAllowedByPolicy() async throws {
    let s = FakeSource(); s.eventRecords = [meeting()]
    let on = try await CalendarConnector(source: s, options: options(notes: true)).sync(cursors: [:], options: SyncOptions())
    #expect(on.events[0].text == [ConnectorText(field: "note", content: "SYNTHETIC-NOTES")])
    var p = CalendarConnector.defaultPolicy; p.storeText = false
    let off = try await CalendarConnector(source: s, options: options(notes: true, policy: p)).sync(cursors: [:], options: SyncOptions())
    #expect(off.events[0].text.isEmpty)
}

@Test func calendarGroupsRecurringOccurrencesIntoOneSeriesThread() async throws {
    let s = FakeSource()
    s.eventRecords = (1...3).map { i in
        var r = meeting("series-1", start: t0.addingTimeInterval(Double(i) * 86400), title: "SYNTHETIC standup")
        r.isRecurring = true; r.occurrenceStart = r.start
        return r
    }
    let r = try await CalendarConnector(source: s, options: options()).sync(cursors: [:], options: SyncOptions())
    #expect(r.events.count == 3 && Set(r.events.map(\.sourceKey)).count == 3)
    #expect(r.events.allSatisfy { $0.threadKey == "series-1" && $0.payload["recurrence_id"]?.stringValue == "series-1" })
    #expect(r.threads.count == 1 && r.threads[0].kind == .calendarSeries && r.threads[0].sourceKey == "series-1")
    #expect(r.threads[0].startedMs == r.events.map(\.startMs).compactMap { $0 }.min())
}

@Test func calendarAllDayUsesEventTimezoneAndHalfOpenEnd() async throws {
    var cal = Calendar(identifier: .gregorian); cal.timeZone = tokyo
    let day = cal.date(from: DateComponents(year: 2026, month: 11, day: 12))!
    var r = meeting("allday", start: day, title: "SYNTHETIC holiday")
    r.end = day.addingTimeInterval(86399); r.isAllDay = true; r.timeZoneId = "Asia/Tokyo"
    let s = FakeSource(); s.eventRecords = [r]
    let e = try await CalendarConnector(source: s, options: options()).sync(cursors: [:], options: SyncOptions()).events[0]
    #expect(e.tzOffsetMin == 540 && e.payload["all_day"]?.boolValue == true)
    #expect(e.endMs! - e.startMs! == 86_400_000)
}

@Test func calendarResyncEmitsOnlyChangesAndReportsRemovals() async throws {
    let s = FakeSource(); s.eventRecords = [meeting("a"), meeting("b", start: t0.addingTimeInterval(7200))]
    let c = CalendarConnector(source: s, options: options())
    let first = try await c.sync(cursors: [:], options: SyncOptions())
    #expect(first.events.count == 2)
    let same = try await c.sync(cursors: first.cursors, options: SyncOptions())
    #expect(same.batches.isEmpty)
    s.eventRecords[0].title = "SYNTHETIC renamed"
    let changed = try await c.sync(cursors: first.cursors, options: SyncOptions())
    #expect(changed.events.map(\.sourceKey) == ["a"])
    s.eventRecords.removeAll { $0.itemId == "b" }
    let gone = try await c.sync(cursors: changed.cursors, options: SyncOptions())
    #expect(gone.events.isEmpty && gone.batches.first?.removedSourceKeys == ["b"])
    let fresh = try await c.sync(cursors: [:], options: SyncOptions())
    #expect(fresh.events.map(\.sourceKey) == ["a"])
}

@Test func calendarBackfillUsesTheGivenRange() async throws {
    let s = FakeSource()
    s.eventRecords = [meeting("old", start: t0.addingTimeInterval(-200 * 86400)), meeting("new")]
    let c = CalendarConnector(source: s, options: options())
    #expect(try await c.sync(cursors: [:], options: SyncOptions()).events.map(\.sourceKey) == ["new"])
    let range = DateInterval(start: t0.addingTimeInterval(-365 * 86400), end: t0)
    #expect(try await c.backfill(range: range, cursors: [:], options: SyncOptions()).events.map(\.sourceKey) == ["old"])
}

@Test func calendarRefusesWithoutAuthorizationAndAfterRevoke() async throws {
    let s = FakeSource(); s.eventRecords = [meeting()]; s.eventsAuth = .denied
    let c = CalendarConnector(source: s, options: options())
    await #expect(throws: ConnectorError.notAuthorized(.denied)) { try await c.sync(cursors: [:], options: SyncOptions()) }
    #expect(s.reads == 0)
    s.eventsAuth = .authorized
    #expect(try await c.sync(cursors: [:], options: SyncOptions()).events.count == 1)
    await c.revoke()
    let before = s.reads
    await #expect(throws: ConnectorError.revoked) { try await c.sync(cursors: [:], options: SyncOptions()) }
    #expect(s.reads == before)
}

@Test func calendarExcludedCalendarsAreNotRead() async throws {
    let s = FakeSource(); s.eventRecords = [meeting()]
    var p = CalendarConnector.defaultPolicy; p.exclude.patterns = ["synthetic work"]
    let r = try await CalendarConnector(source: s, options: options(policy: p)).sync(cursors: [:], options: SyncOptions())
    #expect(r.batches.isEmpty && s.reads == 0)
}

@Test func remindersMapDueCompletedAndUndated() async throws {
    let s = FakeSource()
    s.reminderRecords = [
        ReminderRecord(itemId: "r1", listId: "list-1", listName: "SYNTHETIC List", title: "SYNTHETIC due", notes: "SYNTHETIC-NOTES",
                       due: t0.addingTimeInterval(3600), priority: 1),
        ReminderRecord(itemId: "r2", listId: "list-1", listName: "SYNTHETIC List", title: "SYNTHETIC done",
                       completed: t0.addingTimeInterval(-3600)),
        ReminderRecord(itemId: "r3", listId: "list-1", listName: "SYNTHETIC List", title: "SYNTHETIC undated"),
        ReminderRecord(itemId: "r4", listId: "list-1", listName: "SYNTHETIC List", title: "SYNTHETIC ancient",
                       completed: t0.addingTimeInterval(-400 * 86400))]
    let r = try await RemindersConnector(source: s, options: options()).sync(cursors: [:], options: SyncOptions())
    let byKey = Dictionary(uniqueKeysWithValues: r.events.map { ($0.sourceKey, $0) })
    #expect(Set(byKey.keys) == ["r1", "r2", "r3"])                   // finished reminder outside the window is left out
    #expect(byKey["r1"]?.kind == "reminder.item" && byKey["r1"]?.payload["due_ms"]?.intValue == 1_794_280_500_000)
    #expect(byKey["r1"]?.payload["priority"]?.intValue == 1 && byKey["r1"]?.text.isEmpty == true)
    #expect(byKey["r2"]?.payload["completed_ms"]?.intValue == 1_794_273_300_000)
    #expect(byKey["r3"]?.startMs == nil && byKey["r3"]?.timeBasis == .imported)
    #expect(r.batches[0].cursorAfter.stream == "reminders:list-1")
}

@Test func describeAndDefaultPolicyJSON() throws {
    let s = FakeSource()
    let d = CalendarConnector(source: s, options: options()).describe()
    #expect(d.kind == "calendar.system" && d.eventKinds == ["calendar.event"] && d.threadKinds == [.calendarSeries])
    let p = try JSONDecoder().decode(JSONValue.self, from: Data(d.defaultPolicyJSON.utf8))
    #expect(p["version"]?.intValue == 1 && p["store_text"]?.boolValue == true && p["store_media"]?.boolValue == false)
    #expect(p["exclude"]?["apps"] != nil && p["exclude"]?["threads"] != nil && p["exclude"]?["patterns"] != nil)
}
