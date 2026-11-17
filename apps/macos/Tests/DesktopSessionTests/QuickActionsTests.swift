import Foundation
import Testing
import CandidClient
import TimelineConnectors
@testable import Candid

@MainActor @Test func quickActionsPersistConfigurationAndDetectDuplicateWithoutRegistering() throws {
    let name = "synthetic.hotkeys." + UUID().uuidString
    let defaults = try #require(UserDefaults(suiteName: name))
    defer { defaults.removePersistentDomain(forName: name) }
    let model = GlobalHotkeys(defaults: defaults)
    #expect(model.bindings.isEmpty)
    model.configure(.marker, key: 46)
    model.configure(.note, key: 46)
    #expect(model.errors[QuickAction.note.rawValue] != nil)
    model.configure(.note, key: 45)
    #expect(model.errors.isEmpty)
    #expect(GlobalHotkeys(defaults: defaults).bindings == model.bindings)
    model.configure(.marker, key: nil)
    #expect(model.bindings[QuickAction.marker.rawValue] == nil)
}

private actor SyntheticTimeline {
    var refused = false
    var calls = 0
    func deny() { refused = true }
    func query(_ name: String, _ data: Data) throws -> Data {
        calls += 1
        if refused { throw CoreError.service(status: 403, code: "scope", message: "synthetic revoked") }
        let args = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        if name == "get_event" { return Data(#"{"payload":{"agent":"codex","followups":1},"paragraphs":[{"text":"SYNTHETIC user and followup"}]}"#.utf8) }
        #expect(args["level"] as? String == "event")
        let result = args["cursor"] == nil ? #"{"entries":[{"event":{"event_id":"synthetic-prompt","kind":"ai.prompt","start":"2026-11-12T00:00:00.000Z"}}],"next_cursor":"page2"}"# : #"{"entries":[{"event":{"event_id":"synthetic-work","kind":"ai.work","start":"2026-11-12T00:00:02Z"}}]}"#
        return Data(result.utf8)
    }
}

@MainActor @Test func aiTimelinePagesUsesSourceTimeAndClearsRevokedContent() async throws {
    let transport = SyntheticTimeline()
    let model = AIInteractionTimeline(query: { try await transport.query($0, $1) })
    await model.refresh(day: Date())
    #expect(model.entries.count == 2)
    #expect(model.entries[0].event?.date == Date(timeIntervalSince1970: 1794441600))
    await model.open("synthetic-prompt")
    #expect(model.detail?.paragraphs.first?.text == "SYNTHETIC user and followup")
    await transport.deny()
    await model.refresh(day: Date())
    #expect(model.entries.isEmpty && model.detail == nil && model.reply == nil && model.selected == nil)
    #expect(model.error != nil)
}

@MainActor @Test func quickNoteUsesRealIsolatedCoreAndRemainsQueryable() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("synthetic-quick-note-" + UUID().uuidString)
    defer { TestCore.finish(root) }
    let channel = try #require(try await TestCore.ready(root).channel)
    let model = QuickNote(channel: channel)
    model.text = "SYNTHETIC note for timeline"; model.target = nil
    #expect(await model.save())
    #expect(model.text.isEmpty && model.error == nil)
    let data = try await channel.tool("search", arguments: Data(#"{"q":"SYNTHETIC note","limit":10}"#.utf8))
    let result = String(decoding: data, as: UTF8.self)
    #expect(result.contains("SYNTHETIC note for timeline"))
    #expect(!(await model.save()))
}

@MainActor @Test func aiTimelineReadsRealCoreConsentAndHidesAfterRevocation() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("synthetic-ai-row-" + UUID().uuidString)
    defer { TestCore.finish(root) }
    let channel = try #require(try await TestCore.ready(root).channel)
    let folder = root.appendingPathComponent("synthetic-transcripts")
    try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    let now = Date()
    let stamp = now.ISO8601Format()
    let fixtures: [[String: Any]] = [
        ["timestamp": stamp, "type": "session_meta", "payload": ["id": "synthetic-ui-session", "cwd": "/SYNTHETIC/project"]],
        ["timestamp": stamp, "type": "response_item", "payload": ["type": "message", "id": "u1", "role": "user", "source": "user", "content": [["type": "input_text", "text": "SYNTHETIC authorised instruction"]]]],
        ["timestamp": stamp, "type": "response_item", "payload": ["type": "message", "id": "a1", "role": "assistant", "source": "user", "content": [["type": "output_text", "text": "SYNTHETIC reply read from source"]]]]
    ]
    let bytes = try fixtures.map { String(decoding: try JSONSerialization.data(withJSONObject: $0), as: UTF8.self) }.joined(separator: "\n") + "\n"
    try bytes.write(to: folder.appendingPathComponent("synthetic.jsonl"), atomically: true, encoding: .utf8)
    var consent = TimelineConnectors.AgentConsent(agent: "codex")
    consent.read = true; consent.activity = true; consent.paths = [folder.path]
    let agents = TimelineConnectors.AgentCoreClient(channel: channel)
    try await agents.save(consent)
    try await channel.syncNow()
    let model = AIInteractionTimeline(query: { try await channel.tool($0, arguments: $1) }, reader: { agent, ref, cursor in
        let connector = try #require(AgentConnectors.connector(agent: agent, client: agents))
        return try await connector.read(ref: ref, cursor: cursor, limit: 200)
    })
    for _ in 0..<100 {
        await model.refresh(day: now)
        if !model.entries.isEmpty { break }
        try await Task.sleep(for: .milliseconds(50))
    }
    let prompt = try #require(model.entries.compactMap(\.event).first { $0.kind == "ai.prompt" })
    await model.open(prompt.id)
    #expect(model.detail?.paragraphs.contains { $0.text.contains("SYNTHETIC authorised instruction") } == true)
    await model.readReply()
    #expect(model.reply == "SYNTHETIC reply read from source")
    consent.read = false; consent.activity = false
    try await agents.save(consent)
    await model.refresh(day: now)
    #expect(model.entries.isEmpty && model.detail == nil && model.selected == nil)
}

/// The stored words of an instruction stay on screen when its session file can't be reread; only a note says so.
@MainActor @Test func storedPromptStaysVisibleWhenTheSourceCannotBeReread() async throws {
    let detail = """
    {"event":{"event_id":"p1","kind":"ai.prompt","start":"2026-11-12T10:00:00Z"},
     "payload":{"origin":"transcript","agent":"codex","message_ref":"m1","messages":[{"ref":"m1","source_hash":"h"}]},
     "paragraphs":[{"text":"SYNTHETIC stored prompt"}]}
    """
    let model = AIInteractionTimeline(query: { _, _ in Data(detail.utf8) }, reader: { _, _, _ in throw CoreError.unreachable("synthetic: file moved") })
    await model.open("p1")
    #expect(model.detail?.paragraphs.map(\.text) == ["SYNTHETIC stored prompt"])
    #expect(model.error == nil && model.sourceNote != nil)
}

/// The reply is read page by page from the instruction onward and stops once the next instruction's time is reached.
@MainActor @Test func replyStopsAtTheNextInstructionAndOffersAWayBack() async throws {
    let detail = """
    {"event":{"event_id":"p1","kind":"ai.prompt","start":"2026-11-12T10:00:00Z","end":"2026-11-12T10:05:00Z"},
     "payload":{"origin":"transcript","agent":"claude_code","message_ref":"m1","session_ref":"agent:claude_code:abc-123",
                "project":"/","messages":[{"ref":"m1","source_hash":"h"}]},
     "paragraphs":[{"text":"SYNTHETIC prompt"}]}
    """
    let base: Int64 = 1_794_477_600_000 // 2026-11-12T10:00:00Z
    final class Calls: @unchecked Sendable { var cursors: [String?] = [] }
    let calls = Calls()
    let model = AIInteractionTimeline(query: { _, _ in Data(detail.utf8) }, reader: { _, ref, cursor in
        calls.cursors.append(cursor)
        func page(_ items: [(String, Int64, String, String)], _ next: String?) throws -> AgentReadResult {
            let messages = items.map { ["source_hash": "h", "ref": $0.0, "tsMs": base + $0.1 * 60_000, "role": $0.2, "text": $0.3, "reasoning": false, "truncated": false] as [String: Any] }
            var body: [String: Any] = ["ref": ref, "messages": messages]
            if let next { body["nextCursor"] = next }
            return try JSONDecoder().decode(AgentReadResult.self, from: JSONSerialization.data(withJSONObject: body))
        }
        switch cursor {
        case nil: return try page([("m1", 0, "user", "SYNTHETIC prompt"), ("a1", 1, "assistant", "SYNTHETIC answer")], "c2")
        case "c2": return try page([("m2", 6, "user", "SYNTHETIC next"), ("a2", 7, "assistant", "SYNTHETIC later")], "c3")
        default: Issue.record("read past the next instruction"); return try page([], nil)
        }
    })
    await model.open("p1")
    calls.cursors = []
    await model.readReply()
    #expect(model.reply == "SYNTHETIC answer")
    #expect(calls.cursors.compactMap { $0 } == ["c2"])
    #expect(model.sourceLinks.resumeCommand == "cd '/' && claude --resume abc-123")
    #expect(AIInteractionTimeline.resumeCommand(agent: "codex", sessionRef: "agent:codex:x; rm -rf ~", project: nil) == nil)
    #expect(AIInteractionTimeline.resumeCommand(agent: "codex", sessionRef: "agent:codex:019a-b", project: "/tmp/it's") == "cd '/tmp/it'\\''s' && codex resume 019a-b")
}
