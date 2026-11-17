import Foundation
import CandidClient

// The attached Pi agent (agent-platform-v1 §4.1): Pi runs in RPC mode as a process the App manages, with only
// Candid's extension. Each run freezes model (role `agent_chat`), tools, scope and budget in the core
// (`POST /local/pi/start`); Pi gets a token for the dedicated client `candid-pi`, and every tool and model call
// goes back through the core, which checks scope and logs `access_log` and `ai_calls` (`client = pi`). The user's own
// `~/.pi` is never read: Pi gets its own HOME, agent and session folders under the App's data directory.

/// What one run may do. Frozen when the run starts; change it by starting a new run.
struct PiRunConfig: Sendable, Equatable {
    var runID = UUID().uuidString
    /// In-store writes: `annotate`, `reference`, `commitment`, `draft`. Empty: read and propose only.
    var writes: [String] = []
    /// Source IDs the run may read; nil means every source the user allows for AI.
    var sources: [String]?
    /// Live-read AI session connectors (`codex`, `claude_code`, `pi`).
    var agents: [String] = []
    /// Whether a cloud model may be used (the strictest of feature, source and role settings).
    var allowRemote = false
    var automationID: String?
    var attempt = 0
    var maxModelCalls = 12
    var maxToolCalls = 40
    var maxOutputTokens = 16_000

    var body: [String: Any] {
        var body: [String: Any] = ["run_id": runID, "writes": writes, "agents": agents, "allow_remote": allowRemote,
                                   "budget": ["max_model_calls": maxModelCalls, "max_tool_calls": maxToolCalls, "max_output_tokens": maxOutputTokens]]
        if let sources { body["sources"] = sources }
        if let automationID { body["automation_id"] = automationID; body["attempt"] = attempt }
        return body
    }
}

/// The core's answer to a run start.
struct PiRun: Decodable, Sendable, Equatable {
    struct Model: Decodable, Sendable, Equatable { var provider_id: String; var model_id: String; var destination: String; var role: String }
    var run_id: String
    var client_id: String
    var token: String
    var model: Model
    var tools: [String]
    var writes: [String]
    var context_note: String? = nil
}

/// What the AI page draws.
enum PiEvent: Sendable, Equatable {
    case ready(model: String)
    case turnStarted
    case textDelta(String)
    /// A finished assistant message (its full text).
    case assistantMessage(String)
    case toolStarted(id: String, name: String, arguments: String)
    case toolFinished(id: String, name: String, result: String, isError: Bool)
    /// Pi will not continue on its own: the turn is over (or was cancelled).
    case settled(aborted: Bool)
    case error(String)
    case restarting(after: Double, reason: String)
    case stopped
}

enum PiAgentError: LocalizedError, Equatable {
    case notInstalled
    case noCore
    case notRunning
    case busy

    var errorDescription: String? {
        switch self {
        case .notInstalled: "AI 助手的运行环境还没有安装：在「设置 → 组件」里安装 Node 和 Pi"
        case .noCore: "时间线库尚未打开"
        case .notRunning: "AI 还没有启动"
        case .busy: "上一轮还没结束"
        }
    }
}

/// Where Node and Pi live: the managed component folder (installed from Settings → 组件) with the Node the components
/// resolve, or (development, tests) `CANDID_PI_NODE` and `CANDID_PI_HOME`.
struct PiInstall: Sendable {
    var node: URL
    /// Folder with the extension (`index.ts`, `system-prompt.md`) and Pi in `node_modules`.
    var home: URL

    var cli: URL { home.appendingPathComponent("node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js") }
    var isInstalled: Bool { FileManager.default.isExecutableFile(atPath: node.path) && FileManager.default.fileExists(atPath: cli.path) }

    static func resolved(environment: [String: String] = ProcessInfo.processInfo.environment) -> PiInstall {
        let home = environment["CANDID_PI_HOME"].flatMap { $0.isEmpty ? nil : URL(fileURLWithPath: $0) } ?? PiComponent.managedHome
        let node = NodeRuntime.resolve(environment: environment)?.url ?? NodeRuntime.managedNode
        return PiInstall(node: node, home: home)
    }

