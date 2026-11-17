import Foundation
import Observation
import CandidClient

/// The timeline library as the App sees it (timeline-v1 §2.1, ADR-024): the Rust core runs as the helper
/// `Contents/Helpers/candid serve`, owns the database, mirrors the media library every half minute, and serves
/// the query tools on 127.0.0.1 for the `candid` command and MCP clients. The App talks to it over the local
/// channel (a Unix socket in the run directory) and refreshes when the change feed says so. Desktop capture writes
/// into the core directly once the core has imported and checked the old desktop ledger (the cutover, §9.1).
@MainActor @Observable final class TimelineService: DesktopCoreAccess {
    static let shared = TimelineService()

    enum Phase: Equatable { case off, opening, syncing, ready, failed(String) }

    struct Verification: Equatable {
        var ok: Bool
        var hashed: Bool
        /// Per ledger kind: ledger count and timeline count, only where they differ.
        var differences: [String]
        var blobsChecked: Int
        var at: Date
    }

    struct ClientSetup: Equatable {
        var name: String
        var token: String
        var claudeCode: String
        var codex: String
        var json: String
    }

    /// Where the core keeps its data and where the App keeps its capture state.
    struct Locations: Equatable {
        var root: URL
        var runDirectory: URL
        /// The old desktop ledger (imported once, then kept read-only as a backup).
        var ledger: URL?
        var hub: URL?
        /// The App's capture settings and outbox.
        var capture: URL
    }

    private(set) var phase: Phase = .off
    /// `ledger` until the cutover passed, then `direct`.
    private(set) var captureMode = "ledger"
    private(set) var cutover: CoreCutover?
    /// Why the switch to direct capture did not happen (shown under 设置 → 数据 → 时间线库).
    private(set) var cutoverError: String?
    private(set) var cuttingOver = false
    /// The capture's outbox, for the pending count and delivery problems.
    @ObservationIgnored weak var desktopStore: DesktopStore?
    var captureDirect: Bool { captureMode == "direct" }
    private(set) var rootPath = ""
    private(set) var events = 0
    private(set) var lastSync: Date?
    private(set) var lastChange: String?
    private(set) var syncProblems: [String] = []
    private(set) var verification: Verification?
    private(set) var endpoint: String?
    private(set) var clients: [ClientInfo] = []
    /// Bumped whenever the core reports added or removed events; views that read the timeline can observe it.
    private(set) var dataVersion = 0

    @ObservationIgnored private var supervisor: CoreSupervisor?
    @ObservationIgnored private var feed: Task<Void, Never>?
    @ObservationIgnored private var statusTask: Task<Void, Never>?
    @ObservationIgnored private var statusDirty = false

