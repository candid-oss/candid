import Foundation
import Testing
@testable import Candid

private let peopleFixture = Data(#"{"people":[{"id":"SYNTHETIC-1","name":"合成人物","sample_count":3}],"cleanup_message":null}"#.utf8)

@MainActor @Test func voiceLibraryListsPeopleAndSendsConfirmedManagement() async {
    var sent: [[String: Any]] = []
    let model = VoiceLibraryModel(transport: { body in sent.append(body); return peopleFixture })
    await model.refresh()
    #expect(model.people.map(\.name) == ["合成人物"])
    await model.manage("delete", fields: ["person_id": "SYNTHETIC-1"])
    #expect(sent.last?["action"] as? String == "delete")
    #expect(sent.last?["confirmed"] as? Bool == true)
    #expect(!model.busy)
}

@MainActor @Test func voiceLibraryFailureKeepsThePeopleAndCanRetry() async {
    var attempts = 0
    let model = VoiceLibraryModel(transport: { _ in
        attempts += 1
        if attempts == 2 { throw URLError(.cannotConnectToHost) }
        return peopleFixture
    })
    await model.refresh()
    await model.refresh()
    #expect(model.error != nil)
    #expect(model.people.count == 1)
    await model.refresh()
    #expect(model.error == nil)
}

/// A recording's speakers decode with status and score; lines show the resolved name.
@Test func recordingSpeakersDecodeStatusAndConfidence() throws {
    let json = #"""
    {"id":"6f1c2c6e-1a2b-4c3d-8e9f-0a1b2c3d4e5f","filename":"合成.wav","kind":"audio","source":"import","occurred_at":null,
     "imported_at":"2026-11-13T09:00:00Z","processing_status":"ready","error":null,"processing_mode":"conversation",
     "segments":[{"start_seconds":0,"end_seconds":1,"text":"合成","speaker":"说话人 1","identity":null,"speaker_name":"张伟","speaker_status":"auto"},
                 {"start_seconds":1,"end_seconds":2,"text":"合成","speaker":"说话人 2","identity":null,"speaker_name":null,"speaker_status":"maybe"}],
     "speakers":[{"key":"说话人 1","status":"auto","name":"张伟","person_id":null,"voice_id":"v1","score":0.864,"margin":0.3,"threshold":0.65,"segments":1,"seconds":1.0},
                 {"key":"说话人 2","status":"maybe","name":"李娜","person_id":null,"voice_id":"v2","score":0.68,"margin":0.03,"threshold":0.65,"segments":1,"seconds":1.0}]}
    """#
    let decoder = JSONDecoder(); decoder.keyDecodingStrategy = .convertFromSnakeCase
    decoder.dateDecodingStrategy = .iso8601
    let record = try decoder.decode(MediaLibraryRecord.self, from: Data(json.utf8))
    #expect(record.speakers?.map(\.status) == [.auto, .maybe])
    #expect(record.speakers?.first?.percent == 86)
    #expect(record.speakers?.last?.voiceId == "v2")
    #expect(record.segments.map(\.shownSpeaker) == ["张伟", "说话人 2"])
}
