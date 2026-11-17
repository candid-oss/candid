import Darwin
import CandidLocalization
import Foundation

/// HTTP/1.1 over the core's Unix domain socket (ADR-024 §3). The App is the core's internal client: no token, and
/// the administration, change-feed, health and ingest paths exist only here. Each request is one short connection on a
/// background queue; the change feed keeps its own connection on a dedicated thread.
public struct LocalChannel: Sendable {
    public let socketPath: String

    public init(socketPath: String) { self.socketPath = socketPath }

    // MARK: Typed calls

    public func health(timeout: TimeInterval = 2) async throws -> CoreHealth { try await get("/health", timeout: timeout) }
    public func status() async throws -> CoreStatus { try await get("/local/status") }

    public func clients() async throws -> [ClientInfo] {
        struct List: Decodable { var clients: [ClientInfo] }
        let list: List = try await get("/local/clients")
        return list.clients
    }

    public func registerClient(name: String) async throws -> ClientSetup {
        try await post("/local/clients", body: ["name": name])
    }

    public func revokeClient(_ id: String) async throws {
        struct Done: Decodable { var revoked: String }
        let _: Done = try await post("/local/clients/revoke", body: ["client_id": id])
    }

    /// Asks the core to run a mirroring pass now; the result arrives as a `status` event.
    public func syncNow() async throws {
        _ = try await send("POST", "/local/sync", body: Data("{}".utf8)).checked()
    }

    /// Hashing every screenshot can take a while on a large library.
    public func verify(hashBlobs: Bool) async throws -> CoreVerification {
        try await post("/local/verify", body: ["hash_blobs": hashBlobs], timeout: 600)
    }

    /// A query tool as the App's own client (scoped and audited like any other).
    public func tool(_ name: String, arguments: Data = Data("{}".utf8)) async throws -> Data {
        try await send("POST", "/v1/" + name, body: arguments, timeout: 60).checked()
    }

    public func get<T: Decodable>(_ path: String, timeout: TimeInterval = 30) async throws -> T {
        try JSONDecoder().decode(T.self, from: try await send("GET", path, timeout: timeout).checked())
    }

    public func post<T: Decodable, B: Encodable>(_ path: String, body: B, timeout: TimeInterval = 30) async throws -> T {
        let data = try JSONEncoder().encode(body)
        return try JSONDecoder().decode(T.self, from: try await send("POST", path, body: data, timeout: timeout).checked())
    }

    // MARK: Transport

    public struct Response: Sendable {
        public var status: Int
        public var body: Data

        /// The body of a 2xx answer; otherwise the core's error.
        public func checked() throws -> Data {
            guard (200..<300).contains(status) else {
                let error = (try? JSONSerialization.jsonObject(with: body) as? [String: Any])?["error"] as? [String: Any]
                throw CoreError.service(status: status, code: error?["code"] as? String ?? "http_\(status)",
                                        message: error?["message"] as? String ?? L("时间线核心返回 %@", status))
            }
            return body
        }
    }

