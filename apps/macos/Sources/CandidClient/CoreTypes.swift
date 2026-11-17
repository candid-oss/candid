import Foundation
import CandidLocalization

/// A registered external client as the core lists it (`GET /local/clients`).
public struct ClientInfo: Codable, Sendable, Equatable, Identifiable {
    public var client_id: String
    public var name: String
    public var transport: String
    public var created_ms: Int64
    public var revoked_ms: Int64?
    public var last_access_ms: Int64?
    public var calls_7d: Int
    public var denied_7d: Int
    public var id: String { client_id }

    public init(client_id: String, name: String, transport: String, created_ms: Int64, revoked_ms: Int64? = nil,
                last_access_ms: Int64? = nil, calls_7d: Int = 0, denied_7d: Int = 0) {
        self.client_id = client_id; self.name = name; self.transport = transport; self.created_ms = created_ms
        self.revoked_ms = revoked_ms; self.last_access_ms = last_access_ms; self.calls_7d = calls_7d; self.denied_7d = denied_7d
    }
}

/// Returned once when a client is registered: the token never appears again.
public struct ClientSetup: Codable, Sendable, Equatable {
    public var client_id: String
    public var name: String
    public var token: String
    public var claude_code: String
    public var codex: String
    public var json: String
}

/// Comparison of the desktop ledger with the timeline (`POST /local/verify`).
public struct CoreVerification: Codable, Sendable, Equatable {
    public var ok: Bool
    public var hashed: Bool
    public var differences: [String]
    public var blobs_checked: Int
    public var at_ms: Int64
}

/// The mirroring state (`GET /local/status`).
public struct CoreStatus: Codable, Sendable, Equatable {
    /// `opening`, `syncing` (first pass) or `ready`.
    public var phase: String
    public var events: Int
    public var last_sync_ms: Int64?
    public var last_change: String?
    public var problems: [String]
    public var verification: CoreVerification?
    public var syncing: Bool
    public var endpoint: String?
    public var root: String?
    /// `ledger` (desktop capture waits for the cutover) or `direct`.
    public var capture_mode: String?
    public var cutover: CoreCutover?
    public var cutover_error: String?
}

/// `GET /health`.
public struct CoreHealth: Codable, Sendable, Equatable {
    public var ok: Bool
    public var version: String
    public var pid: Int32
    public var parent_pid: Int32?
    public var store_id: String
    public var root: String
    public var staging_dir: String
    public var endpoint: String
    public var socket: String
    public var uptime_ms: Int64
    public var subscribers: Int
}

/// One line of the change feed.
public struct CoreEvent: Codable, Sendable, Equatable {
    /// `hello` (just subscribed), `status`, `data` (events added or removed), `clients`.
    public var type: String
    public var seq: UInt64
}

public enum CoreError: Error, LocalizedError, Equatable {
    /// The core answered with an error (`{"error": {"code", "message"}}`).
    case service(status: Int, code: String, message: String)
    /// No core answers on the socket.
    case unreachable(String)
    case badResponse

    public var errorDescription: String? {
        switch self {
        case .service(_, _, let message): message
        case .unreachable(let detail): L("时间线核心没有响应：%@", detail)
        case .badResponse: L("时间线核心的响应无效。")
        }
    }
}
