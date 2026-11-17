import Darwin
import Foundation
import Testing
@testable import CandidClient

/// The Rust core built by script/test.sh (`cargo build -p candid-cli`), or `CANDID_CORE_BIN`.
private let coreBinary: URL = {
    if let custom = ProcessInfo.processInfo.environment["CANDID_CORE_BIN"], !custom.isEmpty { return URL(fileURLWithPath: custom) }
    return URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("../../../../core/target/debug/candid").standardizedFileURL
}()

/// A throwaway data root and run directory with a short path (Unix socket paths are limited to 104 bytes).
private struct Sandbox {
    let dir: URL
    init(_ label: String) throws {
        dir = URL(fileURLWithPath: "/tmp/pvc-\(label)-\(UUID().uuidString.prefix(8))", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }
    var root: URL { dir.appendingPathComponent("timeline") }
    var run: URL { dir.appendingPathComponent("run") }
    func launch(ledger: URL? = nil) -> CoreLaunch { CoreLaunch(executable: coreBinary, root: root, runDirectory: run, ledger: ledger) }
    func remove() { try? FileManager.default.removeItem(at: dir) }
}

@MainActor private func waitFor(_ what: String, seconds: Double = 15, _ condition: () async -> Bool) async throws {
    let deadline = Date().addingTimeInterval(seconds)
    while !(await condition()) {
        guard Date() < deadline else { Issue.record("timed out: \(what)"); throw CancellationError() }
        try await Task.sleep(for: .milliseconds(50))
    }
}

@MainActor private func ownedPID(_ s: CoreSupervisor) -> Int32? {
    if case .running(let pid, true) = s.state { return pid }
    return nil
}

@Suite struct RunDirectoryTests {
    @Test func socketStaysInTheRunDirectoryUnlessThePathIsTooLong() {
        let short = URL(fileURLWithPath: "/tmp/run", isDirectory: true)
        #expect(RunDirectory.socket(in: short).path == "/tmp/run/core.sock")
        let long = URL(fileURLWithPath: "/var/folders/xx/" + String(repeating: "d", count: 90) + "/run", isDirectory: true)
        let temp = URL(fileURLWithPath: "/private/var/folders/ab/T", isDirectory: true)
        let socket = RunDirectory.socket(in: long, temporary: temp)
        #expect(socket.deletingLastPathComponent().path == temp.path)
        #expect(socket.lastPathComponent.hasPrefix("candid-") && socket.path.utf8.count < 104)
        #expect(RunDirectory.socket(in: long, temporary: temp) == socket, "stable across launches")
        #expect(RunDirectory.socket(in: long.appendingPathComponent("x"), temporary: temp) != socket)
        #expect(RunDirectory.url(environment: ["CANDID_RUN_DIR": "/tmp/qa-run"]).path == "/tmp/qa-run")
    }

    @Test func launchArgumentsNameTheSocketAndThisProcessAsParent() {
        let l = CoreLaunch(executable: coreBinary, root: URL(fileURLWithPath: "/tmp/r"), runDirectory: URL(fileURLWithPath: "/tmp/run"),
                           ledger: URL(fileURLWithPath: "/tmp/l"), hub: URL(fileURLWithPath: "/tmp/h"), syncInterval: 5)
        #expect(l.arguments == ["serve", "--root", "/tmp/r", "--run-dir", "/tmp/run", "--socket", "/tmp/run/core.sock",
                                "--parent-pid", String(getpid()), "--ledger", "/tmp/l", "--hub", "/tmp/h", "--sync-interval", "5"])
    }

    @Test func responsesParseAndErrorsCarryTheCoresMessage() throws {
        let ok = try LocalChannel.parse(Data("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}trailing".utf8))
        #expect(ok.status == 200 && ok.body == Data("{}".utf8))
        let denied = try LocalChannel.parse(Data("HTTP/1.1 403 Forbidden\r\n\r\n{\"error\":{\"code\":\"local_only\",\"message\":\"只在本机\"}}".utf8))
        #expect(throws: CoreError.service(status: 403, code: "local_only", message: "只在本机")) { try denied.checked() }
        #expect(throws: CoreError.badResponse) { try LocalChannel.parse(Data("garbage".utf8)) }
    }

    @Test func nobodyOnTheSocketIsUnreachable() async throws {
        let channel = LocalChannel(socketPath: "/tmp/pvc-none-\(UUID().uuidString.prefix(8)).sock")
        await #expect(throws: CoreError.self) { _ = try await channel.health() }
    }
}

@Suite(.serialized) @MainActor struct CoreSupervisorTests {
    init() {
        precondition(FileManager.default.isExecutableFile(atPath: coreBinary.path),
                     "Rust core missing at \(coreBinary.path); run script/test.sh (it builds core first)")
    }