    /// The command line: RPC mode, no built-in tools, no other extensions, skills, templates, themes, context files,
    /// MCP or project trust; only our extension, our system prompt and our provider.
    func arguments(sessions: URL) -> [String] {
        [cli.path, "--mode", "rpc", "--session-dir", sessions.path,
         "--no-builtin-tools", "--no-extensions", "-e", home.appendingPathComponent("index.ts").path, "--tools", "pv_*",
         "--no-mcp", "--no-skills", "--no-prompt-templates", "--no-themes", "--no-context-files", "--no-approve", "--offline",
         "--system-prompt", home.appendingPathComponent("system-prompt.md").path, "--provider", "candid", "--model", "agent_chat"]
    }
}

/// Splits Pi's stdout into records on LF only (JSON strings may hold U+2028/U+2029, which are not record breaks).
struct PiLineBuffer {
    private var pending = Data()

    mutating func append(_ data: Data) -> [Data] {
        pending.append(data)
        var lines: [Data] = []
        while let i = pending.firstIndex(of: 0x0A) {
            var line = pending[pending.startIndex..<i]
            if line.last == 0x0D { line = line.dropLast() }
            if !line.isEmpty { lines.append(Data(line)) }
            pending.removeSubrange(pending.startIndex...i)
        }
        return lines
    }
}

enum PiRecord {
    /// The events one stdout record stands for (most records are bookkeeping the page does not draw).
    static func events(_ line: Data) -> [PiEvent] {
        guard let record = try? JSONSerialization.jsonObject(with: line) as? [String: Any], let type = record["type"] as? String else { return [] }
        switch type {
        case "turn_start": return [.turnStarted]
        case "message_update":
            let update = record["assistantMessageEvent"] as? [String: Any]
            if update?["type"] as? String == "text_delta", let delta = update?["delta"] as? String { return [.textDelta(delta)] }
            return []
        case "message_end":
            guard let message = record["message"] as? [String: Any], message["role"] as? String == "assistant" else { return [] }
            var events: [PiEvent] = []
            let text = (message["content"] as? [[String: Any]] ?? []).filter { $0["type"] as? String == "text" }.compactMap { $0["text"] as? String }.joined()
            if !text.isEmpty { events.append(.assistantMessage(text)) }
            if message["stopReason"] as? String == "error", let error = message["errorMessage"] as? String { events.append(.error(error)) }
            return events
        case "tool_execution_start":
            let args = (try? JSONSerialization.data(withJSONObject: record["args"] ?? [:], options: [.sortedKeys])).flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
            return [.toolStarted(id: record["toolCallId"] as? String ?? "", name: record["toolName"] as? String ?? "", arguments: args)]
        case "tool_execution_end":
            let content = (record["result"] as? [String: Any])?["content"] as? [[String: Any]] ?? []
            let text = content.compactMap { $0["text"] as? String }.joined()
            return [.toolFinished(id: record["toolCallId"] as? String ?? "", name: record["toolName"] as? String ?? "", result: text, isError: record["isError"] as? Bool ?? false)]
        case "agent_settled": return [.settled(aborted: record["aborted"] as? Bool ?? false)]
        case "extension_error": return [.error(record["error"] as? String ?? "插件出错")]
        case "response":
            if record["success"] as? Bool == false { return [.error(record["error"] as? String ?? "Pi 拒绝了命令")] }
            if record["command"] as? String == "prompt", (record["data"] as? [String: Any])?["disposition"] as? String == "handled" { return [.settled(aborted: false)] }
            return []
        default: return []
        }
    }
}

/// Starts, feeds, cancels and restarts the Pi process for one run at a time.
@MainActor final class PiAgent {
    var onEvent: ((PiEvent) -> Void)?
    private(set) var run: PiRun?
    private(set) var config: PiRunConfig?
    /// A turn is running (from a prompt until Pi settles).
    private(set) var busy = false
    private(set) var restarts = 0
    /// Delay before the n-th consecutive restart (n from 0).
    var backoff: (Int) -> Double = { min(60, pow(2, Double($0))) }

