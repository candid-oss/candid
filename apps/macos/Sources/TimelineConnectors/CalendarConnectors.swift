import Foundation
import CryptoKit

public struct CalendarConnectorOptions: Sendable {
    /// Secret salt for attendee e-mail hashes. Required: an unsalted hash of an address is reversible by guessing.
    public var emailSalt: Data
    public var policy: ConnectorPolicy
    public var storeNotes = false
    public var pastWindowDays = 30
    public var futureWindowDays = 90
    public var timeZone: TimeZone = .current
    public var now: @Sendable () -> Date = { Date() }
    public init(emailSalt: Data, policy: ConnectorPolicy = CalendarConnector.defaultPolicy) {
        self.emailSalt = emailSalt; self.policy = policy
    }
}

func emailHash(_ email: String, salt: Data) -> String {
    var d = salt
    d.append(Data(email.trimmingCharacters(in: .whitespaces).lowercased().utf8))
    return SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined()
}

/// Per-stream cursor: content digest and start of what was last emitted, so unchanged events are not re-sent and
/// events that vanish from the synced window are reported as removed.
struct WindowCursor: Codable, Equatable {
    struct Entry: Codable, Equatable { var d: String; var s: Int64 }
    var v = 1
    var fromMs: Int64 = 0
    var toMs: Int64 = 0
    var entries: [String: Entry] = [:]
    enum CodingKeys: String, CodingKey { case v, fromMs = "from_ms", toMs = "to_ms", entries }
}

private func digest(_ e: ConnectorEvent) -> String {
    let enc = JSONEncoder(); enc.outputFormatting = [.sortedKeys]
    let d = (try? enc.encode(e)) ?? Data()
    return SHA256.hash(data: d).prefix(8).map { String(format: "%02x", $0) }.joined()
}

public class EventKitBackedConnector: Connector, @unchecked Sendable {
    let source: CalendarDataSource
    let options: CalendarConnectorOptions
    private let revokeFlag = RevokeFlag()
    init(source: CalendarDataSource, options: CalendarConnectorOptions) { self.source = source; self.options = options }

    public func describe() -> ConnectorDescription { fatalError("override") }
    public func authorize() async throws -> AuthorizationResult { fatalError("override") }
    func streams() async throws -> [SourceCalendar] { fatalError("override") }
    func streamName(_ id: String) -> String { fatalError("override") }
    func fetch(stream: SourceCalendar, range: DateInterval) async throws -> (threads: [ConnectorThread], events: [ConnectorEvent])
    { fatalError("override") }
    func authState() -> ConnectorAuthState { fatalError("override") }
    var sourceKind: String { fatalError("override") }

    var isRevoked: Bool { revokeFlag.isSet }
    public func revoke() async { revokeFlag.set() }

    public func sync(cursors: [String: String], options so: SyncOptions) async throws -> SyncResult {
        let now = options.now()
        let range = DateInterval(start: now.addingTimeInterval(-Double(options.pastWindowDays) * 86400),
                                 end: now.addingTimeInterval(Double(options.futureWindowDays) * 86400))
        return try await run(cursors: cursors, range: range)
    }
    public func backfill(range: DateInterval, cursors: [String: String], options so: SyncOptions) async throws -> SyncResult {
        try await run(cursors: cursors, range: range)
    }

