import Darwin
import CandidLocalization
import Foundation

/// The per-user run directory: `~/Library/Application Support/Candid/run`, or `CANDID_RUN_DIR` when set.
/// The core writes `endpoint.json` and `cli.token` there for the `candid` command and MCP clients.
public enum RunDirectory {
    public static func url(environment: [String: String] = ProcessInfo.processInfo.environment) -> URL {
        if let custom = environment["CANDID_RUN_DIR"], !custom.isEmpty { return URL(fileURLWithPath: custom, isDirectory: true) }
        // A test process never reaches the user's running core: without its own run directory it gets an empty one.
        if isTestProcess(environment) {
            return FileManager.default.temporaryDirectory.appendingPathComponent("candid-test-run-\(getpid())", isDirectory: true)
        }
        return FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Candid/run", isDirectory: true)
    }

    static func isTestProcess(_ environment: [String: String]) -> Bool {
        let name = ProcessInfo.processInfo.processName
        return environment["XCTestSessionIdentifier"] != nil || environment["XCTestConfigurationFilePath"] != nil
            || name == "xctest" || name.hasPrefix("swiftpm-testing-helper") || Bundle.main.bundlePath.hasSuffix(".xctest")
            || NSClassFromString("XCTestCase") != nil
    }

    public static func endpointFile(in dir: URL) -> URL { dir.appendingPathComponent("endpoint.json") }
    public static func cliTokenFile(in dir: URL) -> URL { dir.appendingPathComponent("cli.token") }

    /// `<run>/core.sock`, or, when that path is too long for a Unix socket (104 bytes), a name derived from the run
    /// directory in this user's private temporary directory (0700).
    public static func socket(in dir: URL, temporary: URL = FileManager.default.temporaryDirectory) -> URL {
        let inRun = dir.appendingPathComponent("core.sock")
        if inRun.path.utf8.count <= 100 { return inRun }
        var hash: UInt64 = 0xcbf2_9ce4_8422_2325 // FNV-1a: stable across launches, no dependencies
        for byte in dir.standardizedFileURL.path.utf8 { hash = (hash ^ UInt64(byte)) &* 0x0000_0100_0000_01b3 }
        return temporary.appendingPathComponent("candid-" + String(hash, radix: 16) + ".sock")
    }
}

/// How to start the core (`candid serve`) as the App's helper.
public struct CoreLaunch: Sendable, Equatable {
    public var executable: URL
    public var root: URL
    public var runDirectory: URL
    public var socket: URL
    public var ledger: URL?
    public var hub: URL?
    public var syncInterval: Int?

    public init(executable: URL, root: URL, runDirectory: URL, socket: URL? = nil, ledger: URL? = nil, hub: URL? = nil, syncInterval: Int? = nil) {
        self.executable = executable; self.root = root; self.runDirectory = runDirectory
        self.socket = socket ?? RunDirectory.socket(in: runDirectory)
        self.ledger = ledger; self.hub = hub; self.syncInterval = syncInterval
    }

    public var arguments: [String] {
        var args = ["serve", "--root", root.path, "--run-dir", runDirectory.path, "--socket", socket.path,
                    "--parent-pid", String(getpid())]
        if let ledger { args += ["--ledger", ledger.path] }
        if let hub { args += ["--hub", hub.path] }
        if let syncInterval { args += ["--sync-interval", String(syncInterval)] }
        return args
    }
}

/// Keeps one core answering on the socket. It starts `candid serve`, restarts it with backoff when it exits
/// (1, 2, 4 … 60 s; reset after a minute of healthy running), and stops it on request. A healthy core already on
/// the socket (another instance of the App) is used as is and never started twice; the data root's lock refuses a
/// second writer anyway (exit 75), which is retried like any other exit.
@MainActor public final class CoreSupervisor {
    public enum State: Equatable, Sendable {
        case idle
        case starting
        /// `owned`: this App started it and will stop it.
        case running(pid: Int32, owned: Bool)
        case restarting(after: Double, reason: String)
        case failed(String)
        case stopped
    }

    public private(set) var state: State = .idle { didSet { if state != oldValue { onChange?(state) } } }
    public var onChange: ((State) -> Void)?
    public let launch: CoreLaunch
    public let channel: LocalChannel
    public private(set) var restarts = 0
    /// Delay before the n-th consecutive restart (n from 0).
    public var backoff: (Int) -> Double = { min(60, pow(2, Double($0))) }
    /// How long a started core may take to answer before it is considered stuck and terminated.
    public var startupTimeout: Double = 60
    public private(set) var process: Process?
    private var loop: Task<Void, Never>?
    private var stopping = false
    private let log: CoreLog

    public init(launch: CoreLaunch) {
        self.launch = launch
        channel = LocalChannel(socketPath: launch.socket.path)
        log = CoreLog(file: launch.runDirectory.appendingPathComponent("core.log"))
    }

