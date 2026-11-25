import Foundation
import Testing
import DesktopCore
import CandidClient
@testable import Candid

@MainActor @Test func fragmentEditingUsesCoreAndPreservesNewerDraftDuringSave() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("synthetic-fragment-" + UUID().uuidString)
    defer { TestCore.finish(root) }
    let channel = try #require(try await TestCore.ready(root).channel)
    let raw = DesktopRecord(id: "synthetic-fragment", kind: .context, appName: "Synthetic", bundleID: "example.synthetic", windowTitle: "Original window", runID: "synthetic-run", context: .init(readableText: "SYNTHETIC original words"))
    try await writeDirectly(root, [raw])
    let model = FragmentEditModel(target: .init(activities: [raw.id]), channel: channel)
    await model.load()
    #expect(model.error == nil && model.automaticTitle)
    model.title = "人工片段"; model.notes = "SYNTHETIC saved annotation"; model.automaticTitle = false; model.automaticNotes = false
    var finished = false
    let saving = Task { @MainActor in let result = await model.save(); finished = true; return result }
    while !model.saving && !finished { await Task.yield() }
    try #require(model.saving && !finished)
    model.notes = "SYNTHETIC newer draft"
    #expect(!(await saving.value)) // keep the sheet and the newer draft open
    #expect(model.notes == "SYNTHETIC newer draft")
    let read = FragmentEditModel(target: .init(activities: [raw.id]), channel: channel)
    await read.load()
    #expect(read.title == "人工片段" && read.notes == "SYNTHETIC saved annotation")
    #expect(await model.save())
    await read.load(); #expect(read.notes == "SYNTHETIC newer draft")
    let records = try await storedRecords(root)
    #expect(records.first?.windowTitle == "Original window")
    #expect(records.first?.manualTitle == "人工片段")
    #expect(records.first?.context?.readableText == "SYNTHETIC original words")
    read.clear(); #expect(await read.save())
    await model.load(); #expect(model.title.isEmpty && !model.automaticTitle)
    model.reset(); #expect(await model.save())
    await read.load(); #expect(read.automaticTitle && read.automaticNotes)
    read.notes = String(repeating: "x", count: 16001); read.automaticNotes = false
    #expect(!(await read.save())); #expect(read.notes.count == 16001 && read.error != nil)
    read.notes = "SYNTHETIC retry after rejection"; #expect(await read.save())
    await model.load(); #expect(model.notes == "SYNTHETIC retry after rejection")
    let missing = FragmentEditModel(target: .init(activities: ["missing"]), channel: channel)
    await missing.load(); missing.notes = "retained"
    #expect(!(await missing.save())); #expect(missing.notes == "retained" && missing.error != nil)
}

@MainActor @Test func screenshotEditMapsExactCoreIdentityAndKeepsActivitySeparate() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("synthetic-shot-edit-" + UUID().uuidString)
    defer { TestCore.finish(root) }
    let channel = try #require(try await TestCore.ready(root).channel)
    let source = try await DesktopDelivery.registerSource(channel)
    let now = Int64(Date().timeIntervalSince1970 * 1000)
    var anchor = IngestEvent(source_key: "synthetic-anchor", kind: "window.focus", start_ms: now)
    anchor.title = "Original title"; anchor.app_bundle_id = "example.synthetic"; anchor.payload = ["run_id": .string("synthetic")]
    var shot = IngestEvent(source_key: "synthetic-shot", kind: "screen.shot", start_ms: now + 1)
    shot.parent_key = anchor.source_key; shot.payload = anchor.payload
    _ = try await channel.ingest(.init(source_id: source, events: [anchor, shot]))
    let model = FragmentEditModel(target: .init(activities: [anchor.source_key], screenshotID: shot.source_key), channel: channel)
    await model.load(); try #require(model.error == nil)
    model.title = "截图名称"; model.automaticTitle = false
    #expect(await model.save())
    struct Answer: Decodable { var activity: AppActivity? }
    let before: Answer = try await channel.app("app_activity", ["id": .string(anchor.source_key)])
    let shotEventID = try #require(before.activity?.screenshots.first?.event_id)
    var recognition = IngestEvent(source_key: "synthetic-ocr", kind: "image.text", start_ms: now + 2)
    recognition.parent_key = shot.source_key; recognition.payload = ["method": .string("ocr"), "image_event_id": .string(shotEventID)]
    recognition.text = [IngestText(field: "ocr", content: "SYNTHETIC recognized text")]
    _ = try await channel.ingest(.init(source_id: source, events: [recognition]))
    let answer: Answer = try await channel.app("app_activity", ["id": .string(anchor.source_key)])
    #expect(answer.activity?.anchor.manual_title == nil)
    #expect(answer.activity?.screenshots.first?.manual_title == "截图名称")
}