    private func run(cursors: [String: String], range: DateInterval) async throws -> SyncResult {
        if isRevoked { throw ConnectorError.revoked }
        let state = authState()
        guard state == .authorized else { throw ConnectorError.notAuthorized(state) }
        var result = SyncResult(batches: [], cursors: cursors, issues: [], hasMore: false, stats: SyncStats())
        for cal in try await streams() {
            if isRevoked { throw ConnectorError.revoked }
            if options.policy.exclude.patterns.contains(where: { !$0.isEmpty && cal.name.lowercased().contains($0.lowercased()) })
                || options.policy.exclude.threads.contains(cal.id) { continue }
            let stream = streamName(cal.id)
            var prior = WindowCursor()
            if let text = cursors[stream] {
                guard let c = JSONCodec.decode(WindowCursor.self, from: text), c.v == 1 else {
                    throw ConnectorError.invalidCursor(stream: stream)
                }
                prior = c
            }
            let fetched: (threads: [ConnectorThread], events: [ConnectorEvent])
            do { fetched = try await fetch(stream: cal, range: range) } catch {
                result.issues.append(ConnectorIssue(stream: stream, location: cal.id, code: ConnectorIssue.Code.unreadable))
                continue
            }
            let fromMs = epochMs(range.start), toMs = epochMs(range.end)
            var next = WindowCursor(fromMs: fromMs, toMs: toMs)
            var changed: [ConnectorEvent] = []
            for e in fetched.events {
                let d = digest(e)
                next.entries[e.sourceKey] = .init(d: d, s: e.startMs ?? 0)
                if prior.entries[e.sourceKey]?.d != d { changed.append(e) }
            }
            var removed: [String] = []
            for (k, old) in prior.entries where next.entries[k] == nil {
                if old.s >= fromMs && old.s < toMs { removed.append(k) } else { next.entries[k] = old }
            }
            let text = JSONCodec.encode(next)
            result.cursors[stream] = text
            if changed.isEmpty && removed.isEmpty && prior.entries.keys == next.entries.keys { continue }
            let changedThreadKeys = Set(changed.compactMap(\.threadKey))
            let threads = fetched.threads.filter { changedThreadKeys.contains($0.sourceKey) }
            result.batches.append(ConnectorBatch(
                sourceKind: sourceKind, cursorAfter: ConnectorCursor(stream: stream, cursor: text),
                threads: threads, events: changed, removedSourceKeys: removed.sorted()))
        }
        return result
    }
}

// MARK: - Calendar

public final class CalendarConnector: EventKitBackedConnector, @unchecked Sendable {
    public static let kind = "calendar.system"
    public static let defaultPolicy = ConnectorPolicy(version: 1, storeText: true, storeMedia: false, agentVisible: true)

    public override init(source: CalendarDataSource, options: CalendarConnectorOptions) {
        super.init(source: source, options: options)
    }
    override var sourceKind: String { Self.kind }
    override func authState() -> ConnectorAuthState { source.eventsAuthorization() }
    override func streamName(_ id: String) -> String { "calendar:" + id }
    override func streams() async throws -> [SourceCalendar] { try await source.calendars() }

    public override func describe() -> ConnectorDescription {
        ConnectorDescription(kind: Self.kind, displayName: "Calendar", requiredPermissions: ["EventKit full access (read only use)"],
                             eventKinds: ["calendar.event"], threadKinds: [.calendarSeries], defaultPolicy: Self.defaultPolicy)
    }
    public override func authorize() async throws -> AuthorizationResult {
        AuthorizationResult(state: try await source.requestEventsAccess(), secretRef: nil)
    }

    override func fetch(stream cal: SourceCalendar, range: DateInterval) async throws
        -> (threads: [ConnectorThread], events: [ConnectorEvent]) {
        let records = try await source.events(calendarId: cal.id, range: range)
        var threads: [String: ConnectorThread] = [:]
        var events: [ConnectorEvent] = []
        var seen = Set<String>()
        for r in records {
            let e = map(r)
            guard seen.insert(e.sourceKey).inserted else { continue }
            events.append(e)
            if r.isRecurring {
                var t = threads[r.itemId] ?? ConnectorThread(
                    sourceKey: r.itemId, kind: .calendarSeries, title: r.title, startedMs: e.startMs,
                    payload: .object(["calendar_id": .string(r.calendarId), "calendar_name": .string(r.calendarName)]))
                if let s = e.startMs { t.startedMs = min(t.startedMs ?? s, s) }
                threads[r.itemId] = t
            }
        }
        return (threads.values.sorted { $0.sourceKey < $1.sourceKey }, events.sorted { ($0.startMs ?? 0, $0.sourceKey) < ($1.startMs ?? 0, $1.sourceKey) })
    }

