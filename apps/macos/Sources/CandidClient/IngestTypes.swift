import Foundation

/// A JSON value for event payloads (timeline-v1 §4.3); keys are written sorted.
public enum CoreJSON: Sendable, Hashable, Codable {
    case null, bool(Bool), number(Double), int(Int64), string(String), array([CoreJSON]), object([String: CoreJSON])

    public init(from decoder: any Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let v = try? c.decode(Bool.self) { self = .bool(v) }
        else if let v = try? c.decode(Int64.self) { self = .int(v) }
        else if let v = try? c.decode(Double.self) { self = .number(v) }
        else if let v = try? c.decode(String.self) { self = .string(v) }
        else if let v = try? c.decode([CoreJSON].self) { self = .array(v) }
        else { self = .object(try c.decode([String: CoreJSON].self)) }
    }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .null: try c.encodeNil()
        case .bool(let v): try c.encode(v)
        case .number(let v): try c.encode(v)
        case .int(let v): try c.encode(v)
        case .string(let v): try c.encode(v)
        case .array(let v): try c.encode(v)
        case .object(let v): try c.encode(v)
        }
    }
}

extension CoreJSON: ExpressibleByStringLiteral, ExpressibleByBooleanLiteral {
    public init(stringLiteral value: String) { self = .string(value) }
    public init(booleanLiteral value: Bool) { self = .bool(value) }
}

/// One text field of an event (§6.1); the core splits it into paragraphs.
public struct IngestText: Codable, Sendable, Hashable {
    public var field: String
    public var content: String
    public init(field: String, content: String) { self.field = field; self.content = content }
}

/// An original (or derived file) the core moves from its staging directory into `blobs/`.
public struct IngestBlob: Codable, Sendable, Hashable {
    public var staging_path: String
    public var sha256: String
    public var role: String
    public var mime_type: String?
    public var pixel_w: Int?
    public var pixel_h: Int?
    public init(staging_path: String, sha256: String, role: String = "original", mime_type: String? = nil, pixel_w: Int? = nil, pixel_h: Int? = nil) {
        self.staging_path = staging_path; self.sha256 = sha256; self.role = role; self.mime_type = mime_type; self.pixel_w = pixel_w; self.pixel_h = pixel_h
    }
}

/// One event of an `IngestBatch` (timeline-v1 §6.1). Absent optionals are left out of the JSON.
public struct IngestEvent: Codable, Sendable, Hashable {
    public var source_key: String
    public var kind: String
    public var start_ms: Int64?
    public var time_basis: String = "observed"
    public var app_bundle_id: String?
    public var app_name: String?
    public var title: String?
    public var parent_key: String?
    public var actor: String = "self"
    public var sensitivity: String = "normal"
    public var payload_version: Int = 1
    public var payload: [String: CoreJSON] = [:]
    public var text: [IngestText] = []
    public var blobs: [IngestBlob] = []

    public init(source_key: String, kind: String, start_ms: Int64?) {
        self.source_key = source_key; self.kind = kind; self.start_ms = start_ms
    }
}

public struct IngestBatch: Codable, Sendable, Hashable {
    public var source_id: String
    public var events: [IngestEvent]
    /// `detach`: keep an event whose parent never arrived (withheld by the capture gate, or deleted) without the link.
    public var on_missing_parent: String?
    /// `true`: the core folds typing continuations of `screen.text` into the earlier event (provisional rule).
    public var merge_text: Bool?
    public init(source_id: String, events: [IngestEvent], on_missing_parent: String? = nil, merge_text: Bool? = nil) {
        self.source_id = source_id; self.events = events; self.on_missing_parent = on_missing_parent; self.merge_text = merge_text
    }
}

public struct IngestResult: Codable, Sendable, Equatable {
    public var inserted: Int
    public var duplicates: Int
    public var conflicts: Int
    public var tombstoned: Int
    public var merged: Int?
    public var blobs_stored: Int
}

/// A source policy (§4.1) for registering the App's own sources.
public struct SourcePolicy: Codable, Sendable, Equatable {
    public struct Exclude: Codable, Sendable, Equatable { public var apps: [String] = []; public var threads: [String] = []; public var patterns: [String] = [] }
    public var version = 1
    public var store_text: Bool
    public var store_media: Bool
    public var agent_visible = true
    public var exclude = Exclude()
    public init(store_text: Bool, store_media: Bool) { self.store_text = store_text; self.store_media = store_media }
}