    public var lastLogLine: String? { log.lastLine }

    public func start() {
        guard loop == nil else { return }
        stopping = false
        loop = Task { [weak self] in await self?.run() }
    }

    /// Stops the core this App started: SIGTERM, then up to `wait` seconds for it to finish its current pass. A core
    /// that is still busy exits on its own afterwards (it also watches for its parent going away).
    public func stop(wait: TimeInterval = 2) {
        stopping = true
        loop?.cancel(); loop = nil
        if let process, process.isRunning {
            process.terminate()
            let deadline = Date().addingTimeInterval(wait)
            while process.isRunning && Date() < deadline { usleep(20_000) }
        }
        process = nil
        state = .stopped
    }

    private func run() async {
        var failures = 0
        while !Task.isCancelled && !stopping {
            if let health = try? await channel.health(), health.ok {
                // Someone else's core (another instance of the App): use it while it answers.
                state = .running(pid: health.pid, owned: false)
                while !Task.isCancelled && !stopping, (try? await channel.health()) != nil {
                    try? await Task.sleep(for: .seconds(2))
                }
                continue
            }
            guard FileManager.default.isExecutableFile(atPath: launch.executable.path) else {
                state = .failed(L("找不到核心程序：%@", launch.executable.path))
                return
            }
            state = .starting
            let started = Date()
            let exit: AsyncStream<Int32>
            do { exit = try spawn() } catch {
                state = .failed(L("无法启动核心：%@", error.localizedDescription))
                return
            }
            var healthy = false
            var exitCode: Int32?
            var iterator = exit.makeAsyncIterator()
            // Wait until it answers, exits, or takes too long.
            while !Task.isCancelled && !stopping {
                if process?.isRunning != true { exitCode = await iterator.next(); break }
                if let health = try? await channel.health(timeout: 1), health.pid == process?.processIdentifier {
                    healthy = true; break
                }
                if Date().timeIntervalSince(started) > startupTimeout { process?.terminate() }
                try? await Task.sleep(for: .milliseconds(100))
            }
            if healthy, let pid = process?.processIdentifier {
                state = .running(pid: pid, owned: true)
                exitCode = await iterator.next()
            }
            if stopping || Task.isCancelled { break }
            if Date().timeIntervalSince(started) > 60 { failures = 0 }
            let delay = backoff(failures)
            failures += 1; restarts += 1
            let code = exitCode.map { $0 == 75 ? L("另一个核心正在运行") : L("退出码 %@", $0) } ?? L("已退出")
            let detail = log.lastLine.map { L("%@：%@", code, $0) } ?? code
            state = .restarting(after: delay, reason: detail)
            try? await Task.sleep(for: .seconds(delay))
        }
        if stopping { state = .stopped }
    }

    private func spawn() throws -> AsyncStream<Int32> {
        let (stream, continuation) = AsyncStream<Int32>.makeStream(bufferingPolicy: .bufferingNewest(1))
        let process = Process()
        process.executableURL = launch.executable
        process.arguments = launch.arguments
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = FileHandle.nullDevice
        let pipe = Pipe()
        process.standardError = pipe
        let log = log
        log.beginRun()
        pipe.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            if data.isEmpty { handle.readabilityHandler = nil } else { log.append(data) }
        }
        process.terminationHandler = { p in
            continuation.yield(p.terminationStatus)
            continuation.finish()
        }
        try process.run()
        self.process = process
        return stream
    }
}

/// The core's stderr (counts and states only) in `<run>/core.log`, rotated at 512 KB; the last line is kept for the
/// status shown after an exit.
final class CoreLog: @unchecked Sendable {
    let file: URL
    private let lock = NSLock()
    private var last: String?

    init(file: URL) { self.file = file }

    var lastLine: String? { lock.lock(); defer { lock.unlock() }; return last }

    func beginRun() {
        lock.lock(); defer { lock.unlock() }
        last = nil
        if let size = try? FileManager.default.attributesOfItem(atPath: file.path)[.size] as? Int, size > 512 * 1024 {
            let old = file.appendingPathExtension("1")
            try? FileManager.default.removeItem(at: old)
            try? FileManager.default.moveItem(at: file, to: old)
        }
    }

    func append(_ data: Data) {
        lock.lock(); defer { lock.unlock() }
        if let line = String(decoding: data, as: UTF8.self).split(separator: "\n").last.map(String.init), !line.isEmpty { last = line }
        if !FileManager.default.fileExists(atPath: file.path) {
            FileManager.default.createFile(atPath: file.path, contents: nil, attributes: [.posixPermissions: 0o600])
        }
        guard let handle = try? FileHandle(forWritingTo: file) else { return }
        defer { try? handle.close() }
        _ = try? handle.seekToEnd()
        try? handle.write(contentsOf: data)
    }
}
