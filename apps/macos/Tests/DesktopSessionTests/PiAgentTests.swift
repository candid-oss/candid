import Foundation
import CandidClient
import Testing
@testable import Candid

/// The attached Pi agent against a real core, the pinned Pi from agents/pi-extension and a scripted local
/// model (test/mock-model.mjs). No real model is called; all data is synthetic. Needs Node (`CANDID_PI_NODE`,
/// or Homebrew's) and `npm ci` in the extension folder; without them the end-to-end test records why and returns.
@MainActor @Suite(.serialized) struct PiAgentTests {
    static let extensionDir = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        .appendingPathComponent("../../../../agents/pi-extension").standardizedFileURL

    @Test func linesSplitOnlyOnLineFeeds() {
        var buffer = PiLineBuffer()
        let first = buffer.append(Data("{\"a\":\"x\u{2028}y\"}\r\n{\"b\":".utf8))
        #expect(first == [Data("{\"a\":\"x\u{2028}y\"}".utf8)])
        #expect(buffer.append(Data("1}\n\n".utf8)) == [Data("{\"b\":1}".utf8)])
    }

    @Test func recordsBecomePageEvents() {
        func events(_ json: String) -> [PiEvent] { PiRecord.events(Data(json.utf8)) }
        #expect(events(#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"你好"}}"#) == [.textDelta("你好")])
        #expect(events(#"{"type":"tool_execution_start","toolCallId":"c1","toolName":"pv_search","args":{"q":"预算"}}"#) == [.toolStarted(id: "c1", name: "pv_search", arguments: #"{"q":"预算"}"#)])
        #expect(events(#"{"type":"tool_execution_end","toolCallId":"c1","toolName":"pv_search","result":{"content":[{"type":"text","text":"x"}]},"isError":true}"#) == [.toolFinished(id: "c1", name: "pv_search", result: "x", isError: true)])
        #expect(events(#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"答"}],"stopReason":"stop"}}"#) == [.assistantMessage("答")])
        #expect(events(#"{"type":"message_end","message":{"role":"user","content":"问"}}"#).isEmpty)
        #expect(events(#"{"type":"agent_settled","aborted":true}"#) == [.settled(aborted: true)])
        #expect(events(#"{"type":"response","command":"prompt","success":false,"error":"nope"}"#) == [.error("nope")])
        #expect(events("not json").isEmpty)
    }

    @Test func commandLineKeepsPiIsolated() {
        let install = PiInstall(node: URL(fileURLWithPath: "/n"), home: URL(fileURLWithPath: "/p"))
        let args = install.arguments(sessions: URL(fileURLWithPath: "/s"))
        for flag in ["--mode", "rpc", "--no-builtin-tools", "--no-extensions", "--no-mcp", "--no-skills", "--no-context-files", "--no-approve", "--offline"] {
            #expect(args.contains(flag), "\(flag)")
        }
        #expect(args[args.firstIndex(of: "-e")! + 1] == "/p/index.ts")
        #expect(args[args.firstIndex(of: "--tools")! + 1] == "pv_*")
    }

    private static var node: URL? {
        let candidates = [ProcessInfo.processInfo.environment["CANDID_PI_NODE"], "/opt/homebrew/bin/node", "/usr/local/bin/node"]
        return candidates.compactMap { $0 }.filter { !$0.isEmpty && FileManager.default.isExecutableFile(atPath: $0) }.map { URL(fileURLWithPath: $0) }.first
    }

    static var nodeForTests: URL? { node }

    private func mockModel(_ node: URL) throws -> (Process, Int) { try Self.startMockModel(node) }

    /// Starts the scripted model and returns its port.
    static func startMockModel(_ node: URL) throws -> (Process, Int) {
        let process = Process()
        process.executableURL = node
        process.arguments = [Self.extensionDir.appendingPathComponent("test/mock-model.mjs").path]
        let out = Pipe()
        process.standardOutput = out
        // Never share the test runner's stderr, and tie the server's life to ours through its stdin (the Process keeps
        // the pipe alive; when this test process exits the pipe closes and the server exits too).
        process.standardError = FileHandle.nullDevice
        process.standardInput = Pipe()
        process.environment = ProcessInfo.processInfo.environment.merging(["MOCK_MODEL_EXIT_ON_EOF": "1"]) { $1 }
        try process.run()
        var line = Data()
        while !line.contains(0x0A) {
            let chunk = out.fileHandleForReading.availableData
            if chunk.isEmpty { break }
            line.append(chunk)
        }
        struct Port: Decodable { var port: Int }
        let port = try JSONDecoder().decode(Port.self, from: line.split(separator: 0x0A).first.map { Data($0) } ?? Data())
        return (process, port.port)
    }

    private func post(_ channel: LocalChannel, _ path: String, _ body: [String: Any]) async throws -> [String: Any] {
        let data = try await channel.send("POST", path, body: JSONSerialization.data(withJSONObject: body)).checked()
        return (try JSONSerialization.jsonObject(with: data) as? [String: Any]) ?? [:]
    }

