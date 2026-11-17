import DesktopCore
import Foundation
import CandidClient
@testable import Candid

/// A real timeline core (`CANDID_CORE_BIN`, built by script/test.sh) per synthetic test folder: the session's
/// capture state lives in the folder, the core's data root and run directory below it.
@MainActor final class TestCore: DesktopCoreAccess {
    private static var cores: [String: TestCore] = [:]
    let supervisor: CoreSupervisor
    var captureDirect = false

    var channel: LocalChannel? {
        if case .running = supervisor.state { return supervisor.channel }
        return nil
    }

    private init(root: URL, ledger: URL?) {
        let binary = URL(fileURLWithPath: ProcessInfo.processInfo.environment["CANDID_CORE_BIN"] ?? "/nonexistent/candid")
        supervisor = CoreSupervisor(launch: CoreLaunch(executable: binary, root: root.appendingPathComponent("timeline", isDirectory: true),
                                                       runDirectory: root.appendingPathComponent("run", isDirectory: true), ledger: ledger))
    }

    /// The running core for `root`, switched to direct capture unless `cutover` is false.
    static func ready(_ root: URL, cutover: Bool = true, ledger: URL? = nil) async throws -> TestCore {
        if let core = cores[root.path] { return core }
        let core = TestCore(root: root, ledger: ledger)
        cores[root.path] = core
        core.supervisor.start()
        let deadline = ContinuousClock.now.advanced(by: .seconds(20))
        while core.channel == nil {
            if case .failed(let message) = core.supervisor.state { throw DesktopStoreError.unavailable(message) }
            guard ContinuousClock.now < deadline else { throw DesktopStoreError.unavailable("测试核心没有就绪") }
            try await Task.sleep(for: .milliseconds(20))
        }
        if cutover { try await core.cutover() }
        return core
    }

    func cutover() async throws {
        _ = try await channel!.cutover()
        captureDirect = true
    }

    /// Stops the folder's core and removes the folder.
    static func finish(_ root: URL) {
        cores.removeValue(forKey: root.path)?.supervisor.stop()
        try? FileManager.default.removeItem(at: root)
    }
}

/// Waits until the session's outbox under `root` is empty (everything reached the core).
@MainActor func drained(_ root: URL, timeout: Duration = .seconds(10)) async throws {
    let outbox = root.appendingPathComponent("outbox")
    let deadline = ContinuousClock.now.advanced(by: timeout)
    while ContinuousClock.now < deadline {
        let entries = (try? FileManager.default.contentsOfDirectory(atPath: outbox.path))?.filter { $0.hasSuffix(".json") } ?? []
        if entries.isEmpty { return }
        try await Task.sleep(for: .milliseconds(20))
    }
    throw DesktopStoreError.unavailable("待写队列没有清空")
}

/// Everything the core holds for the desktop capture, newest first (what the tests used to read from the ledger).
@MainActor func storedRecords(_ root: URL, query: String = "") async throws -> [DesktopRecord] {
    try await drained(root)
    let channel = try await TestCore.ready(root).channel!
    var arguments: [String: CoreJSON] = ["start_ms": .int(0), "end_ms": .int(Int64(Date().timeIntervalSince1970 * 1000) + 86_400_000), "limit": .int(1000)]
    if !query.isEmpty { arguments["q"] = .string(query) }
    let day: AppActivities = try await channel.app("app_activities", arguments)
    var records: [DesktopRecord] = []
    for summary in day.activities {
        struct Answer: Decodable { var activity: AppActivity? }
        let detail: Answer = try await channel.app("app_activity", ["id": .string(summary.anchor.id), "limit": .int(10_000)])
        if let activity = detail.activity { records += DesktopEventMapping.event(activity).allRecords }
    }
    if !query.isEmpty {
        let needle = query.lowercased()
        records = records.filter { r in
            [r.appName, r.windowTitle, r.detail, r.context?.readableText ?? "", r.context?.selectedText ?? ""].contains { $0.lowercased().contains(needle) }
        }
    }
    return records.sorted { $0.timestamp > $1.timestamp }
}

@MainActor func storedStatistics(_ root: URL) async throws -> DesktopStatistics {
    try await drained(root)
    let day: AppActivities = try await TestCore.ready(root).channel!.app("app_activities", ["start_ms": .int(0), "end_ms": .int(1), "limit": .int(1)])
    return DesktopStore.statistics(day.stats)
}

/// Writes records straight into the core, as earlier capture would have left them.
@MainActor func writeDirectly(_ root: URL, _ records: [DesktopRecord]) async throws {
    let channel = try await TestCore.ready(root).channel!
    let source = try await DesktopDelivery.registerSource(channel)
    let events = try records.map { try DesktopEventMapping.event(for: $0).event }
    _ = try await channel.ingest(IngestBatch(source_id: source, events: events, on_missing_parent: "detach"))
}
