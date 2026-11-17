import Foundation

public enum ToolCallDetail: String, Codable, Sendable { case none, nameOnly = "name_only" }

/// Per-agent authorisation (`policy_json` of an `agent.transcript` source, §6.2). Every switch defaults to off.
public struct AgentConsent: Codable, Equatable, Sendable {
    public struct Exclude: Codable, Equatable, Sendable {
        /// Working directories; a session whose directory is equal to or below an entry is invisible.
        public var projects: [String] = []
        public var sessions: [String] = []
        public init(projects: [String] = [], sessions: [String] = []) { self.projects = projects; self.sessions = sessions }
    }
    public struct Content: Codable, Equatable, Sendable {
        public var user = true
        public var assistant = true
        public var toolCalls = ToolCallDetail.nameOnly
        public var toolOutput = false
        public var reasoning = false
        public init() {}
        enum CodingKeys: String, CodingKey { case user, assistant, toolCalls = "tool_calls", toolOutput = "tool_output", reasoning }
    }
    public var version = 1
    public var agent: String
    public var read = false
    public var search = false
    /// Writes only user messages and work summaries when read is also enabled.
    public var activity = false
    public var promptMaxChars: Int? = nil
    /// Empty means the format's default paths.
    public var paths: [String] = []
    public var exclude = Exclude()
    public var content = Content()

    public init(agent: String) { self.agent = agent }
    enum CodingKeys: String, CodingKey { case version, agent, read, search, activity, paths, exclude, content; case promptMaxChars = "prompt_max_chars" }
}

public protocol AgentConsentStore: Sendable {
    func consent(for agent: String) -> AgentConsent
    func save(_ consent: AgentConsent) throws
}

public final class InMemoryAgentConsentStore: AgentConsentStore, @unchecked Sendable {
    private let lock = NSLock()
    private var all: [String: AgentConsent] = [:]
    public init() {}
    public func consent(for agent: String) -> AgentConsent { lock.withLock { all[agent] ?? AgentConsent(agent: agent) } }
    public func save(_ consent: AgentConsent) throws { lock.withLock { all[consent.agent] = consent } }
}
