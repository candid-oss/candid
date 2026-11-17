import Foundation
import Testing
@testable import TimelineConnectors

@Test func batchJSONUsesTheSpecFieldNames() throws {
    let e = ConnectorEvent(sourceKey: "k", kind: "ai.turn", startMs: 1, tzOffsetMin: 0, timeBasis: .source, threadKey: "t",
                           payload: .object(["role": "user"]), text: [ConnectorText(field: "message", content: "x")],
                           blobs: [ConnectorBlobRef(stagingPath: "p", sha256: "h", role: .original)])
    let b = ConnectorBatch(sourceKind: "ai.codex", cursorAfter: ConnectorCursor(stream: "sessions", cursor: "c"),
                           threads: [ConnectorThread(sourceKey: "t", kind: .aiSession, startedMs: 1)], events: [e])
    let v = try JSONDecoder().decode(JSONValue.self, from: JSONEncoder().encode(b))
    let ev = try #require(v["events"]?.arrayValue?.first)
    for k in ["source_key", "kind", "start_ms", "time_basis", "thread_key", "payload_version", "payload", "text", "blobs", "actor", "sensitivity"] {
        #expect(ev[k] != nil, "missing \(k)")
    }
    #expect(ev["blobs"]?.arrayValue?.first?["staging_path"] != nil)
    #expect(v["cursor_after"]?["stream"]?.stringValue == "sessions")
    #expect(v["threads"]?.arrayValue?.first?["kind"]?.stringValue == "ai_session")
    #expect(v["threads"]?.arrayValue?.first?["started_ms"]?.intValue == 1)
    let back = try JSONDecoder().decode(ConnectorBatch.self, from: JSONEncoder().encode(b))
    #expect(back == b)
}

@Test func isoTimeParsing() {
    #expect(ISOTime.parse("1970-01-01T00:00:01.5Z")?.ms == 1500)
    #expect(ISOTime.parse("2026-11-10T10:15:00+08:00")?.ms == ISOTime.parse("2026-11-10T02:15:00Z")?.ms)
    #expect(ISOTime.parse("2026-11-10T10:15:00+08:00")?.offsetMin == 480)
    #expect(ISOTime.parse("2026-11-10T02:15:00Z")?.offsetMin == nil)
    #expect(ISOTime.parse("2026-11-10T02:15:00.123456Z")?.ms == ISOTime.parse("2026-11-10T02:15:00.123Z")?.ms)
    #expect(ISOTime.parse("2026-13-10T02:15:00Z") == nil && ISOTime.parse("nonsense") == nil && ISOTime.parse("") == nil)
}
