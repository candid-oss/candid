import Foundation
import Testing
import CandidClient
@testable import Candid

/// Synthetic integration regression: typing during an in-flight save must not lose the newer draft.
@MainActor @Test func editingDuringNoteSaveRetainsNewDraftAndStoresOriginalSnapshot() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("synthetic-note-inflight-" + UUID().uuidString)
    defer { TestCore.finish(root) }
    let channel = try #require(try await TestCore.ready(root).channel)
    let source = try await channel.registerSource(kind: "manual", name: "Synthetic targets", policy: .init(store_text: true, store_media: false))
    var target = IngestEvent(source_key: "synthetic-first", kind: "note", start_ms: Int64(Date().timeIntervalSince1970 * 1000))
    target.text = [IngestText(field: "body", content: "SYNTHETIC target object first")]
    var secondTarget = target; secondTarget.source_key = "synthetic-second"
    secondTarget.text = [IngestText(field: "body", content: "SYNTHETIC target object second")]
    _ = try await channel.ingest(.init(source_id: source, events: [target, secondTarget]))
    struct Hit: Decodable { var event_id: String }; struct Search: Decodable { var hits: [Hit] }
    let data = try await channel.tool("search", arguments: Data(#"{"q":"target object first"}"#.utf8))
    let id = try #require(JSONDecoder().decode(Search.self, from: data).hits.first?.event_id)
    let reference = "candid://event/" + id
    let secondData = try await channel.tool("search", arguments: Data(#"{"q":"target object second"}"#.utf8))
    let secondID = try #require(JSONDecoder().decode(Search.self, from: secondData).hits.first?.event_id)
    let secondReference = "candid://event/" + secondID
    #expect(secondID != id)
    let model = QuickNote(channel: channel)
    model.text = "SYNTHETIC original snapshot"
    model.target = reference
    var finished = false
    let saving = Task { @MainActor in
        let result = await model.save()
        finished = true
        return result
    }
    while !model.saving && !finished { await Task.yield() }
    try #require(model.saving && !finished)
    model.text = "SYNTHETIC newer unsaved draft"
    model.target = secondReference
    #expect(await saving.value)
    #expect(model.text == "SYNTHETIC newer unsaved draft")
    let first = try await channel.tool("search", arguments: Data(#"{"q":"original snapshot"}"#.utf8))
    #expect(String(decoding: first, as: UTF8.self).contains("SYNTHETIC original snapshot"))
    let before = try await channel.tool("search", arguments: Data(#"{"q":"newer unsaved draft"}"#.utf8))
    #expect(!String(decoding: before, as: UTF8.self).contains("SYNTHETIC newer unsaved draft"))
    #expect(await model.save())
    #expect(model.text.isEmpty)
    let second = try await channel.tool("search", arguments: Data(#"{"q":"newer unsaved draft"}"#.utf8))
    #expect(String(decoding: second, as: UTF8.self).contains("SYNTHETIC newer unsaved draft"))
}

@MainActor @Test func missingNoteTargetRefusesSaveAndRetainsEditableDraft() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("synthetic-note-missing-" + UUID().uuidString)
    defer { TestCore.finish(root) }
    let channel = try #require(try await TestCore.ready(root).channel)
    let model = QuickNote(channel: channel)
    model.target = "candid://event/synthetic-missing"
    model.text = "SYNTHETIC retained draft"
    #expect(!(await model.save()))
    #expect(model.text == "SYNTHETIC retained draft" && model.error != nil)
    let result = try await channel.tool("search", arguments: Data(#"{"q":"retained draft"}"#.utf8))
    #expect(!String(decoding: result, as: UTF8.self).contains("SYNTHETIC retained draft"))
}
