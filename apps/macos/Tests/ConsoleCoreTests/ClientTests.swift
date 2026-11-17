import Foundation
import Testing
@testable import ConsoleCore

private final class StubState: @unchecked Sendable {
    let lock = NSLock()
    var handler: (@Sendable (URLRequest) throws -> (Int, Data, String))?
    func set(_ block: @escaping @Sendable (URLRequest) throws -> (Int, Data, String)) { lock.withLock { handler = block } }
    func handle(_ request: URLRequest) throws -> (Int, Data, String) { try lock.withLock { try handler!(request) } }
}
private final class RequestStub: URLProtocol, @unchecked Sendable {
    static let state = StubState()
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        do {
            let (code, data, mime) = try Self.state.handle(request)
            client?.urlProtocol(self, didReceive: HTTPURLResponse(url: request.url!, statusCode: code, httpVersion: nil, headerFields: ["Content-Type": mime])!, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: data); client?.urlProtocolDidFinishLoading(self)
        } catch { client?.urlProtocol(self, didFailWithError: error) }
    }
    override func stopLoading() { }
}
private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var value = 0
    func next() -> Int { lock.withLock { value += 1; return value } }
    var count: Int { lock.withLock { value } }
}
private func json(_ value: String) -> Data { Data(value.utf8) }
private let config = #"{"session_token":"memory-only-test","protocol_version":1,"schema_version":1,"max_record_seconds":15}"#
private func client() throws -> DebugClient {
    let configuration = URLSessionConfiguration.ephemeral
    configuration.protocolClasses = [RequestStub.self]
    return try DebugClient(session: URLSession(configuration: configuration))
}

@Suite(.serialized)
struct ClientTests {
    @Test func rejectsExternalOrCredentialURLs() throws {
        for endpoint in ["https://127.0.0.1:8766", "http://192.168.1.2", "http://127.0.0.1.evil.test", "http://user:secret@localhost", "http://localhost/api", "http://localhost?token=x", "file:///tmp/x"] {
            #expect(throws: ConsoleError.self) { try DebugClient(endpoint: endpoint) }
        }
        _ = try DebugClient(endpoint: "http://127.0.0.1:18776")
        _ = try DebugClient(endpoint: "http://[::1]:18776")
    }
    @Test func authenticatesWritesAndSendsActualCommandShape() async throws {
        let writes = Counter()
        RequestStub.state.set { request in
            if request.url?.path == "/api/config" { return (200, json(config), "application/json") }
            #expect(request.url?.path == "/api/device/commands")
            #expect(request.httpMethod == "POST")
            #expect(request.value(forHTTPHeaderField: "X-Debug-Session") == "memory-only-test")
            let body = try #require(request.httpBody ?? request.httpBodyStream.flatMap { stream in
                stream.open(); defer { stream.close() }; var bytes = [UInt8](repeating: 0, count: 4096)
                let count = stream.read(&bytes, maxLength: bytes.count); return count > 0 ? Data(bytes.prefix(count)) : nil
            })
            let payload = try #require(JSONSerialization.jsonObject(with: body) as? [String: Any])
            #expect(payload["transport"] as? String == "serial")
            #expect(payload["mode"] as? String == "monitor")
            #expect(payload["max_seconds"] as? Int == 15)
            #expect(payload["codec"] as? String == "opus")
            #expect(payload["bitrate"] as? Int == 24000)
            #expect(UUID(uuidString: payload["request_id"] as? String ?? "") != nil)
            _ = writes.next()
            return (200, json(#"{"ok":true,"state":"recording"}"#), "application/json")
        }
        let result = try await client().command(CommandRequest(transport: .serial, command: "record.start", codec: "opus", bitrate: 24000, seconds: 15))
        #expect(result.ok); #expect(writes.count == 1)
    }
    @Test func expiredSessionDoesNotReplayWriteAndManualRetryRefreshesToken() async throws {
        let writes = Counter(), configs = Counter()
        RequestStub.state.set { request in
            if request.url?.path == "/api/config" { _ = configs.next(); return (200, json(config), "application/json") }
            let attempt = writes.next()
            return attempt == 1 ? (401, json(#"{"detail":"会话已失效"}"#), "application/json") : (200, json(#"{"ok":true,"state":"recording"}"#), "application/json")
        }
        let api = try client(), request = CommandRequest(transport: .serial, command: "record.start", codec: "wav", bitrate: 24000, seconds: 15)
        await #expect(throws: ConsoleError.self) { try await api.command(request) }
        #expect(writes.count == 1); #expect(configs.count == 1)
        _ = try await api.command(request)
        #expect(writes.count == 2); #expect(configs.count == 2)
    }
    @Test func logicalFailureAndHTTPRejectionSurface() async throws {
        RequestStub.state.set { request in
            request.url?.path == "/api/config" ? (200, json(config), "application/json") : (200, json(#"{"ok":false,"state":"failed","error":"设备 RAM 中仍有未保存媒体"}"#), "application/json")
        }
        await #expect(throws: ConsoleError.self) { try await client().command(CommandRequest(transport: .serial, command: "record.start", codec: "wav", bitrate: 24000, seconds: 15)) }
        RequestStub.state.set { _ in (403, json(#"{"detail":"请求来源不受信任"}"#), "application/json") }
        await #expect(throws: ConsoleError.self) { try await client().state() }
    }
    @Test func versionMismatchAndMalformedJSONFailClosed() async throws {
        RequestStub.state.set { _ in (200, json(config.replacingOccurrences(of: "schema_version\":1", with: "schema_version\":2")), "application/json") }
        await #expect(throws: ConsoleError.self) { try await client().configure() }
        RequestStub.state.set { _ in (200, json("not json"), "application/json") }
        await #expect(throws: DecodingError.self) { try await client().state() }
    }
    @Test func playbackRejectsUnsafeIDAndNonWAV() async throws {
        let calls = Counter()
        RequestStub.state.set { _ in _ = calls.next(); return (200, Data([1, 2]), "text/html") }
        let api = try client()
        await #expect(throws: ConsoleError.self) { try await api.media(assetID: "../../config") }
        #expect(calls.count == 0)
        await #expect(throws: ConsoleError.self) { try await api.media(assetID: String(repeating: "a", count: 64)) }
        #expect(calls.count == 1)
    }
}
