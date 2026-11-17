import Foundation
@testable import AIProviders

/// Routes requests by host so parallel tests never share a handler. No real network is touched.
final class StubProtocol: URLProtocol, @unchecked Sendable {
    struct Reply { var status = 200; var body: Data; var delay: TimeInterval = 0; var error: URLError? }
    nonisolated(unsafe) static var handlers: [String: @Sendable (URLRequest, Data) -> Reply] = [:]
    nonisolated(unsafe) static var seen: [String: [(URLRequest, Data)]] = [:]
    static let lock = NSLock()

    static func session(host: String, handler: @escaping @Sendable (URLRequest, Data) -> Reply) -> URLSession {
        lock.withLock { handlers[host] = handler; seen[host] = [] }
        let c = URLSessionConfiguration.ephemeral
        c.protocolClasses = [StubProtocol.self]
        return URLSession(configuration: c)
    }
    static func requests(_ host: String) -> [(URLRequest, Data)] { lock.withLock { seen[host] ?? [] } }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func stopLoading() {}
    override func startLoading() {
        var body = request.httpBody ?? Data()
        if let stream = request.httpBodyStream {
            stream.open(); defer { stream.close() }
            var buf = [UInt8](repeating: 0, count: 4096)
            while stream.hasBytesAvailable { let n = stream.read(&buf, maxLength: buf.count); if n <= 0 { break }; body.append(buf, count: n) }
        }
        let host = request.url?.host ?? ""
        let h = Self.lock.withLock { () -> (@Sendable (URLRequest, Data) -> Reply)? in
            Self.seen[host, default: []].append((request, body)); return Self.handlers[host]
        }
        guard let h else { client?.urlProtocol(self, didFailWithError: URLError(.cannotConnectToHost)); return }
        let reply = h(request, body)
        if let e = reply.error { client?.urlProtocol(self, didFailWithError: e); return }
        let response = HTTPURLResponse(url: request.url!, statusCode: reply.status, httpVersion: "HTTP/1.1", headerFields: ["Content-Type": "application/json"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: reply.body)
        client?.urlProtocolDidFinishLoading(self)
    }
}

func json(_ text: String) -> Data { Data(text.utf8) }
func bodyJSON(_ data: Data) -> [String: Any] { (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:] }

/// Test rig: one provider/model on a unique stub host.
struct Rig {
    let host: String
    let provider: AIProvider
    let model: AIModel
    let secrets: InMemorySecretStore
    let sink = InMemoryUsageSink()
    let store: InMemoryProviderConfigStore
    let client: AIClient

    init(api: ProviderAPI, destination: Destination = .cloud, key: String? = "sk-test-SECRET-123", cost: ModelCost? = nil,
         handler: @escaping @Sendable (URLRequest, Data) -> StubProtocol.Reply) {
        host = "h\(UUID().uuidString.prefix(8).lowercased()).stub.test"
        let ref = key == nil ? nil : "provider.test"
        provider = AIProvider(providerID: "p1", name: "Test", api: api, baseURL: "https://\(host)" + (api == .anthropicMessages ? "" : "/v1"),
                              secretRef: ref, destination: destination)
        model = AIModel(providerID: "p1", modelID: "m1", cost: cost)
        secrets = InMemorySecretStore(key.map { ["provider.test": $0] } ?? [:])
        store = InMemoryProviderConfigStore(ProviderConfig(providers: [provider], models: [model]))
        client = AIClient(configStore: store, secrets: secrets, usage: sink, usageSource: sink,
                          session: StubProtocol.session(host: host, handler: handler))
    }
}