    static var cliPath: String { Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/candid").path }

    /// The helper inside the App bundle; `CANDID_CORE_BIN` points a development run at a cargo build.
    static var corePath: URL {
        if let custom = ProcessInfo.processInfo.environment["CANDID_CORE_BIN"], !custom.isEmpty { return URL(fileURLWithPath: custom) }
        return URL(fileURLWithPath: cliPath)
    }

    /// Demo, self-test and QA launches (`--mock`, `--data-dir`, `--qa-snapshot-dir` or `CANDID_RUN_DIR`) use a
    /// throwaway data root and run directory, so they never touch the real library or replace the real app's endpoint.
    static func isolated(isDemo: Bool, arguments: [String] = ProcessInfo.processInfo.arguments,
                         environment: [String: String] = ProcessInfo.processInfo.environment) -> Bool {
        isDemo || arguments.contains("--data-dir") || arguments.contains("--qa-snapshot-dir") || !(environment["CANDID_RUN_DIR"] ?? "").isEmpty
    }

    /// The real locations, or for demo, self-test and QA launches a throwaway root and run directory; `--data-dir`
    /// names an old desktop ledger (usually a copy) for the throwaway core to import.
    static func locations(isDemo: Bool, dataRoot: URL?, mediaProfile: URL,
                          environment: [String: String] = ProcessInfo.processInfo.environment) -> Locations {
        let hub = mediaProfile.appendingPathComponent("Hub", isDirectory: true)
        if isolated(isDemo: isDemo, environment: environment) || dataRoot != nil {
            let scratch = FileManager.default.temporaryDirectory.appendingPathComponent("candid-timeline-demo-" + UUID().uuidString, isDirectory: true)
            let run = environment["CANDID_RUN_DIR"].flatMap { $0.isEmpty ? nil : URL(fileURLWithPath: $0, isDirectory: true) }
                ?? scratch.appendingPathComponent("run", isDirectory: true)
            return Locations(root: scratch.appendingPathComponent("timeline", isDirectory: true), runDirectory: run, ledger: dataRoot, hub: hub,
                             capture: scratch.appendingPathComponent("capture", isDirectory: true))
        }
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("Candid", isDirectory: true)
        return Locations(root: support.appendingPathComponent("timeline", isDirectory: true), runDirectory: RunDirectory.url(),
                         ledger: support.appendingPathComponent("desktop", isDirectory: true), hub: hub,
                         capture: support.appendingPathComponent("capture", isDirectory: true))
    }

    /// Starts once per launch.
    func start(_ locations: Locations) {
        guard phase == .off else { return }
        rootPath = locations.root.path
        phase = .opening
        let supervisor = CoreSupervisor(launch: CoreLaunch(executable: Self.corePath, root: locations.root, runDirectory: locations.runDirectory,
                                                           ledger: locations.ledger, hub: locations.hub))
        supervisor.onChange = { [weak self] state in self?.coreChanged(state) }
        self.supervisor = supervisor
        supervisor.start()
    }

    /// Called when the App quits: stops the core this App started (it finishes the pass it is in).
    func stop() {
        feed?.cancel(); feed = nil
        statusTask?.cancel(); statusTask = nil
        supervisor?.stop()
    }

    var channel: LocalChannel? {
        guard let supervisor, case .running = supervisor.state else { return nil }
        return supervisor.channel
    }

    private func coreChanged(_ state: CoreSupervisor.State) {
        switch state {
        case .idle, .starting:
            if case .failed = phase { phase = .opening } else if phase == .off { phase = .opening }
        case .running:
            subscribe()
        case .restarting(let after, let reason):
            feed?.cancel(); feed = nil
            endpoint = nil
            phase = .failed("核心进程已退出（\(reason)），\(Int(after.rounded())) 秒后重启。")
        case .failed(let message):
            endpoint = nil
            phase = .failed(message)
        case .stopped:
            endpoint = nil
        }
    }

    /// Follows the change feed while the core answers; a closed feed ends when the core goes away.
    private func subscribe() {
        guard let channel else { return }
        feed?.cancel()
        feed = Task { [weak self] in
            for await event in channel.events() {
                guard let self else { return }
                switch event.type {
                case "hello":
                    self.refreshStatus()
                    Task { try? await AIRuntime.shared.syncCore() }
                    if !AIRuntime.isTesting { Task { await ConnectorsModel().pushSecrets() } }
                    await self.refreshClients()
                    self.desktopStore?.coreChanged()
                    Task { await self.ensureCutover() }
                case "status": self.refreshStatus()
                case "clients": await self.refreshClients()
                case "data": self.dataVersion += 1; self.refreshStatus(); self.desktopStore?.coreChanged()
                default: break
                }
            }
        }
    }

    /// Coalesces status reads: the core publishes several status events per pass.
    private func refreshStatus() {
        statusDirty = true
        guard statusTask == nil, let channel else { return }
        statusTask = Task { [weak self] in
            while let self, self.statusDirty, !Task.isCancelled {
                self.statusDirty = false
                if let status = try? await channel.status() { self.apply(status) }
            }
            self?.statusTask = nil
        }
    }

    private func apply(_ status: CoreStatus) {
        endpoint = status.endpoint
        if let root = status.root { rootPath = root }
        events = status.events
        lastSync = status.last_sync_ms.map { Date(timeIntervalSince1970: Double($0) / 1000) }
        lastChange = status.last_change
        syncProblems = status.problems
        verification = status.verification.map(Self.verification)
        let wasDirect = captureDirect
        captureMode = status.capture_mode ?? "ledger"
        cutover = status.cutover ?? cutover
        cutoverError = status.cutover_error
        if captureDirect && !wasDirect { desktopStore?.coreChanged() }
        switch status.phase {
        case "ready": phase = .ready
        case "syncing": phase = .syncing
        default: phase = .opening
        }
    }

    private static func verification(_ v: CoreVerification) -> Verification {
        Verification(ok: v.ok, hashed: v.hashed, differences: v.differences, blobsChecked: v.blobs_checked,
                     at: Date(timeIntervalSince1970: Double(v.at_ms) / 1000))
    }

    // MARK: Sync

    func syncNow() {
        guard let channel else { return }
        Task { try? await channel.syncNow() }
    }

    /// The switch to direct capture: the core imports what is left of the old ledger, checks every record and
    /// screenshot hash, and only then accepts desktop batches. Until it passes, capture waits in the outbox.
    func ensureCutover() async {
        guard !captureDirect, !cuttingOver, let channel else { return }
        cuttingOver = true
        defer { cuttingOver = false }
        do {
            cutover = try await channel.cutover()
            captureMode = "direct"; cutoverError = nil
            desktopStore?.coreChanged()
        } catch CoreError.service(_, _, let message) {
            cutoverError = message
        } catch {
            cutoverError = "未切换到直写：" + error.localizedDescription
        }
        if let status = try? await channel.status() { apply(status) }
    }

    /// 重新核对: before the switch it retries the cutover; afterwards it compares the read-only ledger backup with the
    /// timeline (records deleted or expired in the timeline since count as accounted for).
    func verify(hashBlobs: Bool = true) async {
        guard let channel else { return }
        if !captureDirect { await ensureCutover(); return }
        do { verification = Self.verification(try await channel.verify(hashBlobs: hashBlobs)) }
        catch CoreError.service(409, _, _) { return } // no desktop ledger to compare with
        catch {
            verification = Verification(ok: false, hashed: hashBlobs, differences: [error.localizedDescription], blobsChecked: 0, at: Date())
        }
    }

    // MARK: Clients

    func refreshClients() async {
        guard let channel else { return }
        if let list = try? await channel.clients() { clients = list }
    }

    /// Registers an MCP client and returns its token once, with ready-to-paste setup for common AI clients.
    func addClient(name: String) async throws -> ClientSetup {
        guard let channel else { throw CoreError.unreachable("时间线库尚未打开") }
        let setup = try await channel.registerClient(name: name)
        await refreshClients()
        return ClientSetup(name: setup.name, token: setup.token, claudeCode: setup.claude_code, codex: setup.codex, json: setup.json)
    }

    func revoke(_ clientID: String) async {
        guard let channel else { return }
        try? await channel.revokeClient(clientID)
        await refreshClients()
    }
}
