import Foundation
import CandidClient

/// Wire adapter only. Rust owns source access, parsing, consent, redaction, checkpoints and revisions.
public struct AgentCoreClient: Sendable {
    public typealias Transport = @Sendable (_ path: String, _ body: Data?) async throws -> Data
    private let transport: Transport
    public init(transport: @escaping Transport) { self.transport = transport }
    public init(channel: LocalChannel) {
        transport = { path, body in try await channel.send(body == nil ? "GET" : "POST", path, body: body).checked() }
    }
    public struct Settings: Decodable, Sendable {
        public var consents: [AgentConsent]
        public var stored_counts: [String: Int]
    }
    public func settings() async throws -> Settings { try JSONDecoder().decode(Settings.self, from: await transport("/local/agents", nil)) }
    public func consents() async throws -> [AgentConsent] { try await settings().consents }
    public func save(_ consent: AgentConsent, deleteStored: Bool = false) async throws {
        struct Save: Encodable { var consent: AgentConsent; var delete_stored: Bool }
        _ = try await transport("/local/agents/consent", JSONEncoder().encode(Save(consent: consent, delete_stored: deleteStored)))
    }
    fileprivate func call<T: Decodable>(_ type: T.Type, _ arguments: [String: JSONValue]) async throws -> T {
        try JSONDecoder().decode(type, from: await transport("/local/agents/query", JSONEncoder().encode(arguments)))
    }
}

public enum AgentConnectors {
    public static func all(client: AgentCoreClient) -> [any AgentTranscriptConnector] {
        ["codex", "claude_code", "pi"].map { RustAgentConnector(agent: $0, client: client) }
    }
    public static func connector(agent: String, client: AgentCoreClient) -> (any AgentTranscriptConnector)? {
        all(client: client).first { $0.describe().agent == agent }
    }
}

private struct RustAgentConnector: AgentTranscriptConnector {
    let agent: String
    let client: AgentCoreClient
    func describe() -> AgentDescription {
        let values: (String, String, String) = switch agent {
        case "codex": ("Codex", "~/.codex/sessions", "codex-rollout/1")
        case "claude_code": ("Claude Code", "~/.claude/projects", "claude-code-jsonl/1")
        default: ("pi", "~/.pi/agent/sessions", "pi-session/3")
        }
        return AgentDescription(agent: agent, displayName: values.0, defaultPaths: [values.1], formatVersions: [values.2])
    }
    private func arguments(_ action: String) -> [String: JSONValue] { ["agent": .string(agent), "action": .string(action)] }
    func detect() async -> AgentDetection {
        (try? await client.call(AgentDetection.self, arguments("detect"))) ?? AgentDetection(agent: agent, state: .notFound, formatVersion: nil, headInspected: false)
    }
    func sessions(range: DateInterval, project: String?) async throws -> AgentSessionList {
        var a = ranged("sessions", range, project)
        a["limit"] = .int(500)
        return try await client.call(AgentSessionList.self, a)
    }
    func read(ref: String, cursor: String?, limit: Int) async throws -> AgentReadResult {
        var a = arguments("read"); a["reference"] = .string(ref); a["limit"] = .int(Int64(limit))
        if let cursor { a["cursor"] = .string(cursor) }
        return try await client.call(AgentReadResult.self, a)
    }
    func search(query: String, range: DateInterval, project: String?, limit: Int, deadlineMs: Int) async throws -> AgentSearchResult {
        var a = ranged("search", range, project); a["query"] = .string(query); a["limit"] = .int(Int64(limit)); a["deadline_ms"] = .int(Int64(deadlineMs))
        return try await client.call(AgentSearchResult.self, a)
    }
    func usage(range: DateInterval) async throws -> AgentUsage { try await client.call(AgentUsage.self, ranged("usage", range, nil)) }
    private func ranged(_ action: String, _ range: DateInterval, _ project: String?) -> [String: JSONValue] {
        var a = arguments(action)
        a["start_ms"] = .int(Int64(range.start.timeIntervalSince1970 * 1000)); a["end_ms"] = .int(Int64(range.end.timeIntervalSince1970 * 1000))
        if let project { a["project"] = .string(project) }
        return a
    }
}
