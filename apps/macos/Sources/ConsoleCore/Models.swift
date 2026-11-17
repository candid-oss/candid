import Foundation

public struct ServiceConfig: Decodable, Sendable {
    let session_token: String
    public let protocol_version: Int
    public let schema_version: Int
    public let max_record_seconds: Int
}

public struct BenchState: Decodable, Sendable {
    public var device: DeviceSnapshot
    public var serial_device: DeviceSnapshot?
    public var serial: SerialInfo?
    public var live_audio: LiveAudio?
    public var assets: [AudioAsset]
    public var transcripts: [Transcript]
    public var asr: ASRStatus
    public var codec_variants: [CodecVariant]?
    public var events: [BenchEvent]?
    public var audioAssets: [AudioAsset] { assets.filter { $0.mime_type == "audio/wav" } }
    public func device(for transport: Transport) -> DeviceSnapshot? {
        transport == .serial ? serial_device : device
    }
}

public enum Transport: String, CaseIterable, Sendable, Identifiable {
    case serial, http
    public var id: String { rawValue }
    public var title: String { self == .serial ? "USB · 服务现有连接" : "Wi-Fi · 服务现有连接" }
}
public struct SerialInfo: Decodable, Sendable { public let path: String; public let baud: Int }
public struct DeviceSnapshot: Decodable, Sendable {
    public var connected: Bool
    public var status: DeviceStatus
    public var last_seen: String?
    public var error: String?
    public var transfer: Transfer?
}
public struct DeviceStatus: Decodable, Sendable {
    public var device_id: String?
    public var firmware_version: String?
    public var capabilities: [String: Bool]?
    public var unavailable: [String: String]?
    public var recording: Recording?
    public var vad: VADStatus?
}
public struct Recording: Decodable, Sendable {
    public var state: String
    public var capture_id: String?
    public var max_seconds: Int?
    public var codec: String?
    public var bitrate: Int?
    public var duration_ms: Double?
    public var started_monotonic_ms: Double?
    public var stop_reason: String?
    public var size: Int?
}
public struct VADStatus: Decodable, Sendable {
    public var enabled: Bool
    public var mode: String
    public var state: String
    public var auto_armed: Bool?
    public var engine: String?
}
public struct Transfer: Decodable, Sendable {
    public var state: String
    public var received: Int
    public var total: Int
}
public struct LiveAudio: Decodable, Sendable { public var frames: [LevelFrame]; public var gap_samples: Int }
public struct LevelFrame: Decodable, Sendable {
    public var device_id: String
    public var boot_id: String
    public var start_sample: Int
    public var buckets: [[Int]]
    public var rms_dbfs: Double
    public var clipped_samples: Int
    public var monotonic_ms: Double
    public var recording: Bool
    public var vad_state: String
    public var received_at: String
    public var gap_samples: Int?
    public var receivedDate: Date? { WireDate.parse(received_at) }
}
public enum WireDate {
    public static func parse(_ value: String) -> Date? {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter.date(from: value) ?? ISO8601DateFormatter().date(from: value)
    }
}
public struct AudioAsset: Decodable, Identifiable, Sendable {
    public let asset_id: String
    public let filename: String
    public let mime_type: String
    public let size: Int
    public let created_at: String
    public let source: String
    public let transfer_hash_verified: Bool
    public let metadata: AudioMetadata
    public var id: String { asset_id }
    public var isSynthetic: Bool { source == "synthetic" }
    public var date: Date? { WireDate.parse(created_at) }
}
public struct AudioMetadata: Decodable, Sendable {
    public let duration_s: Double?
    public let sample_rate: Int?
    public let channels: Int?
    public let peak_dbfs: Double?
    public let rms_dbfs: Double?
    public let clipped_percent: Double?
}
public struct ASRStatus: Decodable, Sendable {
    public let ready: Bool
    public let busy: Bool
    public let provider: String
    public let model: String?
    public let detail: String
}
public struct Transcript: Decodable, Identifiable, Sendable {
    public let job_id: String
    public let asset_id: String
    public let status: String
    public let error: String?
    public let result: TranscriptResult?
    public var id: String { job_id }
}
public struct TranscriptResult: Decodable, Sendable {
    public let text: String?
    public let provider: String
    public let model: String
    public let model_revision: String?
    public let timestamps_available: Bool
}
public struct CodecVariant: Decodable, Sendable, Identifiable {
    public let opus_id: String
    public let decoded_asset_id: String
    public let source: String
    public let bitrate: Int?
    public let size: Int
    public var id: String { opus_id }
}
public struct BenchEvent: Decodable, Sendable {
    public let seq: Int
    public let source: String
    public let type: String
}
public struct CommandRequest: Encodable, Sendable {
    public let request_id: UUID
    public let transport: String
    public let command: String
    public let codec: String
    public let bitrate: Int
    public let max_seconds: Int
    public let mode = "monitor"
    public init(transport: Transport, command: String, codec: String, bitrate: Int, seconds: Int, id: UUID = UUID()) {
        request_id = id; self.transport = transport.rawValue; self.command = command
        self.codec = codec; self.bitrate = bitrate; max_seconds = seconds
    }
}
public struct CommandResult: Decodable, Sendable {
    public let ok: Bool
    public let state: String
    public let error: String?
    public let asset: AudioAsset?
}
public struct LiveMessage: Decodable, Sendable {
    public let type: String
    public let state: BenchState?
    public let serial_device: DeviceSnapshot?
    public let frame: LevelFrame?
    public let gap_samples: Int?
    public let event: BenchEvent?
}
