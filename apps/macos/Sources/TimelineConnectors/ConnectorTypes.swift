import Foundation

// MARK: - Batch types (mirror timeline-v1 §6.1 `IngestBatch`; JSON names are snake_case exactly as in the spec)

public enum TimeBasis: String, Sendable, Codable { case observed, source, imported, estimated }
public enum EventActor: String, Sendable, Codable { case `self`, other, agent, system, unknown }
public enum Sensitivity: String, Sendable, Codable { case normal, sensitive, protected }
public enum ThreadKind: String, Sendable, Codable {
    case aiSession = "ai_session", mailThread = "mail_thread", meeting, recording
    case calendarSeries = "calendar_series", chat
}
public enum BlobRole: String, Sendable, Codable { case original, preview, derived }

public struct ConnectorCursor: Sendable, Hashable, Codable {
    public var stream: String
    /// Opaque to everyone except the connector that produced it.
    public var cursor: String
    public init(stream: String, cursor: String) { self.stream = stream; self.cursor = cursor }
}

public struct ConnectorThread: Sendable, Hashable, Codable {
    public var sourceKey: String
    public var kind: ThreadKind
    public var title: String?
    public var startedMs: Int64?
    public var endedMs: Int64?
    public var participants: [JSONValue]
    public var payload: JSONValue
    enum CodingKeys: String, CodingKey {
        case sourceKey = "source_key", kind, title, startedMs = "started_ms", endedMs = "ended_ms"
        case participants = "participants_json", payload = "payload_json"
    }
    public init(sourceKey: String, kind: ThreadKind, title: String? = nil, startedMs: Int64? = nil,
                endedMs: Int64? = nil, participants: [JSONValue] = [], payload: JSONValue = .object([:])) {
        self.sourceKey = sourceKey; self.kind = kind; self.title = title; self.startedMs = startedMs
        self.endedMs = endedMs; self.participants = participants; self.payload = payload
    }
}

public struct ConnectorText: Sendable, Hashable, Codable {
    /// One of the `event_text.field` values: body, selection, title, transcript, ocr, vlm, message, subject, note.
    public var field: String
    public var content: String
    public init(field: String, content: String) { self.field = field; self.content = content }
}

public struct ConnectorBlobRef: Sendable, Hashable, Codable {
    public var stagingPath: String
    public var sha256: String
    public var role: BlobRole
    enum CodingKeys: String, CodingKey { case stagingPath = "staging_path", sha256, role }
    public init(stagingPath: String, sha256: String, role: BlobRole) {
        self.stagingPath = stagingPath; self.sha256 = sha256; self.role = role
    }
}

public struct ConnectorEvent: Sendable, Hashable, Codable {
    public var sourceKey: String
    public var kind: String
    public var startMs: Int64?
    public var endMs: Int64?
    public var tzOffsetMin: Int?
    public var timeBasis: TimeBasis
    public var timeUncertaintyMs: Int64?
    public var appBundleId: String?
    public var appName: String?
    public var title: String?
    public var url: String?
    public var threadKey: String?
    public var parentSourceKey: String?
    public var actor: EventActor
    public var sensitivity: Sensitivity
    public var payloadVersion: Int
    public var payload: JSONValue
    public var text: [ConnectorText]
    public var blobs: [ConnectorBlobRef]
    enum CodingKeys: String, CodingKey {
        case sourceKey = "source_key", kind, startMs = "start_ms", endMs = "end_ms", tzOffsetMin = "tz_offset_min"
        case timeBasis = "time_basis", timeUncertaintyMs = "time_uncertainty_ms", appBundleId = "app_bundle_id"
        case appName = "app_name", title, url, threadKey = "thread_key", parentSourceKey = "parent_source_key"
        case actor, sensitivity, payloadVersion = "payload_version", payload, text, blobs
    }
    public init(sourceKey: String, kind: String, startMs: Int64?, endMs: Int64? = nil, tzOffsetMin: Int? = nil,
                timeBasis: TimeBasis, timeUncertaintyMs: Int64? = nil, appBundleId: String? = nil,
                appName: String? = nil, title: String? = nil, url: String? = nil, threadKey: String? = nil,
                parentSourceKey: String? = nil, actor: EventActor = .self, sensitivity: Sensitivity = .normal,
                payloadVersion: Int = 1, payload: JSONValue = .object([:]), text: [ConnectorText] = [],
                blobs: [ConnectorBlobRef] = []) {
        self.sourceKey = sourceKey; self.kind = kind; self.startMs = startMs; self.endMs = endMs
        self.tzOffsetMin = tzOffsetMin; self.timeBasis = timeBasis; self.timeUncertaintyMs = timeUncertaintyMs
        self.appBundleId = appBundleId; self.appName = appName; self.title = title; self.url = url
        self.threadKey = threadKey; self.parentSourceKey = parentSourceKey; self.actor = actor
        self.sensitivity = sensitivity; self.payloadVersion = payloadVersion; self.payload = payload
        self.text = text; self.blobs = blobs
    }
}