@MainActor @Test func timelineMediaSearchMatchesCoreTranscriptNotesAndTitleWithoutUnrelatedCards() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("synthetic-timeline-search-" + UUID().uuidString)
    defer { TestCore.finish(root) }
    let channel = try #require(try await TestCore.ready(root).channel)
    // This fixture supplies its own quick transcript; don't race the default conversation worker.
    _ = try await channel.send("POST", "/local/workflows", body: JSONSerialization.data(withJSONObject: ["action": "settings", "settings": ["version": 1, "automatic": false, "titles": false, "summary": false]])).checked()
    let client = MediaLibraryClient(channel: channel)
    let wav = root.appendingPathComponent("SYNTHETIC.wav")
    try (RecordingJournal.wavHeader(frames: 8000, rate: 8000) + Data(repeating: 0, count: 16000)).write(to: wav)
    let audioID = UUID()
    _ = try await client.upload(.init(id: audioID, filename: "SYNTHETIC.wav", storedName: "SYNTHETIC.wav", kind: "audio", source: "import", occurredAt: Date(), needsReview: false), file: wav)
    _ = try await client.edit(audioID, title: "sharedneedle audio", notes: "noteneedle")
    struct Job: Decodable { var id: String; var attempt: Int }
    // The recording's one transcription job, answered as a local voice service would (quick mode, text only).
    let models: [String: Any] = ["version": 1, "providers": [["provider_id": "voice", "name": "SYNTHETIC", "api": "other", "base_url": "http://127.0.0.1:1/v1", "destination": "on_device", "enabled": true]],
                                 "models": [["provider_id": "voice", "model_id": "SYNTHETIC"]], "roles": [["role": "asr", "model": ["provider_id": "voice", "model_id": "SYNTHETIC"]]]]
    _ = try await channel.send("POST", "/local/models", body: JSONSerialization.data(withJSONObject: ["action": "save", "config": models])).checked()
    _ = try await channel.send("POST", "/local/media", body: JSONSerialization.data(withJSONObject: ["action": "recognize", "id": audioID.uuidString.lowercased(), "mode": "quick"])).checked()
    let job: Job = try await channel.post("/local/jobs", body: ["action": CoreJSON.string("claim"), "owner": .string("SYNTHETIC"), "kind": .string("speech.recording")])
    let payload: [String: Any] = ["action": "result", "format": "candid-recording-result/1", "job_id": job.id, "attempt": job.attempt, "owner": "SYNTHETIC",
                                  "result": ["speaker_count": 0, "unresolved_segments": 0, "producer": ["model_id": "SYNTHETIC", "model_revision": "fixture/1"],
                                             "segments": [["start_seconds": 0.0, "end_seconds": 0.5, "text": "transcriptonlyneedle"]]]]
    _ = try await channel.send("POST", "/local/media", body: JSONSerialization.data(withJSONObject: payload)).checked()

    let png = root.appendingPathComponent("SYNTHETIC.png")
    try Data(base64Encoded: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/a9sAAAAASUVORK5CYII=")!.write(to: png)
    let imageID = UUID()
    _ = try await client.upload(.init(id: imageID, filename: "SYNTHETIC.png", storedName: "SYNTHETIC.png", kind: "image", source: "import", occurredAt: Date(), needsReview: false), file: png)
    _ = try await client.edit(imageID, title: "sharedneedle image", notes: "pictureonly")
    let desktop = DesktopRecord(id: "synthetic-search-desktop", kind: .context, appName: "Synthetic", bundleID: "example.synthetic", windowTitle: "sharedneedle desktop", runID: "synthetic-search", context: .init(readableText: "SYNTHETIC desktop body"))
    try await writeDirectly(root, [desktop])
    let desktopMatches: AppActivities = try await channel.app("app_activities", ["start_ms": .int(0), "end_ms": .int(Int64(Date().timeIntervalSince1970 * 1000) + 10000), "q": .string("sharedneedle")])
    #expect(desktopMatches.activities.count == 1)
    let search = TimelineMediaSearch()
    await search.refresh(query: "sharedneedle", media: nil, client: client)
    #expect(Set(search.items.map(\.id)) == Set([audioID, imageID]))
    await search.refresh(query: "noteneedle", media: nil, client: client)
    #expect(search.items.map(\.id) == [audioID])
    await search.refresh(query: "pictureonly", media: nil, client: client)
    #expect(search.items.map(\.id) == [imageID])
    await search.refresh(query: "transcriptonlyneedle", media: nil, client: client)
    #expect(search.items.map(\.id) == [audioID])
    await search.refresh(query: "NATIVE_QA_OK", media: nil, client: client)
    #expect(search.items.isEmpty)
    let desktopMisses: AppActivities = try await channel.app("app_activities", ["start_ms": .int(0), "end_ms": .int(Int64(Date().timeIntervalSince1970 * 1000) + 10000), "q": .string("NATIVE_QA_OK")])
    #expect(desktopMisses.activities.isEmpty)
    #expect(search.records(query: "different pending request", media: nil).isEmpty)
}