    @Test func startsTheCoreAndAdministersClientsOverTheLocalChannel() async throws {
        let box = try Sandbox("admin"); defer { box.remove() }
        let supervisor = CoreSupervisor(launch: box.launch())
        supervisor.start()
        try await waitFor("core running") { ownedPID(supervisor) != nil }
        let channel = supervisor.channel
        let health = try await channel.health()
        #expect(health.pid == ownedPID(supervisor))
        #expect(health.parent_pid == getpid())
        let mode = { (url: URL) in ((try? FileManager.default.attributesOfItem(atPath: url.path)[.posixPermissions]) as? Int) ?? 0 }
        #expect(mode(supervisor.launch.socket) == 0o600)
        #expect(mode(box.run) == 0o700)
        #expect(FileManager.default.fileExists(atPath: RunDirectory.cliTokenFile(in: box.run).path))
        let status = try await channel.status()
        #expect(status.phase == "ready" && status.events == 0)
        #expect(status.endpoint?.hasPrefix("http://127.0.0.1:") == true)

        let setup = try await channel.registerClient(name: "Synthetic AI")
        #expect(setup.token.count == 64)
        #expect(setup.claude_code.contains("CANDID_TOKEN=\(setup.token)") && setup.claude_code.hasSuffix("\" mcp"))
        #expect(setup.codex.hasPrefix("[mcp_servers.candid]"))
        #expect(setup.json.contains("\"mcpServers\""))
        let clients = try await channel.clients()
        #expect(clients.contains { $0.name == "Synthetic AI" && $0.transport == "mcp" && $0.revoked_ms == nil })
        #expect(clients.contains { $0.transport == "cli" })
        try await channel.revokeClient(setup.client_id)
        #expect(try await channel.clients().first { $0.client_id == setup.client_id }?.revoked_ms != nil)
        await #expect(throws: CoreError.self) { try await channel.revokeClient("no-such-client") }
        await #expect(throws: CoreError.self) { _ = try await channel.registerClient(name: "  ") }
        // The App's own queries go through the same tools.
        let answer = try JSONSerialization.jsonObject(with: try await channel.tool("status")) as? [String: Any]
        #expect((answer?["store"] as? [String: Any])?["store_id"] as? String == health.store_id)

        supervisor.stop()
        #expect(supervisor.state == .stopped)
        #expect(!FileManager.default.fileExists(atPath: supervisor.launch.socket.path), "the core removes its socket")
        #expect(!FileManager.default.fileExists(atPath: RunDirectory.endpointFile(in: box.run).path))
    }

    @Test func restartsACrashedCoreWithBackoff() async throws {
        let box = try Sandbox("crash"); defer { box.remove() }
        let supervisor = CoreSupervisor(launch: box.launch())
        supervisor.backoff = { 0.2 * pow(2, Double($0)) }
        var states: [CoreSupervisor.State] = []
        supervisor.onChange = { states.append($0) }
        supervisor.start()
        try await waitFor("first core") { ownedPID(supervisor) != nil }
        let first = try #require(ownedPID(supervisor))
        kill(first, SIGKILL)
        try await waitFor("restarted core") { ownedPID(supervisor).map { $0 != first } ?? false }
        #expect(supervisor.restarts == 1)
        #expect(states.contains { if case .restarting(let after, _) = $0 { return after == 0.2 }; return false })
        // The data root survives a hard kill: the new core reopens it and answers.
        #expect(try await supervisor.channel.status().phase == "ready")
        let second = try #require(ownedPID(supervisor))
        kill(second, SIGKILL)
        try await waitFor("second restart") { ownedPID(supervisor).map { $0 != second } ?? false }
        #expect(states.contains { if case .restarting(let after, _) = $0 { return after == 0.4 }; return false }, "backoff doubles")
        supervisor.stop()
    }

    @Test func aSecondAppUsesTheRunningCoreInsteadOfStartingAnother() async throws {
        let box = try Sandbox("single"); defer { box.remove() }
        let first = CoreSupervisor(launch: box.launch())
        first.start()
        try await waitFor("first core") { ownedPID(first) != nil }
        let pid = try #require(ownedPID(first))
        let second = CoreSupervisor(launch: box.launch())
        second.start()
        try await waitFor("second attaches") { second.state == .running(pid: pid, owned: false) }
        #expect(second.process == nil, "no second core was started")
        second.stop()
        #expect(kill(pid, 0) == 0, "stopping the non-owner leaves the core running")
        first.stop()
    }

