import CandidClient
import CandidLocalization
import DesktopCore
import DesktopPlatform
import Foundation
import Observation
import SwiftUI

/// The app's own background work. Each case is registered with the same name and a cost in the core's
/// `job_priority.rs`; a name here without a line there fails the core's `every_job_kind_is_registered` test.
enum AppBackgroundWork: String {
    /// System OCR of a captured or stored screenshot.
    case ocr = "app.ocr"
    /// Screenshot analysis (or OCR) through the 「图像理解」/「文字识别」 role's model.
    case vision = "app.vision"
}

/// 后台处理: when background work may spend power. The core keeps it and decides (power v1 §2.1).
enum BackgroundPace: String, CaseIterable {
    case anytime, saver, manual

    var title: String {
        switch self {
        case .anytime: L("随时")
        case .saver: L("省电")
        case .manual: L("手动")
        }
    }

    var detail: String {
        switch self {
        case .anytime: L("数据一到就处理；Mac 过热时暂停，低电量模式下按省电处理。")
        case .saver: L("用电池时不跑本机模型，接上电源后自动继续；远端模型和整理照常。")
        case .manual: L("不自动处理；在等待的条目上点「现在处理」才会运行。")
        }
    }
}

/// Reports the Mac's power state to the core, keeps the pace and the day's energy line, and asks the core's gate
/// before the app's own automatic work. The core's `power_allows` is the only decision; nothing is decided here.
@MainActor @Observable
final class BackgroundPower {
    static let shared = BackgroundPower()
    /// Posted when the core publishes `power` (the gate's decision changed) or the pace changed here.
    static let changed = Notification.Name("Candid.backgroundPowerChanged")
    /// Waiting reasons the gate gives.
    static let reasons: Set<String> = ["waiting_power", "waiting_thermal", "waiting_manual"]

    private(set) var pace: BackgroundPace = .saver
    /// Why heavy work waits right now (`waiting_power`, `waiting_thermal`, `waiting_manual`), or nil.
    private(set) var reason: String?
    private(set) var todayCPUSeconds: Double = 0
    private(set) var waiting = 0
    private(set) var error: String?

    @ObservationIgnored private var observation: DesktopRecognitionPowerObservation?
    @ObservationIgnored private var heartbeat: Task<Void, Never>?
    @ObservationIgnored private var lastReport: DesktopRecognitionPower?

    private var channel: LocalChannel? { TimelineService.shared.channel }

    /// Starts reporting: on every power, thermal or Low Power Mode change, on wake, and every 5 minutes so the core's
    /// copy never goes stale (it counts a report older than 10 minutes as unknown, which it treats as battery).
    func start() {
        guard observation == nil, !AIRuntime.isTesting else { return }
        observation = DesktopRecognitionPowerObservation { [weak self] _ in Task { await self?.report() } }
        heartbeat = Task { [weak self] in
            while !Task.isCancelled {
                await self?.report(force: true)
                try? await Task.sleep(for: .seconds(300))
            }
        }
    }

    /// The change feed said `hello` or `power`.
    func coreChanged() {
        Task { await report(force: true); await refresh() }
        NotificationCenter.default.post(name: Self.changed, object: nil)
    }

    func report(force: Bool = false) async {
        let power = DesktopRecognitionPowerSource.read()
        guard force || power != lastReport, let channel else { return }
        guard let body = try? JSONSerialization.data(withJSONObject: power.report(idleSeconds: DesktopRecognitionPowerSource.idleSeconds())) else { return }
        if let data = try? await channel.send("POST", "/local/power", body: body).checked() {
            lastReport = power
            apply(status: data)
        }
    }

