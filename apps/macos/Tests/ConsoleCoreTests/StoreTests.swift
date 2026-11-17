import Foundation
import CryptoKit
import Testing
@testable import ConsoleCore

private actor ControlledService: ConsoleService {
    nonisolated let endpoint = "test-only"
    nonisolated let isMock = true
    let mock = MockService()
    var fail = false
    var writes = 0
    var reads = 0
    var customState: BenchState?
    func setState(_ value: BenchState) { customState = value }
    func setOffline(_ value: Bool) { fail = value }
    func configure() async throws -> ServiceConfig { if fail { throw URLError(.cannotConnectToHost) }; return try await mock.configure() }
    func state() async throws -> BenchState { reads += 1; if fail { throw URLError(.cannotConnectToHost) }; if let customState { return customState }; return try await mock.state() }
    func command(_ request: CommandRequest) async throws -> CommandResult { writes += 1; return try await mock.command(request) }
    func pauseMonitor() async throws { writes += 1; try await mock.pauseMonitor() }
    func transcribe(assetID: String) async throws { writes += 1; try await mock.transcribe(assetID: assetID) }
    func media(assetID: String) async throws -> Data { try await mock.media(assetID: assetID) }
    func messages() async -> AsyncThrowingStream<LiveMessage, Error> {
        // Model a failed WebSocket: the controller must reconnect via read-only requests.
        AsyncThrowingStream { $0.finish(throwing: URLError(.networkConnectionLost)) }
    }
}

@Suite(.serialized) @MainActor
struct StoreTests {
    @Test func startupReadsOnlyAndOfflineRecoveryNeverCaptures() async throws {
        let service = ControlledService()
        let observed = ConsoleStore(service: service)
        let task = Task { await observed.run() }
        // Wait for the first read instead of a fixed delay: slower machines (CI) take longer to start the loop.
        let deadline = Date().addingTimeInterval(5)
        while await service.reads == 0, Date() < deadline { try await Task.sleep(for: .milliseconds(20)) }
        #expect(await service.reads > 0); #expect(await service.writes == 0)
        task.cancel(); await task.value
        #expect(observed.connection == .offline && !observed.canStart && !observed.canStop)
        await service.setOffline(true); await observed.refresh()
        #expect(observed.connection == .offline); #expect(!observed.canStart)
        await observed.command("record.start"); #expect(await service.writes == 0)
        await service.setOffline(false); await observed.refresh()
        #expect(observed.connection == .online); #expect(observed.canStart)
        #expect(await service.writes == 0)
    }
    @Test func manualRecordAndExplicitTranscriptionRemainSeparate() async throws {
        let service = ControlledService(), store = ConsoleStore(service: service)
        await store.refresh()
        #expect(store.recordings.first?.isSynthetic == true)
        #expect(store.snapshot?.transcripts.isEmpty == true)
        await store.command("record.start")
        #expect(store.recordingState == "recording"); #expect(!store.canStart); #expect(store.canStop)
        #expect(store.snapshot?.serial_device?.status.vad?.enabled == false)
        await store.command("record.stop")
        #expect(store.recordingState == "ready")
        #expect(store.snapshot?.transcripts.isEmpty == true)
        #expect(await service.writes == 2)
        await store.transcribe()
        #expect(store.snapshot?.transcripts.count == 1); #expect(await service.writes == 3)
    }
    @Test func monitorToggleDoesNotStartRecording() async throws {
        let service = ControlledService(), store = ConsoleStore(service: service)
        await store.refresh(); await store.toggleMonitor()
        #expect(store.device?.status.vad?.enabled == true); #expect(store.recordingState == "idle")
        await store.command("record.start"); #expect(store.device?.status.vad?.enabled == true)
        await store.toggleMonitor(); #expect(store.device?.status.vad?.enabled == false)
        #expect(store.recordingState == "recording")
    }
    @Test func oldTelemetryCannotClaimLiveSpeech() async throws {
        let mock = MockService(), store = ConsoleStore(service: mock)
        await store.refresh(); await store.toggleMonitor()
        // No fresh frames: reporting the monitor enabled must not imply speech or silence.
        #expect(store.vadLabel(at: .now) == "等待新鲜采样")
        #expect(store.freshFrame(at: .now) == nil)
    }
    @Test func staleOrWrongDeviceSamplesDoNotClaimLiveSpeech() async throws {
        let service = ControlledService(), store = ConsoleStore(service: service)
        var state = try await service.mock.state()
        let now = Date()
        let frame = LevelFrame(device_id: "MOCK-ESP32", boot_id: "test", start_sample: 0, buckets: [[-100, 100]], rms_dbfs: -30, clipped_samples: 0, monotonic_ms: 0, recording: false, vad_state: "speech", received_at: ISO8601DateFormatter().string(from: now), gap_samples: 0)
        state.serial_device?.status.vad?.enabled = true
        state.live_audio = LiveAudio(frames: [frame], gap_samples: 0)
        await service.setState(state); await store.refresh()
        #expect(store.vadLabel(at: now) == "检测到话音")
        #expect(store.vadLabel(at: now.addingTimeInterval(3)) == "等待新鲜采样")
        state.serial_device?.status.device_id = "another-device"
        await service.setState(state); await store.refresh()
        #expect(store.freshFrame(at: now) == nil)
    }
    @Test func encodingTransferAutoModeAndMissingCapabilitiesBlockStart() async throws {
        let service = ControlledService(), store = ConsoleStore(service: service)
        let baseline = try await service.mock.state()
        for variant in 0..<4 {
            var state = baseline
            if variant == 0 { state.serial_device?.status.recording?.state = "encoding" }
            if variant == 1 { state.serial_device?.transfer = Transfer(state: "receiving", received: 10, total: 100) }
            if variant == 2 { state.serial_device?.status.vad?.auto_armed = true }
            if variant == 3 { state.serial_device?.status.capabilities?["audio"] = false }
            await service.setState(state); await store.refresh()
            #expect(!store.canStart)
            await store.command("record.start")
        }
        #expect(await service.writes == 0)
    }
    @Test func syntheticWAVHeaderIsValid() async throws {
        let wav = SyntheticTone.wav()
        #expect(wav.count == 64044)
        let sample = try await MockService().state().audioAssets.first
        #expect(sample?.id == SHA256.hash(data: wav).map { String(format: "%02x", $0) }.joined())
        #expect(String(data: wav.prefix(4), encoding: .ascii) == "RIFF")
        #expect(String(data: wav[8..<12], encoding: .ascii) == "WAVE")
    }
}