    @Test func runsReadsRestartsAfterACrashAndCancels() async throws {
        guard let node = Self.node, FileManager.default.fileExists(atPath: Self.extensionDir.appendingPathComponent("node_modules/@earendil-works/pi-coding-agent/package.json").path) else {
            Issue.record("跳过：没有 Node 或未在 agents/pi-extension 运行 npm ci")
            return
        }
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("pi-agent-" + UUID().uuidString, isDirectory: true)
        let channel = try await TestCore.ready(root, cutover: false).channel!
        let (model, port) = try mockModel(node)
        defer { model.terminate(); TestCore.finish(root) }

        let source = try await post(channel, "/local/sources", ["kind": "mac.region", "display_name": "合成笔记"])["source_id"] as! String
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        _ = try await post(channel, "/local/ingest", ["source_id": source, "events": [[
            "source_key": "n1", "kind": "screen.text", "start_ms": now - 60_000, "time_basis": "observed", "actor": "self", "sensitivity": "normal",
            "payload_version": 1, "payload": [:], "blobs": [], "app_bundle_id": "com.example.notes", "app_name": "Notes", "title": "季度预算",
            "text": [["field": "body", "content": "【合成样例】季度预算需要复核", "visible": true, "focus_distance": 0]],
        ]]])
        _ = try await post(channel, "/local/models", ["action": "save", "config": [
            "version": 1,
            "providers": [["provider_id": "mock", "name": "模拟模型", "api": "openai-chat", "base_url": "http://127.0.0.1:\(port)/v1", "destination": "on_device", "enabled": true, "headers_json": [:]]],
            "models": [["provider_id": "mock", "model_id": "mock-agent"]],
            "roles": [["role": "agent_chat", "model": ["provider_id": "mock", "model_id": "mock-agent"]]],
        ]])

        let agent = PiAgent(channel: { channel }, install: PiInstall(node: node, home: Self.extensionDir),
                            directory: root.appendingPathComponent("pi", isDirectory: true), prepare: nil)
        agent.backoff = { _ in 0.2 }
        var events: [PiEvent] = []
        agent.onEvent = { events.append($0) }
        func turn(_ text: String, cancelAfter: Duration? = nil) async throws -> [PiEvent] {
            events.removeAll()
            try agent.prompt(text)
            if let cancelAfter { try await Task.sleep(for: cancelAfter); agent.cancel() }
            let deadline = ContinuousClock.now.advanced(by: .seconds(60))
            while !events.contains(where: { if case .settled = $0 { true } else { false } }) {
                guard ContinuousClock.now < deadline else { Issue.record("没有等到 settled：\(events)"); break }
                try await Task.sleep(for: .milliseconds(20))
            }
            return events
        }

        try await agent.start(PiRunConfig(runID: "swift-run", writes: []))
        #expect(agent.run?.model.model_id == "mock-agent")
        #expect(agent.run?.tools.contains("annotate") == false)
        var got = try await turn("[读取] 今天的预算记录")
        let finished = got.compactMap { e -> (String, String, Bool)? in if case let .toolFinished(_, name, result, isError) = e { (name, result, isError) } else { nil } }
        #expect(finished.count == 1 && finished[0].0 == "pv_search" && !finished[0].2 && finished[0].1.contains("季度预算"))
        #expect(got.contains(.assistantMessage("找到了预算记录。")))
        #expect(got.contains { if case .textDelta = $0 { true } else { false } })

        // Crash: the agent restarts Pi with the same run and keeps working.
        let first = agent.pid
        kill(first!, SIGKILL)
        let deadline = ContinuousClock.now.advanced(by: .seconds(20))
        while agent.pid == nil || agent.pid == first {
            guard ContinuousClock.now < deadline else { Issue.record("Pi 没有重启"); return }
            try await Task.sleep(for: .milliseconds(50))
        }
        #expect(agent.run?.run_id == "swift-run")
        try await Task.sleep(for: .milliseconds(500))
        got = try await turn("[读取] 再查一次")
        #expect(got.contains(.assistantMessage("找到了预算记录。")))

        // Cancel: a slow answer is abandoned.
        let started = ContinuousClock.now
        got = try await turn("[慢] 慢一点", cancelAfter: .milliseconds(400))
        #expect(got.contains(.settled(aborted: true)))
        #expect(ContinuousClock.now - started < .seconds(4))

        let token = agent.run!.token
        await agent.stop()
        #expect(agent.run == nil)
        let after = try await Self.withToken(channel.socketPath, token)
        #expect(after == 401, "the run's token ends with the run")
        let usage = try await post(channel, "/local/models", ["action": "usage"])
        let calls = usage["calls"] as? [[String: Any]] ?? []
        #expect(!calls.isEmpty && calls.allSatisfy { $0["client"] as? String == "pi" })
    }

    /// A tool call with a bearer token over the socket; returns the HTTP status.
    nonisolated static func withToken(_ path: String, _ token: String) async throws -> Int {
        try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global().async {
                let fd = socket(AF_UNIX, SOCK_STREAM, 0)
                var address = sockaddr_un()
                address.sun_family = sa_family_t(AF_UNIX)
                _ = withUnsafeMutableBytes(of: &address.sun_path) { raw in path.utf8CString.withUnsafeBytes { raw.copyMemory(from: $0) } }
                let ok = withUnsafePointer(to: &address) { $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) } }
                guard ok == 0 else { close(fd); continuation.resume(throwing: CoreError.unreachable("socket")); return }
                let request = "POST /v1/status HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer \(token)\r\nContent-Length: 2\r\n\r\n{}"
                _ = request.withCString { write(fd, $0, strlen($0)) }
                var buffer = [UInt8](repeating: 0, count: 64)
                let n = read(fd, &buffer, 64)
                close(fd)
                let head = String(decoding: buffer.prefix(max(0, n)), as: UTF8.self)
                continuation.resume(returning: Int(head.split(separator: " ").dropFirst().first ?? "0") ?? 0)
            }
        }
    }
}
