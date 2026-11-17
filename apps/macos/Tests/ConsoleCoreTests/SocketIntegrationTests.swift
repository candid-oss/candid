import Foundation
import Testing
@testable import ConsoleCore

@Suite(.serialized)
struct SocketIntegrationTests {
    @Test(.enabled(if: ProcessInfo.processInfo.environment["MAC_CONSOLE_FIXTURE_URL"] != nil))
    func actualHTTPAndWebSocketContracts() async throws {
        let endpoint = try #require(ProcessInfo.processInfo.environment["MAC_CONSOLE_FIXTURE_URL"])
        let api = try DebugClient(endpoint: endpoint)
        _ = try await api.configure()
        let initial = try await api.state()
        #expect(initial.serial_device?.connected == true)
        let stream = await api.messages()
        var iterator = stream.makeAsyncIterator()
        let first = try await iterator.next()
        #expect(first?.type == "snapshot"); #expect(first?.state?.assets.first?.isSynthetic == true)
        _ = try await api.command(CommandRequest(transport: .serial, command: "vad.start", codec: "wav", bitrate: 24000, seconds: 15))
        var frame: LevelFrame?
        for _ in 0..<8 {
            let message = try await iterator.next()
            if message?.type == "audio.levels" { frame = message?.frame; break }
        }
        #expect(frame?.device_id == "MOCK-ESP32"); #expect(frame?.buckets.count == 5)
        try await api.pauseMonitor()
        _ = try await api.command(CommandRequest(transport: .serial, command: "record.start", codec: "opus", bitrate: 24000, seconds: 3))
        #expect(try await api.state().serial_device?.status.recording?.state == "recording")
        let stopped = try await api.command(CommandRequest(transport: .serial, command: "record.stop", codec: "opus", bitrate: 24000, seconds: 3))
        let asset = try #require(stopped.asset)
        let data = try await api.media(assetID: asset.id)
        #expect(data.count == 64044)
        try await api.transcribe(assetID: asset.id)
        #expect(try await api.state().transcripts.first?.result?.provider == "mock")
    }
    @Test(.enabled(if: ProcessInfo.processInfo.environment["MAC_CONSOLE_FIXTURE_URL"] != nil)) @MainActor
    func disconnectAndReconnectWithoutReplayingCapture() async throws {
        let endpoint = try #require(ProcessInfo.processInfo.environment["MAC_CONSOLE_FIXTURE_URL"])
        let api = try DebugClient(endpoint: endpoint), store = ConsoleStore(service: try DebugClient(endpoint: endpoint))
        let before = try await stats(endpoint)
        let task = Task { await store.run() }
        defer { task.cancel() }
        try await wait { store.connection == .online }
        try await offline(endpoint, true)
        try await wait { store.connection != .online }
        #expect(!store.canStart); #expect(store.snapshot != nil)
        try await offline(endpoint, false)
        try await wait { store.connection == .online }
        let after = try await stats(endpoint)
        #expect(after.writes == before.writes)
        #expect(after.sockets >= before.sockets + 2)
        #expect(try await api.state().serial_device?.connected == true)
        task.cancel(); await task.value
    }
}
private struct Stats: Decodable { let writes: Int; let sockets: Int }
private func stats(_ endpoint: String) async throws -> Stats {
    let (data, _) = try await URLSession.shared.data(from: URL(string: endpoint + "/__test/stats")!)
    return try JSONDecoder().decode(Stats.self, from: data)
}
private func offline(_ endpoint: String, _ offline: Bool) async throws {
    var request = URLRequest(url: URL(string: endpoint + "/__test/offline")!)
    request.httpMethod = "POST"; request.httpBody = try JSONEncoder().encode(["offline": offline])
    _ = try await URLSession.shared.data(for: request)
}
@MainActor private func wait(_ condition: () -> Bool) async throws {
    let deadline = Date().addingTimeInterval(8)
    while !condition() && Date() < deadline { try await Task.sleep(for: .milliseconds(100)) }
    #expect(condition())
}
