import Foundation
import Testing
import CandidClient
@testable import Candid

/// Explicit acceptance: independently deployed local voice API, public Chinese audio, actual App playback code.
@MainActor @Test(.enabled(if: ProcessInfo.processInfo.environment["CANDID_REAL_WORKFLOW"] == "1"))
func publicVoiceRunsThroughCoreSearchAndActualAppPlayback() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("pw-app-real-" + UUID().uuidString)
    defer { TestCore.finish(root) }
    let channel = try #require(try await TestCore.ready(root).channel)
    func post(_ path: String, _ value: [String: Any]) async throws -> [String: Any] {
        let data = try await channel.send("POST", path, body: JSONSerialization.data(withJSONObject: value)).checked()
        return try #require(JSONSerialization.jsonObject(with: data) as? [String: Any])
    }
    let config: [String: Any] = ["version": 1, "providers": [["provider_id": "public-voice", "name": "Public acceptance", "api": "other", "base_url": "http://127.0.0.1:8767/v1", "destination": "on_device", "enabled": true]], "models": [["provider_id": "public-voice", "model_id": "qwen3-asr"]], "roles": [["role": "asr", "model": ["provider_id": "public-voice", "model_id": "qwen3-asr"]]]]
    _ = try await post("/local/models", ["action": "save", "config": config])
    let sample = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("../../../../data/asr/sample-zh.wav").standardizedFileURL
    let id = UUID().uuidString.lowercased()
    _ = try await post("/local/media", ["action": "import", "id": id, "filename": "PUBLIC.wav", "kind": "audio", "source": "import", "path": sample.path])
    _ = try await post("/local/media", ["action": "recognize", "id": id, "mode": "quick"])
    var item: [String: Any] = [:]
    for _ in 0..<600 {
        item = try await post("/local/media", ["action": "item", "id": id])
        if (item["segments"] as? [[String: Any]])?.isEmpty == false { break }
        try await Task.sleep(for: .milliseconds(100))
    }
    let segment = try #require((item["segments"] as? [[String: Any]])?.first)
    let text = try #require(segment["text"] as? String)
    let hits = try await post("/v1/search", ["q": String(text.prefix(3)), "attention": "all"])
    #expect((hits["hits"] as? [Any])?.isEmpty == false)
    let original = try await post("/local/media", ["action": "content", "id": id])
    let path = try #require(original["path"] as? String)
    let player = PlaybackController()
    defer { player.stop() }
    await player.toggleFile(id: id, startAt: 1.0) { URL(fileURLWithPath: path) }
    #expect(player.error == nil)
    #expect(player.isPlaying)
    #expect(abs(player.position - 1.0) < 0.1)
    #expect(player.duration > 4 && player.duration < 5)
    try await Task.sleep(for: .milliseconds(100))
}