    /// Status and the day's ledger: 「今天后台计算 X 分钟，等待 N 项」.
    func refresh() async {
        guard let channel else { return }
        do {
            apply(status: try await channel.send("POST", "/local/power", body: Data(#"{"action":"status"}"#.utf8)).checked())
            let data = try await channel.tool("energy_summary", arguments: Data(#"{"start":"today"}"#.utf8))
            let summary = try JSONSerialization.jsonObject(with: data) as? [String: Any]
            let days = summary?["days"] as? [[String: Any]] ?? []
            todayCPUSeconds = days.reduce(0) { $0 + (($1["cpu_seconds"] as? NSNumber)?.doubleValue ?? 0) }
            waiting = ((summary?["waiting"] as? [String: Any])?["total"] as? NSNumber)?.intValue ?? 0
            error = nil
        } catch { self.error = error.localizedDescription }
    }

    func setPace(_ pace: BackgroundPace) async {
        guard let channel, pace != self.pace else { return }
        let previous = self.pace
        self.pace = pace
        do {
            let body = try JSONSerialization.data(withJSONObject: ["action": "set_pace", "pace": pace.rawValue])
            apply(status: try await channel.send("POST", "/local/power", body: body).checked())
            NotificationCenter.default.post(name: Self.changed, object: nil)
            await refresh()
        } catch {
            self.pace = previous
            self.error = error.localizedDescription
        }
    }

    /// Asks the core's gate whether this automatic run may spend power now; nil means yes, otherwise the reason.
    /// `local`: whether the model runs on this Mac (only for model work). A manual run (「现在处理」) always may.
    func allows(_ work: AppBackgroundWork, local: Bool? = nil) async -> String? {
        guard let channel else { return "waiting_core" }
        var request: [String: Any] = ["action": "allows", "kind": work.rawValue, "manual": false]
        if let local { request["local"] = local }
        guard let body = try? JSONSerialization.data(withJSONObject: request),
              let data = try? await channel.send("POST", "/local/power", body: body).checked(),
              let answer = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return "waiting_core" }
        return answer["allowed"] as? Bool == true ? nil : (answer["reason"] as? String ?? "waiting_power")
    }

    /// Adds one run of the app's own work to the core's energy ledger. CPU is this process's CPU during the run,
    /// an estimate that also counts whatever else the app did meanwhile.
    func account(_ work: AppBackgroundWork, subject: String, since start: BackgroundPower.Mark) {
        let end = Mark.now()
        account(work, subject: subject, cpuMs: end.cpuMs - start.cpuMs, wallMs: end.wallMs - start.wallMs)
    }

    func account(_ work: AppBackgroundWork, subject: String, cpuMs: Int64, wallMs: Int64) {
        guard let channel, let body = try? JSONSerialization.data(withJSONObject: [
            "action": "account", "kind": work.rawValue, "subject": subject, "cpu_ms": max(0, cpuMs), "wall_ms": max(0, wallMs)]) else { return }
        Task { _ = try? await channel.send("POST", "/local/power", body: body) }
    }

    /// CPU and wall clock at one moment, for `account`.
    struct Mark: Sendable {
        let cpuMs: Int64
        let wallMs: Int64

        static func now() -> Mark {
            var usage = rusage()
            getrusage(RUSAGE_SELF, &usage)
            let cpu = Int64(usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) * 1000 + Int64(usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) / 1000
            return Mark(cpuMs: cpu, wallMs: Int64(Date().timeIntervalSince1970 * 1000))
        }
    }

    /// Whether capture should sample sparingly: on battery (or unknown power) unless the pace is 随时, and always in
    /// Low Power Mode. Follows the pace instead of a separate switch.
    func capturesSparingly(onBattery: Bool?, lowPower: Bool?) -> Bool {
        lowPower == true || (pace != .anytime && onBattery != false)
    }

    /// Whether the model assigned to `role` runs on this Mac (a loopback provider); nil when none is assigned.
    static func modelIsLocal(role: String) -> Bool? {
        let runtime = AIRuntime.shared
        guard let ref = runtime.model(for: role), let provider = runtime.config().provider(ref.providerID),
              let host = URL(string: provider.baseURL.trimmingCharacters(in: .whitespaces))?.host?.lowercased() else { return nil }
        return ["localhost", "127.0.0.1", "::1", "[::1]"].contains(host)
    }

    /// The user-facing words for a waiting reason the gate gives.
    static func label(_ reason: String) -> String {
        switch reason {
        case "waiting_power": L("等待接入电源")
        case "waiting_thermal": L("等待 Mac 降温")
        case "waiting_manual": L("等待手动处理")
        case "waiting_core": L("等待时间线核心")
        default: reason
        }
    }

    private func apply(status data: Data) {
        guard let status = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
        if let raw = status["pace"] as? String, let pace = BackgroundPace(rawValue: raw) { self.pace = pace }
        reason = status["reason"] as? String
        if status["changed"] as? Bool == true { NotificationCenter.default.post(name: Self.changed, object: nil) }
    }
}

/// The 「后台处理」 card at the top of the AI settings: the pace and one line of the day's ledger.
struct BackgroundPaceSettings: View {
    @ViewState private var power = BackgroundPower.shared

    private func choose(_ pace: BackgroundPace) {
        Task { @MainActor in await power.setPace(pace) }
    }

    var body: some View {
        SettingsGroup(title: L("后台处理")) {
            SettingRow(title: L("何时处理"), detail: power.pace.detail) {
                CapsuleChoice(options: BackgroundPace.allCases.map { ($0, $0.title) },
                              selection: Binding(get: { power.pace }, set: { pace in choose(pace) }))
            }
            Text(L("今天后台计算 %@ 分钟，等待 %@ 项", Int((power.todayCPUSeconds / 60).rounded(.up)), power.waiting))
                .typography(.secondary).foregroundStyle(.secondary)
                .accessibilityIdentifier("background-energy-line")
            if let reason = power.reason, reason != "waiting_manual" {
                InlineMessage(kind: .note, text: L("本机模型作业：%@", BackgroundPower.label(reason))).padding(.top, Space.xs)
            }
            if let error = power.error { InlineMessage(kind: .error, text: error).padding(.top, Space.xs) }
        }
        .task(id: TimelineService.shared.channel?.socketPath) { await power.refresh() }
        .onReceive(NotificationCenter.default.publisher(for: BackgroundPower.changed)) { _ in Task { await power.refresh() } }
    }
}

/// 「现在处理」 next to anything waiting on the power gate: runs that one item once, passing the power gate only
/// (authorization, sources and daily limits still apply).
struct ProcessNowButton: View {
    let reason: String?
    let action: () async -> Void
    @ViewState private var running = false

    var body: some View {
        if let reason, BackgroundPower.reasons.contains(reason) {
            SecondaryButton(title: running ? L("正在排队") : L("现在处理")) {
                running = true
                Task { await action(); running = false }
            }
            .disabled(running)
            .accessibilityIdentifier("process-now")
        }
    }
}