    @Test func aCoreHoldingTheDataRootElsewhereMakesTheNewOneRetry() async throws {
        let box = try Sandbox("busy"); defer { box.remove() }
        // Another core on the same data root but a different socket: ours exits 75 and is retried with backoff.
        let other = Process()
        other.executableURL = coreBinary
        other.arguments = ["serve", "--root", box.root.path, "--run-dir", box.dir.appendingPathComponent("other").path]
        other.standardError = FileHandle.nullDevice
        try other.run()
        try await waitFor("other core") { FileManager.default.fileExists(atPath: box.dir.appendingPathComponent("other/endpoint.json").path) }
        let supervisor = CoreSupervisor(launch: box.launch())
        supervisor.backoff = { _ in 0.2 }
        var reasons: [String] = []
        supervisor.onChange = { if case .restarting(_, let reason) = $0 { reasons.append(reason) } }
        supervisor.start()
        try await waitFor("busy retry") { !reasons.isEmpty }
        #expect(reasons[0].hasPrefix("另一个核心正在运行"))
        other.terminate(); other.waitUntilExit()
        try await waitFor("core after the other one stopped") { ownedPID(supervisor) != nil }
        supervisor.stop()
    }

    @Test func aMissingExecutableFailsWithoutRetrying() async throws {
        let box = try Sandbox("missing"); defer { box.remove() }
        var launch = box.launch()
        launch.executable = box.dir.appendingPathComponent("no-such-candid")
        let supervisor = CoreSupervisor(launch: launch)
        supervisor.start()
        try await waitFor("failure") { if case .failed = supervisor.state { return true }; return false }
        #expect(supervisor.restarts == 0)
    }

    @Test func theChangeFeedReportsIngestAndClientsAndEndsWithTheCore() async throws {
        let box = try Sandbox("feed"); defer { box.remove() }
        let supervisor = CoreSupervisor(launch: box.launch())
        supervisor.start()
        try await waitFor("core running") { ownedPID(supervisor) != nil }
        let channel = supervisor.channel
        let collected = EventLog()
        let reader = Task { for await event in channel.events() { await collected.add(event.type) }; await collected.add("closed") }
        try await waitFor("hello") { await collected.contains("hello") }
        // Without an old ledger the switch to direct capture happens at once; before it desktop batches wait (409).
        let cutover = try await channel.cutover()
        #expect(cutover.ledger == nil && cutover.records == 0)
        struct Source: Decodable { var source_id: String }
        let source: Source = try await channel.post("/local/sources", body: ["kind": "mac.desktop", "display_name": "合成桌面"])
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        let batch = """
        {"source_id":"\(source.source_id)","events":[{"source_key":"k1","kind":"screen.text","start_ms":\(now),"title":"合成","text":[{"field":"body","content":"合成段落"}]}]}
        """
        struct Ingested: Decodable { var inserted: Int; var duplicates: Int }
        let first = try JSONDecoder().decode(Ingested.self, from: try await channel.send("POST", "/local/ingest", body: Data(batch.utf8)).checked())
        #expect(first.inserted == 1)
        try await waitFor("data event") { await collected.contains("data") }
        let retry = try JSONDecoder().decode(Ingested.self, from: try await channel.send("POST", "/local/ingest", body: Data(batch.utf8)).checked())
        #expect(retry.inserted == 0 && retry.duplicates == 1, "retrying a batch is harmless")
        _ = try await channel.registerClient(name: "Feed")
        try await waitFor("clients event") { await collected.contains("clients") }
        #expect(try await channel.status().events == 1)
        supervisor.stop()
        try await waitFor("feed closed") { await collected.contains("closed") }
        reader.cancel()
    }

    @Test func mirrorsALedgerAndVerifiesOnTheFirstPass() async throws {
        let box = try Sandbox("ledger"); defer { box.remove() }
        // An empty ledger directory: the first pass finishes, there is nothing to compare, `verify` reports no ledger.
        let ledger = box.dir.appendingPathComponent("desktop")
        try FileManager.default.createDirectory(at: ledger, withIntermediateDirectories: true)
        let supervisor = CoreSupervisor(launch: box.launch(ledger: ledger))
        supervisor.start()
        try await waitFor("core running") { ownedPID(supervisor) != nil }
        try await waitFor("first pass") { (try? await supervisor.channel.status())?.last_sync_ms != nil }
        try await supervisor.channel.syncNow()
        await #expect(throws: CoreError.service(status: 409, code: "no_ledger", message: "没有可核对的桌面账本。")) {
            _ = try await supervisor.channel.verify(hashBlobs: false)
        }
        supervisor.stop()
    }
}

private actor EventLog {
    var types: [String] = []
    func add(_ type: String) { types.append(type) }
    func contains(_ type: String) -> Bool { types.contains(type) }
}
