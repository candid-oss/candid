import DesktopCore
import Foundation
import CandidClient
import Testing
import TimelineConnectors
@testable import Candid

/// Actual source formats in isolated files, not a real-user-source acceptance.
@MainActor @Test(arguments: ["codex", "claude_code", "pi"]) func nativeProofReadsIndexedFullUnicodeTailAndRevocation(agent: String) async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("synthetic-native-proof-" + UUID().uuidString)
    defer { TestCore.finish(root) }
    let channel = try #require(try await TestCore.ready(root).channel)
    let folder = root.appendingPathComponent("synthetic-transcripts")
    try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    let at = Date(timeIntervalSince1970: floor(Date().timeIntervalSince1970) - 2)
    let full = "SYNTHETIC 原话 👩🏽‍💻\n" + String(repeating: "中", count: 2000) + "最终尾部"
    let rows: [[String: Any]]
    switch agent {
    case "codex": rows = [
        ["timestamp": at.ISO8601Format(), "type": "session_meta", "payload": ["id": "synthetic-native-session", "cwd": "/SYNTHETIC/project"]],
        ["timestamp": at.ISO8601Format(), "type": "response_item", "payload": ["type": "message", "id": "u1", "role": "user", "source": "user", "content": [["type": "input_text", "text": full]]]]
    ]
    case "claude_code": rows = [
        ["type": "custom-title", "sessionId": "synthetic-native-session", "customTitle": "SYNTHETIC"],
        ["type": "user", "uuid": "u1", "sessionId": "synthetic-native-session", "timestamp": at.ISO8601Format(), "cwd": "/SYNTHETIC/project", "message": ["role": "user", "content": full]]
    ]
    default: rows = [
        ["type": "session", "version": 3, "id": "synthetic-native-session", "timestamp": at.ISO8601Format(), "cwd": "/SYNTHETIC/project"],
        ["type": "message", "id": "u1", "timestamp": at.ISO8601Format(), "message": ["role": "user", "content": full]]
    ]
    }
    let data = try rows.map { try JSONSerialization.data(withJSONObject: $0) }.reduce(into: Data()) { $0.append($1); $0.append(10) }
    try data.write(to: folder.appendingPathComponent("synthetic.jsonl"))
    let agents = AgentCoreClient(channel: channel)
    var consent = AgentConsent(agent: agent); consent.paths = [folder.path]; consent.read = true; consent.activity = true
    try await agents.save(consent); try await channel.syncNow()
    var messages: [DesktopNativeSendProof.Message] = []
    for _ in 0..<100 {
        messages = try await DesktopNativeSendProof.messages(channel: channel, since: at.addingTimeInterval(-1), now: Date())
        if !messages.isEmpty { break }
        try await Task.sleep(for: .milliseconds(50))
    }
    let message = try #require(messages.first)
    #expect(message.text == full)
    let model = AIInteractionTimeline(query: { try await channel.tool($0, arguments: $1) }, reader: { name, ref, cursor in
        let connector = try #require(AgentConnectors.connector(agent: name, client: agents))
        return try await connector.read(ref: ref, cursor: cursor, limit: 200)
    })
    await model.refresh(day: Date())
    let prompt = try #require(model.entries.compactMap(\.event).first { $0.kind == "ai.prompt" })
    await model.open(prompt.id)
    #expect(model.detail?.paragraphs.map(\.text) == [full])
    var context = DesktopContextSnapshot()
    context.recordEvidence = .init(object: .init(key: message.session, basis: "url"), field: "synthetic-window:composer", inputValue: full, inputStatus: "captured")
    let draft = DesktopRecord(timestamp: at.addingTimeInterval(-0.1), kind: .context, bundleID: "com.openai.codex", context: context)
    let proof = try #require(DesktopNativeSendProof.confirmation(draft: draft, messages: messages))
    let source = try await DesktopDelivery.registerSource(channel)
    let event = try DesktopEventMapping.event(for: proof).event
    let first = try await channel.ingest(IngestBatch(source_id: source, events: [event]))
    let retry = try await channel.ingest(IngestBatch(source_id: source, events: [event]))
    #expect(first.inserted == 1 && retry.duplicates == 1)
    consent.read = false; consent.activity = false
    try await agents.save(consent)
    #expect(try await DesktopNativeSendProof.messages(channel: channel, since: at.addingTimeInterval(-1), now: Date()).isEmpty)
}

private actor DelayedSourceReply {
    var continuation: CheckedContinuation<AgentReadResult, Never>?
    var waiting: Bool { continuation != nil }
    func read() async -> AgentReadResult { await withCheckedContinuation { continuation = $0 } }
    func finish() throws {
        let value = try JSONDecoder().decode(AgentReadResult.self, from: Data(#"{"ref":"agent:codex:test","messages":[{"ref":"agent:codex:test#a","tsMs":1800000000000,"role":"assistant","text":"LATE PRIVATE REPLY","reasoning":false,"truncated":false}]}"#.utf8))
        continuation?.resume(returning: value); continuation = nil
    }
}
@MainActor @Test func aiReplyCannotRepopulateClearedViewAfterLateSourceRead() async throws {
    let source = DelayedSourceReply()
    let model = AIInteractionTimeline(query: { _, _ in Data(#"{"payload":{"agent":"codex","session_ref":"agent:codex:test"},"paragraphs":[]}"#.utf8) }, reader: { _, _, _ in await source.read() })
    model.selected = "event"
    let task = Task { await model.readReply() }
    while !(await source.waiting) { await Task.yield() }
    model.clear()
    try await source.finish(); await task.value
    #expect(model.reply == nil && model.detail == nil && model.selected == nil)
}