    func map(_ r: CalendarEventRecord) -> ConnectorEvent {
        let tz = r.timeZoneId.flatMap(TimeZone.init(identifier:)) ?? options.timeZone
        let start = epochMs(r.start)
        var endDate = r.end
        if r.isAllDay {
            // EventKit ends all-day events at 23:59:59; the store uses half-open intervals.
            var cal = Calendar(identifier: .gregorian); cal.timeZone = tz
            let sod = cal.startOfDay(for: endDate)
            if sod != endDate, let next = cal.date(byAdding: .day, value: 1, to: sod) { endDate = next }
        }
        let attendees: [JSONValue] = r.attendees.map { a in
            var o: [String: JSONValue] = [:]
            if let n = a.name, !n.isEmpty { o["name"] = .string(n) }
            if let m = a.email, !m.isEmpty { o["email_hash"] = .string(emailHash(m, salt: options.emailSalt)) }
            return .object(o)
        }
        var p: [String: JSONValue] = [
            "calendar_id": .string(r.calendarId), "calendar_name": .string(r.calendarName),
            "all_day": .bool(r.isAllDay), "attendees": .array(attendees), "status": .string(r.status)]
        if let l = r.location, !l.isEmpty { p["location"] = .string(l) }
        if r.isRecurring { p["recurrence_id"] = .string(r.itemId) }
        if let m = r.lastModified { p["source_modified_ms"] = .int(epochMs(m)) }
        var text: [ConnectorText] = []
        if options.storeNotes, options.policy.storeText, let n = r.notes, !n.isEmpty {
            text.append(ConnectorText(field: "note", content: n))
        }
        let key = r.isRecurring ? "\(r.itemId)#\(epochMs(r.occurrenceStart ?? r.start))" : r.itemId
        return ConnectorEvent(
            sourceKey: key, kind: "calendar.event", startMs: start, endMs: max(epochMs(endDate), start),
            tzOffsetMin: tz.offsetMinutes(atMs: start), timeBasis: .source, title: r.title,
            threadKey: r.isRecurring ? r.itemId : nil, actor: .self, payloadVersion: 1, payload: .object(p), text: text)
    }
}

// MARK: - Reminders

public final class RemindersConnector: EventKitBackedConnector, @unchecked Sendable {
    public static let kind = "reminders.system"
    public static let defaultPolicy = ConnectorPolicy(version: 1, storeText: true, storeMedia: false, agentVisible: true)

    public override init(source: CalendarDataSource, options: CalendarConnectorOptions) {
        super.init(source: source, options: options)
    }
    override var sourceKind: String { Self.kind }
    override func authState() -> ConnectorAuthState { source.remindersAuthorization() }
    override func streamName(_ id: String) -> String { "reminders:" + id }
    override func streams() async throws -> [SourceCalendar] { try await source.reminderLists() }

    public override func describe() -> ConnectorDescription {
        ConnectorDescription(kind: Self.kind, displayName: "Reminders", requiredPermissions: ["EventKit reminders full access (read only use)"],
                             eventKinds: ["reminder.item"], threadKinds: [], defaultPolicy: Self.defaultPolicy)
    }
    public override func authorize() async throws -> AuthorizationResult {
        AuthorizationResult(state: try await source.requestRemindersAccess(), secretRef: nil)
    }

    override func fetch(stream list: SourceCalendar, range: DateInterval) async throws
        -> (threads: [ConnectorThread], events: [ConnectorEvent]) {
        let records = try await source.reminders(listId: list.id)
        var events: [ConnectorEvent] = []
        var seen = Set<String>()
        for r in records {
            let e = map(r)
            guard seen.insert(e.sourceKey).inserted else { continue }
            // Open reminders always count; finished ones only when their anchor time is inside the window.
            if r.completed != nil, let s = e.startMs, s < epochMs(range.start) || s >= epochMs(range.end) { continue }
            events.append(e)
        }
        return ([], events.sorted { ($0.startMs ?? 0, $0.sourceKey) < ($1.startMs ?? 0, $1.sourceKey) })
    }

    func map(_ r: ReminderRecord) -> ConnectorEvent {
        // Anchor: due time, else completion, else creation. Without any date the time is unknown (imported).
        let anchor = r.due ?? r.completed ?? r.created
        let start = anchor.map(epochMs)
        var p: [String: JSONValue] = ["list_name": .string(r.listName)]
        if let d = r.due { p["due_ms"] = .int(epochMs(d)) }
        if let c = r.completed { p["completed_ms"] = .int(epochMs(c)) }
        if let pr = r.priority, pr > 0 { p["priority"] = .int(Int64(pr)) }
        if let m = r.lastModified { p["source_modified_ms"] = .int(epochMs(m)) }
        var text: [ConnectorText] = []
        if options.storeNotes, options.policy.storeText, let n = r.notes, !n.isEmpty {
            text.append(ConnectorText(field: "note", content: n))
        }
        return ConnectorEvent(
            sourceKey: r.itemId, kind: "reminder.item", startMs: start,
            tzOffsetMin: start.map { options.timeZone.offsetMinutes(atMs: $0) },
            timeBasis: start == nil ? .imported : .source, title: r.title, actor: .self,
            payloadVersion: 1, payload: .object(p), text: text)
    }
}
