import Foundation
import Testing
import CandidClient
@testable import TimelineConnectors

private final class SyntheticCore: @unchecked Sendable {
    let dir = URL(fileURLWithPath: "/tmp/pv-agent-" + UUID().uuidString.prefix(8))
    let process = Process()
    var channel: LocalChannel { LocalChannel(socketPath: dir.appendingPathComponent("run/core.sock").path) }
    var client: AgentCoreClient { AgentCoreClient(channel: channel) }
    init() throws {
        guard let bin = ProcessInfo.processInfo.environment["CANDID_CORE_BIN"] else { throw CoreError.unreachable("Run script/test.sh to build the isolated Rust helper") }
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        process.executableURL = URL(fileURLWithPath: bin)
        process.arguments = ["serve", "--root", dir.appendingPathComponent("db").path, "--run-dir", dir.appendingPathComponent("run").path, "--socket", channel.socketPath, "--sync-interval", "1"]
        process.standardOutput = FileHandle.nullDevice; process.standardError = FileHandle.nullDevice
        try process.run()
    }
    func ready() async throws {
        for _ in 0..<200 { if (try? await channel.health(timeout: 0.2)) != nil { return }; try await Task.sleep(for: .milliseconds(20)) }
        throw CoreError.unreachable("Synthetic helper did not start")
    }
    deinit { if process.isRunning { process.terminate(); process.waitUntilExit() }; try? FileManager.default.removeItem(at: dir) }
}

@Test func rustConnectorBridgeReadsSearchesAndSynchronisesOnlyAfterExplicitConsent() async throws {
    let core = try SyntheticCore(); try await core.ready()
    let initial = try await core.client.consents()
    #expect(initial.count == 3 && initial.allSatisfy { !$0.read && !$0.search && !$0.activity })
    let source = core.dir.appendingPathComponent("SYNTHETIC-transcripts")
    try FileManager.default.createDirectory(at: source, withIntermediateDirectories: true)
    let lines: [[String: Any]] = [
        ["timestamp": "2026-11-11T01:00:00Z", "type": "session_meta", "payload": ["id": "SYNTHETIC", "cwd": "/SYNTHETIC/project"]],
        ["timestamp": "2026-11-11T01:00:01Z", "type": "response_item", "payload": ["type": "message", "id": "u1", "role": "user", "source": "user", "content": [["type": "input_text", "text": "SYNTHETIC pineapple prompt"]]]],
        ["timestamp": "2026-11-11T01:00:02Z", "type": "response_item", "payload": ["type": "message", "id": "a1", "role": "assistant", "content": [["type": "output_text", "text": "SYNTHETIC pineapple reply"]]]]
    ]
    let data = try lines.reduce(into: Data()) { data, line in data.append(try JSONSerialization.data(withJSONObject: line)); data.append(0x0a) }
    try data.write(to: source.appendingPathComponent("rollout-SYNTHETIC.jsonl"))
    var consent = AgentConsent(agent: "codex"); consent.paths = [source.path]
    try await core.client.save(consent)
    let connector = try #require(AgentConnectors.connector(agent: "codex", client: core.client))
    let detection = await connector.detect()
    #expect(detection.state == .found && !detection.headInspected)
    await #expect(throws: (any Error).self) { try await connector.read(ref: "agent:codex:SYNTHETIC", cursor: nil, limit: 20) }
    consent.read = true; consent.search = true; consent.activity = true
    try await core.client.save(consent)
    let range = DateInterval(start: Date(timeIntervalSince1970: 0), end: Date(timeIntervalSince1970: 4_000_000_000))
    let sessions = try await connector.sessions(range: range, project: nil)
    #expect(sessions.sessions.count == 1 && sessions.sessions[0].ref == "agent:codex:SYNTHETIC")
    let read = try await connector.read(ref: "agent:codex:SYNTHETIC", cursor: nil, limit: 1)
    #expect(read.messages.first?.text == "SYNTHETIC pineapple prompt" && read.nextCursor != nil)
    let next = try await connector.read(ref: "agent:codex:SYNTHETIC", cursor: read.nextCursor, limit: 1)
    #expect(next.messages.first?.role == "assistant" && !next.stored)
    let hits = try await connector.search(query: "pineapple", range: range, project: nil, limit: 20, deadlineMs: 1000)
    #expect(hits.hits.count == 2 && !hits.stored)
    _ = try await connector.usage(range: range)
    var observedPrompt = false
    for _ in 0..<100 {
        let raw = try await core.channel.tool("timeline", arguments: Data(#"{"start":"2026-11-11","end":"2026-11-12","level":"event"}"#.utf8))
        let value = try JSONSerialization.jsonObject(with: raw) as? [String: Any]
        if let events = value?["entries"] as? [[String: Any]], events.contains(where: { ($0["event"] as? [String: Any])?["kind"] as? String == "ai.prompt" }) { observedPrompt = true; break }
        try await Task.sleep(for: .milliseconds(20))
    }
    #expect(observedPrompt, "The Rust scheduler must admit the synthetic prompt")
    consent.read = false; consent.search = false; consent.activity = false
    try await core.client.save(consent, deleteStored: true)
    await #expect(throws: (any Error).self) { try await connector.read(ref: "agent:codex:SYNTHETIC", cursor: nil, limit: 20) }
    let saved = try await core.client.consents()
    #expect(saved.first(where: { $0.agent == "codex" })?.activity == false)
}

@Test func activityLimitIsOptionalAndSerialisesSeparatelyFromPublicArtifacts() throws {
    var consent = AgentConsent(agent: "pi")
    #expect(consent.promptMaxChars == nil)
    consent.promptMaxChars = 1200
    let data = try JSONEncoder().encode(consent)
    let object = try #require(JSONSerialization.jsonObject(with: data) as? [String: Any])
    #expect(object["prompt_max_chars"] as? Int == 1200)
    #expect(try JSONDecoder().decode(AgentConsent.self, from: data) == consent)
}