    public func send(_ method: String, _ path: String, body: Data? = nil, timeout: TimeInterval = 30) async throws -> Response {
        let socketPath = socketPath
        return try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .utility).async {
                continuation.resume(with: Result { try Self.exchange(socketPath, method, path, body ?? Data(), timeout: timeout) })
            }
        }
    }

    static func connect(_ path: String, timeout: TimeInterval) throws -> Int32 {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw CoreError.unreachable(String(cString: strerror(errno))) }
        var on: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
        var tv = timeval(tv_sec: Int(timeout), tv_usec: Int32((timeout - Double(Int(timeout))) * 1_000_000))
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
        setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        let capacity = MemoryLayout.size(ofValue: address.sun_path)
        guard bytes.count < capacity else { close(fd); throw CoreError.unreachable(L("套接字路径过长")) }
        withUnsafeMutableBytes(of: &address.sun_path) { raw in
            raw.copyBytes(from: bytes)
            raw[bytes.count] = 0
        }
        address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        let rc = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        guard rc == 0 else {
            let reason = String(cString: strerror(errno))
            close(fd)
            throw CoreError.unreachable(reason)
        }
        return fd
    }

    static func writeAll(_ fd: Int32, _ data: Data) throws {
        try data.withUnsafeBytes { raw in
            var offset = 0
            while offset < raw.count {
                let n = Darwin.write(fd, raw.baseAddress! + offset, raw.count - offset)
                if n <= 0 { throw CoreError.unreachable(String(cString: strerror(errno))) }
                offset += n
            }
        }
    }

    static func request(_ method: String, _ path: String, _ body: Data) -> Data {
        var head = "\(method) \(path) HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n"
        head += "Content-Length: \(body.count)\r\nConnection: close\r\n\r\n"
        return Data(head.utf8) + body
    }

    static func exchange(_ socketPath: String, _ method: String, _ path: String, _ body: Data, timeout: TimeInterval) throws -> Response {
        let fd = try connect(socketPath, timeout: timeout)
        defer { close(fd) }
        try writeAll(fd, request(method, path, body))
        var raw = Data()
        var buffer = [UInt8](repeating: 0, count: 64 * 1024)
        while true {
            let n = read(fd, &buffer, buffer.count)
            if n == 0 { break }
            if n < 0 { throw CoreError.unreachable(String(cString: strerror(errno))) }
            raw.append(buffer, count: n)
        }
        return try parse(raw)
    }

    static func parse(_ raw: Data) throws -> Response {
        guard let split = raw.range(of: Data("\r\n\r\n".utf8)) else { throw CoreError.badResponse }
        let head = String(decoding: raw[raw.startIndex..<split.lowerBound], as: UTF8.self)
        let parts = head.split(separator: " ", maxSplits: 2)
        guard parts.count >= 2, let status = Int(parts[1]) else { throw CoreError.badResponse }
        var body = raw[split.upperBound...]
        for line in head.split(separator: "\r\n").dropFirst() {
            let pair = line.split(separator: ":", maxSplits: 1)
            if pair.count == 2, pair[0].lowercased() == "content-length", let length = Int(pair[1].trimmingCharacters(in: .whitespaces)) {
                body = body.prefix(length)
            }
        }
        return Response(status: status, body: Data(body))
    }

    // MARK: Change feed

    /// Events until the core closes the feed or the stream is cancelled. A closed stream means the core went away or
    /// is restarting; subscribe again once it answers.
    public func events() -> AsyncStream<CoreEvent> {
        let (stream, continuation) = AsyncStream<CoreEvent>.makeStream()
        let reader = FeedReader(socketPath: socketPath, continuation: continuation)
        continuation.onTermination = { _ in reader.cancel() }
        reader.start()
        return stream
    }
}

/// Reads the newline-delimited feed on its own thread; `cancel()` shuts the socket down to unblock the read.
final class FeedReader: @unchecked Sendable {
    let socketPath: String
    let continuation: AsyncStream<CoreEvent>.Continuation
    private let lock = NSLock()
    private var fd: Int32 = -1
    private var cancelled = false

    init(socketPath: String, continuation: AsyncStream<CoreEvent>.Continuation) {
        self.socketPath = socketPath; self.continuation = continuation
    }

    func start() {
        let thread = Thread { [self] in run(); continuation.finish() }
        thread.name = "candid-feed"
        thread.start()
    }

    func cancel() {
        lock.lock(); defer { lock.unlock() }
        cancelled = true
        if fd >= 0 { shutdown(fd, SHUT_RDWR) }
    }

    private func run() {
        guard let fd = try? LocalChannel.connect(socketPath, timeout: 5) else { return }
        lock.lock()
        if cancelled { lock.unlock(); close(fd); return }
        self.fd = fd
        lock.unlock()
        defer { lock.lock(); self.fd = -1; lock.unlock(); close(fd) }
        guard (try? LocalChannel.writeAll(fd, LocalChannel.request("GET", "/local/events", Data()))) != nil else { return }
        // Events are rare; no read timeout once subscribed.
        var tv = timeval(tv_sec: 0, tv_usec: 0)
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
        var pending = Data()
        var headerDone = false
        var buffer = [UInt8](repeating: 0, count: 4096)
        while true {
            let n = read(fd, &buffer, buffer.count)
            if n <= 0 { return }
            pending.append(buffer, count: n)
            if !headerDone {
                guard let end = pending.range(of: Data("\r\n\r\n".utf8)) else { continue }
                let status = String(decoding: pending[pending.startIndex..<end.lowerBound], as: UTF8.self)
                guard status.hasPrefix("HTTP/1.1 200") else { return }
                pending = Data(pending[end.upperBound...])
                headerDone = true
            }
            while let newline = pending.firstIndex(of: UInt8(ascii: "\n")) {
                let line = pending[pending.startIndex..<newline]
                pending = Data(pending[pending.index(after: newline)...])
                if let event = try? JSONDecoder().decode(CoreEvent.self, from: Data(line)) { continuation.yield(event) }
            }
        }
    }
}