    private let channel: () -> LocalChannel?
    private let fixedInstall: PiInstall?
    /// Resolved at each start, so installing or removing components takes effect without relaunching.
    private var install: PiInstall { fixedInstall ?? .resolved() }
    private let directory: URL
    private let prepare: (() async throws -> Void)?
    private var process: Process?
    private var input: FileHandle?
    private var buffer = PiLineBuffer()
    private var stopping = false
    private var launchedAt = Date.distantPast
    private var restartTask: Task<Void, Never>?

    /// `directory` holds Pi's own HOME, agent folder, sessions and working folder (never the user's `~/.pi`).
    /// `prepare` brings the core's model settings and keys up to date before a run (the App's `AIRuntime`).
    init(channel: @escaping () -> LocalChannel? = { TimelineService.shared.channel },
         install: PiInstall? = nil,
         directory: URL = PiAgent.defaultDirectory,
         prepare: (() async throws -> Void)? = { try await AIRuntime.shared.syncCore() }) {
        self.channel = channel
        self.fixedInstall = install
        self.directory = directory
        self.prepare = prepare
    }

    nonisolated static var defaultDirectory: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("Candid/pi", isDirectory: true)
    }

    var pid: Int32? { process?.isRunning == true ? process?.processIdentifier : nil }

    /// Freezes a run in the core and starts Pi for it. A running run is stopped first.
    func start(_ config: PiRunConfig) async throws {
        if run != nil { await stop() }
        guard install.isInstalled else { throw PiAgentError.notInstalled }
        try await prepare?()
        let run = try await freeze(config)
        self.config = config
        self.run = run
        stopping = false
        restarts = 0
        try launch(run)
    }

    /// Sends one user message. A message that starts like a Pi command (`!`, `/`) is sent as plain text.
    func prompt(_ text: String) throws {
        guard run != nil, process?.isRunning == true else { throw PiAgentError.notRunning }
        guard !busy else { throw PiAgentError.busy }
        let message = text.hasPrefix("!") || text.hasPrefix("/") ? " " + text : text
        busy = true
        let scoped = run?.context_note.map { $0 + "\n\n" + message } ?? message
        try send(["type": "prompt", "message": scoped])
    }

    /// Cancels the running turn; Pi answers with `settled(aborted: true)`.
    func cancel() {
        guard busy else { return }
        try? send(["type": "abort"])
    }

    /// Ends the run: Pi shuts down, the core forgets the token.
    func stop() async {
        stopping = true
        restartTask?.cancel(); restartTask = nil
        if let process, process.isRunning {
            try? input?.close()
            let deadline = Date().addingTimeInterval(3)
            while process.isRunning && Date() < deadline { try? await Task.sleep(for: .milliseconds(30)) }
            if process.isRunning { process.terminate() }
        }
        process = nil; input = nil
        if let run, let channel = channel() { _ = try? await channel.send("POST", "/local/pi/stop", body: JSONSerialization.data(withJSONObject: ["run_id": run.run_id])) }
        run = nil
        busy = false
        onEvent?(.stopped)
    }

    // MARK: Process

    private func freeze(_ config: PiRunConfig) async throws -> PiRun {
        guard let channel = channel() else { throw PiAgentError.noCore }
        let body = try JSONSerialization.data(withJSONObject: config.body)
        return try JSONDecoder().decode(PiRun.self, from: try await channel.send("POST", "/local/pi/start", body: body).checked())
    }

    private func launch(_ run: PiRun) throws {
        guard let channel = channel() else { throw PiAgentError.noCore }
        let fm = FileManager.default
        let home = directory.appendingPathComponent("home"), agent = directory.appendingPathComponent("agent")
        let sessions = directory.appendingPathComponent("sessions"), work = directory.appendingPathComponent("work")
        for folder in [home, agent, sessions, work] {
            try fm.createDirectory(at: folder, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        }
        let process = Process()
        process.executableURL = install.node
        process.arguments = install.arguments(sessions: sessions)
        process.currentDirectoryURL = work
        // Nothing of the user's environment (keys, proxies, PATH tools) reaches Pi.
        process.environment = [
            "PATH": "/usr/bin:/bin", "HOME": home.path, "TMPDIR": NSTemporaryDirectory(), "LANG": "en_US.UTF-8",
            "PI_CODING_AGENT_DIR": agent.path, "PI_OFFLINE": "1", "PI_SKIP_VERSION_CHECK": "1", "PI_TELEMETRY": "0",
            "CANDID_SOCKET": channel.socketPath, "CANDID_PI_TOKEN": run.token,
            "CANDID_PI_TOOLS": run.tools.joined(separator: ","), "CANDID_PI_MODEL": run.model.model_id,
        ]
        let stdin = Pipe(), stdout = Pipe(), stderr = Pipe()
        process.standardInput = stdin
        process.standardOutput = stdout
        process.standardError = stderr
        let log = directory.appendingPathComponent("pi.log")
        if !fm.fileExists(atPath: log.path) { fm.createFile(atPath: log.path, contents: nil, attributes: [.posixPermissions: 0o600]) }
        let logHandle = try? FileHandle(forWritingTo: log)
        _ = try? logHandle?.seekToEnd()
        stderr.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            if data.isEmpty { handle.readabilityHandler = nil; try? logHandle?.close() } else { try? logHandle?.write(contentsOf: data) }
        }
        stdout.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            if data.isEmpty { handle.readabilityHandler = nil; return }
            Task { @MainActor in self?.receive(data) }
        }
        process.terminationHandler = { [weak self] ended in
            let status = ended.terminationStatus
            Task { @MainActor in self?.exited(ended, status: status) }
        }
        try process.run()
        self.process = process
        input = stdin.fileHandleForWriting
        buffer = PiLineBuffer()
        launchedAt = Date()
        onEvent?(.ready(model: run.model.model_id))
    }

    private func send(_ command: [String: Any]) throws {
        guard let input else { throw PiAgentError.notRunning }
        var data = try JSONSerialization.data(withJSONObject: command)
        data.append(0x0A)
        try input.write(contentsOf: data)
    }

    private func receive(_ data: Data) {
        for line in buffer.append(data) {
            for event in PiRecord.events(line) {
                if case .settled = event {
                    busy = false
                    if let id = run?.run_id, let channel = channel() {
                        Task { _ = try? await channel.send("POST", "/local/pi/idle", body: JSONSerialization.data(withJSONObject: ["run_id": id])) }
                    }
                }
                onEvent?(event)
            }
        }
    }

    /// Pi went away. Unless the App stopped it, restart with backoff; the run (and its token) is reused while the core
    /// still holds it, otherwise the same configuration is frozen again.
    private func exited(_ ended: Process, status: Int32) {
        guard ended === process, !stopping else { return }
        process = nil; input = nil
        if busy {
            busy = false
            onEvent?(.error("Pi 意外退出（\(status)），这一轮没有完成"))
            onEvent?(.settled(aborted: true))
        }
        if Date().timeIntervalSince(launchedAt) > 60 { restarts = 0 }
        let delay = backoff(restarts)
        restarts += 1
        onEvent?(.restarting(after: delay, reason: "Pi 退出（\(status)）"))
        restartTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(delay))
            guard let self, !Task.isCancelled, !self.stopping else { return }
            await self.relaunch()
        }
    }

    private func relaunch() async {
        guard let run, let config else { return }
        do {
            var current = run
            struct State: Decodable { struct Run: Decodable { var run_id: String; var stopped: String? }; var run: Run? }
            let state: State? = try? await channel()?.get("/local/pi/run")
            if state?.run?.run_id == run.run_id, let reason = state?.run?.stopped {
                // An exhausted budget is not reset by a crash.
                onEvent?(.error("本次运行已停止（\(reason)），请开始新的对话"))
                await stop()
                return
            }
            if state?.run?.run_id != run.run_id {
                current = try await freeze(config)
                self.run = current
            }
            try launch(current)
        } catch {
            onEvent?(.error("Pi 无法重启：\(error.localizedDescription)"))
            exitedWithoutProcess()
        }
    }

    private func exitedWithoutProcess() {
        let delay = backoff(restarts)
        restarts += 1
        onEvent?(.restarting(after: delay, reason: "重启失败"))
        restartTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(delay))
            guard let self, !Task.isCancelled, !self.stopping else { return }
            await self.relaunch()
        }
    }
}
