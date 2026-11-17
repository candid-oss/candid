import DesktopCore
import Foundation
import CandidClient
import Testing

private func folder() -> URL {
    FileManager.default.temporaryDirectory.appendingPathComponent("desktop-outbox-synthetic-" + UUID().uuidString, isDirectory: true)
}

private func event(_ key: String, run: String = "run", activity: String? = nil) throws -> IngestEvent {
    try DesktopEventMapping.event(for: DesktopRecord(id: key, timestamp: Date(timeIntervalSince1970: 1_800_000_000), kind: .marker,
                                                     windowTitle: "合成标记", runID: run, activityID: activity)).event
}

@Suite struct DesktopOutboxTests {
    @Test func entriesKeepTheirOrderAndFilesAndSurviveReopening() async throws {
        let dir = folder(); defer { try? FileManager.default.removeItem(at: dir) }
        let outbox = try DesktopOutbox(directory: dir)
        let image = Data([1, 2, 3])
        let sha = DesktopEventMapping.sha256(image)
        try await outbox.append(.ingest([try event("a")]))
        try await outbox.append(.ingest([try event("b")]), files: [sha: (image, "jpg")])
        try await outbox.append(.retention(beforeMs: 42))
        let reopened = try DesktopOutbox(directory: dir)
        let entries = await reopened.peek(10)
        #expect(entries.map(\.command) == [.ingest([try event("a")]), .ingest([try event("b")]), .retention(beforeMs: 42)])
        #expect(try Data(contentsOf: try #require(entries[1].files[sha])) == image)
        await reopened.remove([entries[0].seq, entries[1].seq])
        #expect(await reopened.count == 1)
        #expect(try FileManager.default.contentsOfDirectory(atPath: dir.path).count == 1, "the entry's file went with it")
        // New entries continue after the highest sequence number.
        let next = try await reopened.append(.deleteActivity("x"))
        #expect(next > entries[2].seq)
    }

    @Test func aCrashBeforeCommitLeavesNothingBehind() async throws {
        let dir = folder(); defer { try? FileManager.default.removeItem(at: dir) }
        _ = try DesktopOutbox(directory: dir)
        // What an interrupted append leaves: a half-written entry and a file without an entry.
        try Data("{".utf8).write(to: dir.appendingPathComponent("0000000000000007.json.part"))
        try Data([9]).write(to: dir.appendingPathComponent("0000000000000007-" + String(repeating: "a", count: 64) + ".jpg"))
        let outbox = try DesktopOutbox(directory: dir)
        #expect(await outbox.count == 0)
        #expect(try FileManager.default.contentsOfDirectory(atPath: dir.path).isEmpty)
    }

    @Test func aFullOutboxRefusesAndAStaleGateWritesNothing() async throws {
        let dir = folder(); defer { try? FileManager.default.removeItem(at: dir) }
        let outbox = try DesktopOutbox(directory: dir, limits: .init(items: 2, bytes: 1024))
        try await outbox.append(.ingest([try event("a")]))
        await #expect(throws: DesktopStoreError.self) { try await outbox.append(.ingest([try event("b")]), files: [String(repeating: "b", count: 64): (Data(count: 2048), "jpg")]) }
        try await outbox.append(.ingest([try event("b")]))
        await #expect(throws: DesktopStoreError.outboxFull(2)) { try await outbox.append(.ingest([try event("c")])) }
        let gate = CaptureCommitGate(); let epoch = gate.enable(); gate.invalidate()
        let other = try DesktopOutbox(directory: folder())
        defer { try? FileManager.default.removeItem(at: other.directory) }
        await #expect(throws: CaptureAdmissionError.self) { try await other.append(.ingest([try event("late")]), gate: gate, epoch: epoch) }
        #expect(await other.count == 0)
        #expect(try FileManager.default.contentsOfDirectory(atPath: other.directory.path).isEmpty)
    }

    @Test func deletingAnActivityDropsItsQueuedRecordsOnly() async throws {
        let dir = folder(); defer { try? FileManager.default.removeItem(at: dir) }
        let outbox = try DesktopOutbox(directory: dir)
        try await outbox.append(.ingest([try event("a1", activity: "A")]))
        try await outbox.append(.ingest([try event("b1", activity: "B")]))
        try await outbox.append(.ingest([try event("a2", activity: "A")]))
        await outbox.dropActivity(runID: "run", activityID: "A")
        #expect(await outbox.peek(10).map(\.command) == [.ingest([try event("b1", activity: "B")])])
    }
}

/// The capture's own mapping must equal what the core's ledger import makes of the same records
/// (`core/fixtures/desktop-mapping.json`, written by the core's test).
@Suite struct DesktopEventMappingTests {
    @Test func mappingMatchesTheCoreGoldenFile() throws {
        let file = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("../../../../core/fixtures/desktop-mapping.json")
        let golden = try JSONSerialization.jsonObject(with: Data(contentsOf: file)) as! [String: Any]
        let cases = golden["cases"] as! [[String: Any]]
        #expect(cases.count >= 12)
        for item in cases {
            let record = try JSONDecoder().decode(DesktopRecord.self, from: JSONSerialization.data(withJSONObject: item["record"]!))
            var (event, _) = try DesktopEventMapping.event(for: record)
            for i in event.blobs.indices { event.blobs[i].staging_path = "<staged>" }
            // The import links a parent only when it exists; the capture leaves that to the core (`detach`).
            let expected = item["event"] as! [String: Any]
            if expected["parent_key"] == nil { event.parent_key = nil }
            let mine = try JSONSerialization.jsonObject(with: JSONEncoder().encode(event)) as! NSDictionary
            #expect(mine == expected as NSDictionary, "\(record.id)")
        }
    }

    @Test func recordsReadBackFromTheCoreKeepTheirFields() {
        let a = AppRecord(id: "r1", event_id: "e1", ts_ms: 1_800_000_000_500, kind: "screenshot", app_name: "Notes", bundle_id: "com.example.notes",
                          window_title: "合成", detail: "定时触发", image_hash: String(repeating: "a", count: 64), image_width: 2, image_height: 3,
                          image_path: "/tmp/x.jpg", ax_evidence_path: nil, parent_id: "p1", run_id: "run", trigger: "timer",
                          context: AppContext(focused_role: "AXTextArea", readable_text: "正文", is_protected: nil), activity_id: "act")
        let r = DesktopEventMapping.record(a)
        #expect(r.id == "r1" && r.kind == .screenshot && r.parentID == "p1" && r.activityID == "act" && r.imageWidth == 2)
        #expect(r.timestamp == Date(timeIntervalSince1970: 1_800_000_000.5))
        #expect(r.context?.readableText == "正文" && r.context?.focusedRole == "AXTextArea")
        #expect(DesktopEventMapping.milliseconds(r.timestamp) == a.ts_ms)
    }
}
