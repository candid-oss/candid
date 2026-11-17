import Foundation
import CandidLocalization
import CandidShared

public enum ConsoleError: LocalizedError, Sendable {
    case invalidEndpoint, incompatible, http(Int, String), rejected(String), invalidMedia
    public var errorDescription: String? {
        switch self {
        case .invalidEndpoint: L("仅接受本机回环 HTTP 服务地址，且不能包含凭据、路径或查询参数。")
        case .incompatible: L("服务协议或 schema 版本不兼容；请核对调试服务版本。")
        case let .http(code, detail): L("%@（HTTP %@）", detail, code)
        case let .rejected(detail): detail
        case .invalidMedia: L("录音 ID、媒体类型或大小不符合回听要求。")
        }
    }
}

public protocol ConsoleService: Sendable {
    var endpoint: String { get }
    var isMock: Bool { get }
    func configure() async throws -> ServiceConfig
    func state() async throws -> BenchState
    func command(_ request: CommandRequest) async throws -> CommandResult
    func pauseMonitor() async throws
    func transcribe(assetID: String) async throws
    func media(assetID: String) async throws -> Data
    func messages() async -> AsyncThrowingStream<LiveMessage, Error>
    func importAudio(data: Data, filename: String) async throws -> AudioAsset
}

public extension ConsoleService {
    func importAudio(data: Data, filename: String) async throws -> AudioAsset {
        throw ConsoleError.rejected(L("当前连接暂不支持文件导入"))
    }
}