// MARK: The App's desktop reads (`app_*` tools, internal client only)

public struct AppContext: Codable, Sendable, Equatable {
    public var record_evidence: CoreJSON? = nil
    public var focused_role: String?
    public var focused_label: String?
    public var selected_text: String?
    public var readable_text: String?
    public var text_status: String?
    public var is_protected: Bool?
    public var session_active: Bool?
    public var on_battery: Bool?
    public var low_power: Bool?
    public var thermal_state: String?
    public var idle_seconds: Double?
    public var is_composing: Bool?

    public init(focused_role: String? = nil, focused_label: String? = nil, selected_text: String? = nil, readable_text: String? = nil,
                text_status: String? = nil, is_protected: Bool? = nil, session_active: Bool? = nil, on_battery: Bool? = nil,
                low_power: Bool? = nil, thermal_state: String? = nil, idle_seconds: Double? = nil, is_composing: Bool? = nil) {
        self.focused_role = focused_role; self.focused_label = focused_label; self.selected_text = selected_text; self.readable_text = readable_text
        self.text_status = text_status; self.is_protected = is_protected; self.session_active = session_active; self.on_battery = on_battery
        self.low_power = low_power; self.thermal_state = thermal_state; self.idle_seconds = idle_seconds; self.is_composing = is_composing
    }
}

/// A desktop record as the core reads it back; `id` is the capture's own record ID.
public struct AppRecord: Codable, Sendable, Equatable {
    public var id: String
    public var event_id: String
    public var ts_ms: Int64
    public var kind: String
    public var app_name: String
    public var bundle_id: String
    public var manual_title: String? = nil
    public var manual_notes: String? = nil
    public var window_title: String
    public var detail: String
    public var image_hash: String?
    public var image_width: Int?
    public var image_height: Int?
    public var image_path: String?
    public var ax_evidence_path: String?
    public var parent_id: String?
    public var run_id: String
    public var trigger: String?
    public var context: AppContext?
    public var activity_id: String?

    public init(id: String, event_id: String, ts_ms: Int64, kind: String, app_name: String, bundle_id: String, window_title: String, detail: String,
                image_hash: String? = nil, image_width: Int? = nil, image_height: Int? = nil, image_path: String? = nil, ax_evidence_path: String? = nil,
                parent_id: String? = nil, run_id: String, trigger: String? = nil, context: AppContext? = nil, activity_id: String? = nil) {
        self.id = id; self.event_id = event_id; self.ts_ms = ts_ms; self.kind = kind; self.app_name = app_name; self.bundle_id = bundle_id
        self.window_title = window_title; self.detail = detail; self.image_hash = image_hash; self.image_width = image_width
        self.image_height = image_height; self.image_path = image_path; self.ax_evidence_path = ax_evidence_path; self.parent_id = parent_id
        self.run_id = run_id; self.trigger = trigger; self.context = context; self.activity_id = activity_id
    }
}

public struct AppActivity: Codable, Sendable, Equatable {
    public var anchor: AppRecord
    public var latest: AppRecord
    public var start_ms: Int64
    public var end_ms: Int64
    public var sample_count: Int
    public var has_screenshot: Bool
    public var has_issue: Bool
    public var observations: [AppRecord]
    public var observation_offset: Int
    public var screenshots: [AppRecord]
    public var issues: [AppRecord]
}

public struct AppStats: Codable, Sendable, Equatable {
    public var record_count: Int
    public var screenshot_count: Int
    public var storage_bytes: Int64
    /// Screenshot originals stepped down to text only to stay within the storage limit, in total (newer cores).
    public var freed_bytes: Int64?
    public var freed_screenshots: Int?
}

/// What `POST /local/desktop/free-space` did.
public struct ScreenshotSpaceReport: Codable, Sendable, Equatable {
    public var freed_bytes: Int64
    public var screenshots: Int
    public var storage_bytes: Int64
    public var reached_target: Bool
    public var total_freed_bytes: Int64
    public var total_screenshots: Int
}

