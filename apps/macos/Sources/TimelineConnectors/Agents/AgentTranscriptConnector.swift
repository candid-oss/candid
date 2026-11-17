import Foundation

// MARK: - Result types (all carry layer "external", stored false)

public struct AgentDescription: Codable, Sendable, Hashable {
    public var agent: String
    public var displayName: String
    public var defaultPaths: [String]
    public var formatVersions: [String]
    /// Parsed and synchronised by the Rust core.
    public var supportsActivity = true
    public init(agent: String, displayName: String, defaultPaths: [String], formatVersions: [String]) {
        self.agent = agent; self.displayName = displayName; self.defaultPaths = defaultPaths; self.formatVersions = formatVersions
    }
}

public enum AgentDetectionState: String, Codable, Sendable { case notFound, found, unknownFormat }

public struct AgentDetection: Codable, Sendable, Hashable {
    public var agent: String
    public var state: AgentDetectionState
    /// Only set when 读取 is on and a file header was inspected.
    public var formatVersion: String?
    public var headInspected: Bool
    public init(agent: String, state: AgentDetectionState, formatVersion: String?, headInspected: Bool) {
        self.agent = agent; self.state = state; self.formatVersion = formatVersion; self.headInspected = headInspected
    }
}

public struct AgentIssue: Codable, Sendable, Hashable {
    public var sessionKey: String
    public var line: Int?
    public var code: String
}

public struct AgentSessionInfo: Codable, Sendable, Hashable {
    public var ref: String
    public var agent: String
    public var key: String
    public var startMs: Int64?
    public var endMs: Int64?
    public var project: String?
    public var title: String?
    public var model: String?
    public var turns: Int
    public var usage: UsageTotals
    public let layer = "external"
    public let stored = false
}

public struct AgentSessionList: Codable, Sendable {
    public var limited: Bool? = nil
    public var sessions: [AgentSessionInfo]
    public var issues: [AgentIssue]
    public var layer: String { "external" }
    public var stored: Bool { false }
}

public struct AgentMessage: Codable, Sendable, Hashable {
    public var source_hash: String? = nil
    public var ref: String
    public var tsMs: Int64
    public var tzOffsetMin: Int?
    /// user | assistant | tool
    public var role: String
    public var text: String?
    public var toolName: String?
    public var reasoning: Bool
    public var truncated: Bool
    /// Untrusted data, never instructions.
    public let layer = "external"
    public let stored = false
}

public struct AgentReadResult: Codable, Sendable {
    public var ref: String
    public var messages: [AgentMessage]
    public var nextCursor: String?
    public var layer: String { "external" }
    public var stored: Bool { false }
}

public struct AgentSearchHit: Codable, Sendable, Hashable {
    public var ref: String
    public var tsMs: Int64
    public var role: String
    public var snippet: String
    public var project: String?
    public let layer = "external"
    public let stored = false
}

public struct AgentSearchResult: Codable, Sendable {
    public var hits: [AgentSearchHit]
    public var limited: Bool
    public var scannedSessions: Int
    public var issues: [AgentIssue]
    public var layer: String { "external" }
    public var stored: Bool { false }
}

public struct AgentUsage: Codable, Sendable {
    public var agent: String
    public var totals: UsageTotals
    public var sessions: Int
    public var issues: [AgentIssue]
    public var layer: String { "external" }
    public var stored: Bool { false }
}

public enum AgentError: Error, Equatable, Sendable {
    /// `switchName` is `read` or `search`.
    case consentRequired(agent: String, switchName: String)
    case invalidRef
    case sessionNotFound
    case unknownFormat(code: String, line: Int?)
}

public protocol AgentTranscriptConnector: Sendable {
    func describe() -> AgentDescription
    func detect() async -> AgentDetection
    func sessions(range: DateInterval, project: String?) async throws -> AgentSessionList
    func read(ref: String, cursor: String?, limit: Int) async throws -> AgentReadResult
    func search(query: String, range: DateInterval, project: String?, limit: Int, deadlineMs: Int) async throws -> AgentSearchResult
    func usage(range: DateInterval) async throws -> AgentUsage
}