public actor DebugClient: ConsoleService {
    public nonisolated let endpoint: String
    public nonisolated let isMock = false
    private let base: URL
    private let sessions: ProxyAwareSession?
    private let fixedSession: URLSession?
    /// Follows 设置 → 模型 → 网络代理 unless a session was injected.
    private var session: URLSession { fixedSession ?? sessions!.current }
    private var token: String?

    public init(endpoint: String = "http://127.0.0.1:8766", session: URLSession? = nil) throws {
        guard let url = URL(string: endpoint), let parts = URLComponents(url: url, resolvingAgainstBaseURL: false),
              parts.scheme == "http", ["127.0.0.1", "localhost", "[::1]", "::1"].contains(parts.host ?? ""),
              parts.user == nil, parts.password == nil, parts.query == nil, parts.fragment == nil,
              parts.path.isEmpty || parts.path == "/" else { throw ConsoleError.invalidEndpoint }
        base = url; self.endpoint = endpoint
        if let session { sessions = nil; fixedSession = session }
        else {
            let config = URLSessionConfiguration.ephemeral
            config.timeoutIntervalForRequest = 10
            config.timeoutIntervalForResource = 120
            config.urlCache = nil
            config.httpCookieStorage = nil
            sessions = ProxyAwareSession(configuration: config, delegate: NoRedirects())
            fixedSession = nil
        }
    }
    public func configure() async throws -> ServiceConfig {
        token = nil
        let config: ServiceConfig = try await get("/api/config")
        guard config.protocol_version == 1, config.schema_version == 1, !config.session_token.isEmpty else { throw ConsoleError.incompatible }
        token = config.session_token
        return config
    }
    public func state() async throws -> BenchState { try await get("/api/state") }
    public func command(_ request: CommandRequest) async throws -> CommandResult {
        let response: CommandResult = try await post("/api/device/commands", body: JSONEncoder().encode(request))
        guard response.ok else { throw ConsoleError.rejected(response.error ?? L("设备未确认操作；请查询状态后再决定是否重试。")) }
        return response
    }
    public func pauseMonitor() async throws { _ = try await send("/api/serial/vad/pause", body: Data("{}".utf8)) }
    public func transcribe(assetID: String) async throws {
        try validateID(assetID)
        // Only called by the explicit transcribe action, never during startup or selection.
        _ = try await send("/api/asr/probe", body: Data("{}".utf8))
        _ = try await send("/api/asr", body: JSONEncoder().encode(["asset_id": assetID]))
    }
    public func media(assetID: String) async throws -> Data {
        try validateID(assetID)
        let (data, response) = try await session.data(for: URLRequest(url: base.appendingPathComponent("api/assets/\(assetID)/content")))
        try check(response, data: data)
        guard data.count <= 20 * 1024 * 1024,
              (response as? HTTPURLResponse)?.mimeType == "audio/wav" else { throw ConsoleError.invalidMedia }
        return data
    }
    public func importAudio(data: Data, filename: String) async throws -> AudioAsset {
        guard !data.isEmpty, data.count <= 20 * 1024 * 1024, filename.lowercased().hasSuffix(".wav") else {
            throw ConsoleError.rejected(L("请选择不超过 20 MiB 的 WAV 录音文件"))
        }
        if token == nil { _ = try await configure() }
        var parts = URLComponents(url: base.appendingPathComponent("api/assets/import"), resolvingAgainstBaseURL: false)!
        parts.queryItems = [URLQueryItem(name: "filename", value: filename)]
        var request = URLRequest(url: parts.url!)
        request.httpMethod = "POST"; request.httpBody = data
        request.setValue("application/octet-stream", forHTTPHeaderField: "Content-Type")
        request.setValue(token, forHTTPHeaderField: "X-Debug-Session")
        let (responseData, response) = try await session.data(for: request)
        if (response as? HTTPURLResponse)?.statusCode == 401 { token = nil }
        try check(response, data: responseData)
        return try JSONDecoder().decode(AudioAsset.self, from: responseData)
    }
    private func validateID(_ id: String) throws {
        guard id.count == 64, id.allSatisfy({ "0123456789abcdef".contains($0) }) else { throw ConsoleError.invalidMedia }
    }
    private func get<T: Decodable>(_ path: String) async throws -> T {
        try JSONDecoder().decode(T.self, from: await send(path))
    }
    private func post<T: Decodable>(_ path: String, body: Data) async throws -> T {
        try JSONDecoder().decode(T.self, from: await send(path, body: body))
    }
    private func send(_ path: String, body: Data? = nil) async throws -> Data {
        if body != nil && token == nil { _ = try await configure() }
        var request = URLRequest(url: base.appendingPathComponent(String(path.dropFirst())))
        if let body {
            request.httpMethod = "POST"; request.httpBody = body
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.setValue(token, forHTTPHeaderField: "X-Debug-Session")
        }
        let (data, response) = try await session.data(for: request)
        if (response as? HTTPURLResponse)?.statusCode == 401 { token = nil }
        // Never replay a write automatically: the operation may already have reached the device.
        try check(response, data: data)
        return data
    }
    private func check(_ response: URLResponse, data: Data) throws {
        guard let response = response as? HTTPURLResponse else { throw ConsoleError.http(0, L("服务没有返回 HTTP 响应")) }
        guard (200..<300).contains(response.statusCode) else {
            struct Detail: Decodable { let detail: String }
            let detail = (try? JSONDecoder().decode(Detail.self, from: data).detail) ?? L("本机服务请求失败")
            throw ConsoleError.http(response.statusCode, String(detail.prefix(400)))
        }
    }
    public func messages() async -> AsyncThrowingStream<LiveMessage, Error> {
        var parts = URLComponents(url: base, resolvingAgainstBaseURL: false)!
        parts.scheme = "ws"; parts.path = "/api/live"
        let socket = session.webSocketTask(with: parts.url!)
        socket.maximumMessageSize = 16 * 1024 * 1024
        socket.resume()
        let heartbeat = SocketHeartbeat()
        return AsyncThrowingStream { continuation in
            let reader = Task {
                do {
                    while !Task.isCancelled {
                        let message = try await socket.receive()
                        await heartbeat.touch()
                        let data: Data
                        switch message {
                        case let .data(value): data = value
                        case let .string(value): data = Data(value.utf8)
                        @unknown default: continue
                        }
                        continuation.yield(try JSONDecoder().decode(LiveMessage.self, from: data))
                    }
                    continuation.finish()
                } catch { continuation.finish(throwing: error) }
            }
            let watchdog = Task {
                while !Task.isCancelled {
                    do { try await Task.sleep(for: .seconds(2)) } catch { return }
                    if await heartbeat.expired() {
                        socket.cancel(with: .goingAway, reason: nil)
                        continuation.finish(throwing: URLError(.timedOut))
                        return
                    }
                }
            }
            continuation.onTermination = { @Sendable _ in
                reader.cancel(); watchdog.cancel(); socket.cancel(with: .goingAway, reason: nil)
            }
        }
    }
}

private final class NoRedirects: NSObject, URLSessionTaskDelegate, Sendable {
    func urlSession(_ session: URLSession, task: URLSessionTask,
                    willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest) async -> URLRequest? { nil }
}

private actor SocketHeartbeat {
    private var latest = ContinuousClock.now
    func touch() { latest = .now }
    func expired() -> Bool { latest.duration(to: .now) > .seconds(12) }
}