/// Neutral stand-in for `IngestBatch` (`source_id` is assigned by the integrator, who knows the registered source).
public struct ConnectorBatch: Sendable, Hashable, Codable {
    public var sourceKind: String
    public var cursorAfter: ConnectorCursor
    public var threads: [ConnectorThread]
    public var events: [ConnectorEvent]
    /// `source_key`s that disappeared from the source inside the synced window (calendar deletions).
    /// Not part of §6.1; the integrator may map them to tombstones or ignore them.
    public var removedSourceKeys: [String]
    enum CodingKeys: String, CodingKey {
        case sourceKind = "source_kind", cursorAfter = "cursor_after", threads, events
        case removedSourceKeys = "removed_source_keys"
    }
    public init(sourceKind: String, cursorAfter: ConnectorCursor, threads: [ConnectorThread] = [],
                events: [ConnectorEvent] = [], removedSourceKeys: [String] = []) {
        self.sourceKind = sourceKind; self.cursorAfter = cursorAfter; self.threads = threads
        self.events = events; self.removedSourceKeys = removedSourceKeys
    }
}

// MARK: - Policy

public struct ConnectorPolicy: Sendable, Hashable, Codable {
    public struct Exclude: Sendable, Hashable, Codable {
        public var apps: [String]
        /// Thread (session) keys that are never read.
        public var threads: [String]
        /// Case-insensitive substrings matched against the working directory / calendar name.
        public var patterns: [String]
        public init(apps: [String] = [], threads: [String] = [], patterns: [String] = []) {
            self.apps = apps; self.threads = threads; self.patterns = patterns
        }
    }
    public var version: Int
    public var storeText: Bool
    public var storeMedia: Bool
    public var agentVisible: Bool
    public var retentionDays: Int?
    public var exclude: Exclude
    enum CodingKeys: String, CodingKey {
        case version, storeText = "store_text", storeMedia = "store_media", agentVisible = "agent_visible"
        case retentionDays = "retention_days", exclude
    }
    public init(version: Int = 1, storeText: Bool = true, storeMedia: Bool = false, agentVisible: Bool = true,
                retentionDays: Int? = nil, exclude: Exclude = .init()) {
        self.version = version; self.storeText = storeText; self.storeMedia = storeMedia
        self.agentVisible = agentVisible; self.retentionDays = retentionDays; self.exclude = exclude
    }
    public var json: String { JSONCodec.encode(self) }
}

// MARK: - Protocol

public enum ConnectorAuthState: String, Sendable, Codable { case notDetermined, authorized, denied, restricted, revoked }

public struct ConnectorDescription: Sendable, Hashable {
    public var kind: String                // sources.kind
    public var displayName: String
    public var requiredPermissions: [String]
    public var eventKinds: [String]
    public var threadKinds: [ThreadKind]
    public var defaultPolicy: ConnectorPolicy
    public var defaultPolicyJSON: String { defaultPolicy.json }
}

public struct AuthorizationResult: Sendable, Hashable {
    public var state: ConnectorAuthState
    /// Keychain reference for connectors that hold credentials. Local connectors return nil.
    public var secretRef: String?
}

public struct SyncOptions: Sendable {
    /// Stop after about this many events (at a safe boundary) and report `hasMore`. Lets callers bound memory and resume.
    public var maxEvents: Int?
    public init(maxEvents: Int? = nil) { self.maxEvents = maxEvents }
}

/// A problem that stopped one stream/file at its old cursor. Never contains source content.
public struct ConnectorIssue: Sendable, Hashable, Codable {
    public var stream: String
    public var location: String       // file path relative to the root, or calendar id
    public var line: Int?
    public var code: String           // finite set, see `ConnectorIssue.Code`
    public enum Code {
        public static let invalidEnvelope = "invalid_envelope"
        public static let unsupportedShape = "unsupported_shape"
        public static let malformedJSON = "malformed_json"
        public static let fileRewritten = "file_rewritten"
        public static let unreadable = "unreadable"
        public static let accessDenied = "access_denied"
    }
    public init(stream: String, location: String, line: Int? = nil, code: String) {
        self.stream = stream; self.location = location; self.line = line; self.code = code
    }
}

public struct SyncStats: Sendable, Hashable {
    public var linesRead = 0
    public var filesRead = 0
    public var filesSkipped = 0
    public var ignoredTypes: [String: Int] = [:]
    public var skippedInjected = 0
    public init() {}
    mutating func merge(_ o: SyncStats) {
        linesRead += o.linesRead; filesRead += o.filesRead; filesSkipped += o.filesSkipped
        skippedInjected += o.skippedInjected
        for (k, v) in o.ignoredTypes { ignoredTypes[k, default: 0] += v }
    }
}

public struct SyncResult: Sendable {
    public var batches: [ConnectorBatch]
    /// Latest cursor per stream after all returned batches; persist only after the batches are committed.
    public var cursors: [String: String]
    public var issues: [ConnectorIssue]
    public var hasMore: Bool
    public var stats: SyncStats
}

public enum ConnectorError: Error, Sendable, Equatable {
    case revoked
    case notAuthorized(ConnectorAuthState)
    case invalidCursor(stream: String)
    case missingConfiguration(String)
}

public protocol Connector: Sendable {
    func describe() -> ConnectorDescription
    func authorize() async throws -> AuthorizationResult
    /// History for a user-chosen range; resumable via the same `cursors` as `sync`.
    func backfill(range: DateInterval, cursors: [String: String], options: SyncOptions) async throws -> SyncResult
    func sync(cursors: [String: String], options: SyncOptions) async throws -> SyncResult
    /// Stops all future reads. Deleting stored data is the store's job.
    func revoke() async
}