public struct AppActivities: Codable, Sendable, Equatable {
    public var activities: [AppActivity]
    public var stats: AppStats
}

public struct AppRecognition: Codable, Sendable, Equatable {
    public var event_id: String
    public var source_key: String
    public var method: String
    public var model: String
    public var text: String
    public var completed_ms: Int64
    public var result_path: String?
}

public struct AppRecognitionInfo: Codable, Sendable, Equatable {
    public var screenshot_event_id: String
    public var image_hash: String?
    public var image_path: String?
    public var ax_evidence_path: String?
    public var results: [AppRecognition]
    public var older: [String]
}

/// The record of the switch to direct capture (`POST /local/desktop/cutover`).
public struct CoreCutover: Codable, Sendable, Equatable {
    public var at_ms: Int64
    public var duration_ms: Int64
    public var ledger: String?
    public var records: Int
    public var screenshots: Int
}

extension LocalChannel {
    /// Imports what is left of the old desktop ledger, checks it and switches capture to direct writes; throws
    /// `CoreError.service(409, "cutover_failed", reason)` when the check does not pass. Hashing can take a while.
    public func cutover() async throws -> CoreCutover {
        struct Answer: Decodable { var cutover: CoreCutover }
        let answer: Answer = try await post("/local/desktop/cutover", body: [String: String](), timeout: 900)
        return answer.cutover
    }

    /// Registers (once by kind and name) one of the App's own sources.
    public func registerSource(kind: String, name: String, policy: SourcePolicy) async throws -> String {
        struct Body: Encodable { var kind: String; var display_name: String; var policy: SourcePolicy }
        struct Answer: Decodable { var source_id: String }
        let answer: Answer = try await post("/local/sources", body: Body(kind: kind, display_name: name, policy: policy))
        return answer.source_id
    }

    public func ingest(_ batch: IngestBatch) async throws -> IngestResult {
        try await post("/local/ingest", body: batch, timeout: 120)
    }

    /// Capture preferences an older version kept in the desktop ledger (read-only), or nil.
    public func legacyDesktopPreferences() async throws -> Data? {
        let response = try await send("GET", "/local/desktop/legacy-preferences")
        if response.status == 404 { return nil }
        let body = try response.checked()
        guard let object = try JSONSerialization.jsonObject(with: body) as? [String: Any], let preferences = object["preferences"] else { return nil }
        return try JSONSerialization.data(withJSONObject: preferences)
    }

    /// Deletes a whole desktop activity by its anchor's record ID.
    public func deleteDesktopActivity(_ id: String) async throws -> Int {
        try await deleted("/local/desktop/delete", ["activity_id": .string(id)])
    }

    /// Deletes desktop events by record or event ID; `reason` is `user` or `superseded`.
    public func deleteDesktop(recordIDs: [String] = [], eventIDs: [String] = [], reason: String) async throws -> Int {
        try await deleted("/local/desktop/delete", ["record_ids": .array(recordIDs.map(CoreJSON.string)), "event_ids": .array(eventIDs.map(CoreJSON.string)), "reason": .string(reason)])
    }

    public func deleteDesktopBefore(_ ms: Int64) async throws -> Int {
        try await deleted("/local/desktop/retention", ["before_ms": .int(ms)])
    }

    /// Steps the oldest screenshots (over 7 days old) down to text only until desktop originals use at most
    /// `targetBytes`; the events, OCR text and AX evidence stay.
    public func freeScreenshotSpace(targetBytes: Int64) async throws -> ScreenshotSpaceReport {
        try await post("/local/desktop/free-space", body: ["target_bytes": CoreJSON.int(targetBytes)], timeout: 300)
    }

    private func deleted(_ path: String, _ body: [String: CoreJSON]) async throws -> Int {
        struct Answer: Decodable { var deleted: Int }
        let answer: Answer = try await post(path, body: body, timeout: 120)
        return answer.deleted
    }

    /// One of the App's own read tools.
    public func app<T: Decodable>(_ tool: String, _ arguments: [String: CoreJSON]) async throws -> T {
        try JSONDecoder().decode(T.self, from: try await self.tool(tool, arguments: try JSONEncoder().encode(arguments)))
    }
}
