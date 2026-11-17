import Foundation

/// An isolated demonstration. No sockets, device access, or model calls.
public actor MockService: ConsoleService {
    public nonisolated let endpoint = "隔离 Mock · 无设备与网络访问"
    public nonisolated let isMock = true
    private var current: BenchState
    private var started: Date?
    public init() {
        let url = Bundle.module.url(forResource: "MockState", withExtension: "json")!
        current = try! JSONDecoder().decode(BenchState.self, from: Data(contentsOf: url))
    }
    public func configure() async throws -> ServiceConfig {
        try JSONDecoder().decode(ServiceConfig.self, from: Data(#"{"session_token":"mock-only","protocol_version":1,"schema_version":1,"max_record_seconds":15}"#.utf8))
    }
    public func state() async throws -> BenchState {
        if let started, Date().timeIntervalSince(started) >= Double(current.serial_device?.status.recording?.max_seconds ?? 15) {
            self.started = nil
            current.serial_device?.status.recording?.state = "ready"
            current.serial_device?.status.recording?.stop_reason = "max_duration"
        }
        return current
    }
    public func command(_ request: CommandRequest) async throws -> CommandResult {
        switch request.command {
        case "record.start":
            started = Date()
            current.serial_device?.status.recording = Recording(state: "recording", capture_id: "mock-capture", max_seconds: request.max_seconds, duration_ms: 0, started_monotonic_ms: 0)
        case "record.stop":
            current.serial_device?.status.recording?.duration_ms = min(15000, (started.map { Date().timeIntervalSince($0) } ?? 2) * 1000)
            started = nil; current.serial_device?.status.recording?.state = "ready"
        case "vad.start": current.serial_device?.status.vad?.enabled = true
        case "vad.stop": current.serial_device?.status.vad?.enabled = false
        default: throw ConsoleError.rejected("Mock 不支持该命令")
        }
        return CommandResult(ok: true, state: request.command == "record.stop" ? "verified" : "recording", error: nil, asset: request.command == "record.stop" ? current.audioAssets.first : nil)
    }
    public func pauseMonitor() async throws { current.serial_device?.status.vad?.enabled = false }
    public func transcribe(assetID: String) async throws {
        let result = TranscriptResult(text: "这是隔离 Mock 的合成文字，用于验证点击转写与结果展示。", provider: "mock", model: "Mock · 无模型调用", model_revision: nil, timestamps_available: false)
        current.transcripts.insert(Transcript(job_id: UUID().uuidString, asset_id: assetID, status: "completed", error: nil, result: result), at: 0)
    }
    public func media(assetID: String) async throws -> Data { SyntheticTone.wav() }
    public func messages() async -> AsyncThrowingStream<LiveMessage, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                while !Task.isCancelled {
                    do {
                        let state = try await self.state()
                        continuation.yield(LiveMessage(type: "snapshot", state: state, serial_device: nil, frame: nil, gap_samples: nil, event: nil))
                        try await Task.sleep(for: .milliseconds(250))
                    } catch { break }
                }
                continuation.finish()
            }
            continuation.onTermination = { @Sendable _ in task.cancel() }
        }
    }
}

public enum SyntheticTone {
    /// Explicitly synthetic, two seconds of 440 Hz PCM, 16 kHz mono.
    public static func wav() -> Data {
        var data = Data()
        func ascii(_ value: String) { data.append(contentsOf: value.utf8) }
        func integer<T: FixedWidthInteger>(_ value: T) {
            var little = value.littleEndian
            withUnsafeBytes(of: &little) { data.append(contentsOf: $0) }
        }
        ascii("RIFF"); integer(UInt32(64036)); ascii("WAVEfmt "); integer(UInt32(16))
        integer(UInt16(1)); integer(UInt16(1)); integer(UInt32(16000)); integer(UInt32(32000))
        integer(UInt16(2)); integer(UInt16(16)); ascii("data"); integer(UInt32(64000))
        for index in 0..<32000 { integer(Int16(sin(Double(index) * 2 * .pi * 440 / 16000) * 6000)) }
        return data
    }
}
