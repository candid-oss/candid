import AIProviders
import AppKit
import CandidLocalization
import DesktopCore
import SwiftUI

/// Standing check of what keeps Candid working: permissions still granted (macOS may take Screen Recording back
/// after an update), the core connected, screenshot storage below its limit, chosen models reachable.
/// Permission and core checks are cheap and run every 20 s; models are probed every 5 minutes.
@MainActor @Observable final class HealthMonitor {
    static let shared = HealthMonitor()

    struct Item: Identifiable, Equatable {
        enum Fix: Equatable { case grant(SystemPermission), settings(String), raiseStorage }
        /// Shares ids with `StatusAlerts` ("screen", "ax", "storage") so the menu bar never shows a problem twice.
        let id: String
        let symbol: String
        let title: String
        let detail: String
        let ok: Bool
        var fix: Fix?
        var fixTitle: String?
    }

    private(set) var items: [Item] = []
    var problems: [Item] { items.filter { !$0.ok } }
    private(set) var checkedModels: Date?
    @ObservationIgnored private var models: Item?
    @ObservationIgnored private weak var session: DesktopSession?
    @ObservationIgnored private var loop: Task<Void, Never>?

    func start(session: DesktopSession) {
        self.session = session
        guard loop == nil, !ProcessInfo.processInfo.arguments.contains("--qa-snapshot-dir") else { return }
        loop = Task { [weak self] in
            var tick = 0
            while !Task.isCancelled {
                if tick % 15 == 0 { await self?.probeModels() }
                self?.refresh()
                tick += 1
                try? await Task.sleep(for: .seconds(20))
            }
        }
    }

    func recheck() { Task { await probeModels(); refresh() } }

    func refresh() {
        guard let session else { return }
        var list: [Item] = []
        if !session.isDemo {
            let screenNeeded = session.screenshotsEnabled
            let screen = SystemPermission.screen.state == .granted
            list.append(Item(id: "screen", symbol: SystemPermission.screen.symbol, title: L("屏幕录制"),
                             detail: !screenNeeded ? L("截图未开启，不需要") : screen ? L("已允许") : L("授权已丢失。macOS 更新或 Candid 更新后常会重新询问。"),
                             ok: !screenNeeded || screen, fix: .grant(.screen), fixTitle: L("授权")))
            let axNeeded = session.textEnabled || session.screenshotsEnabled || session.isRecording
            let ax = SystemPermission.accessibility.state == .granted
            list.append(Item(id: "ax", symbol: SystemPermission.accessibility.symbol, title: L("辅助功能"),
                             detail: !axNeeded ? L("暂不需要") : ax ? L("已允许") : L("授权已丢失，无法识别窗口和读取文字。"),
                             ok: !axNeeded || ax, fix: .grant(.accessibility), fixTitle: L("授权")))
        }
        list.append(core)
        if !session.isDemo { list.append(storage(session.storageBudget)) }
        if let models { list.append(models) }
        if list != items { items = list }
    }

    private var core: Item {
        let phase = TimelineService.shared.phase
        let (ok, detail): (Bool, String) = switch phase {
        case .ready: (true, L("已连接"))
        case .syncing: (true, L("首次同步中"))
        case .opening: (true, L("正在连接"))
        case .off: (false, L("核心没有启动，新记录不会写入时间线。"))
        case .failed(let message): (false, message)
        }
        return Item(id: "core", symbol: "cpu", title: L("核心"), detail: detail, ok: ok, fix: .settings("storage"), fixTitle: L("查看"))
    }

    private func storage(_ budget: DesktopStorageBudget) -> Item {
        let used = ByteCountFormatter.string(fromByteCount: budget.usedBytes, countStyle: .file)
        let limit = ByteCountFormatter.string(fromByteCount: budget.limitBytes, countStyle: .file)
        let canRaise = budget.raisedLimit != nil
        return Item(id: "storage", symbol: "internaldrive", title: L("截图存储"),
                    detail: budget.message ?? L("已用 %@ / %@", used, limit), ok: budget.level == .ok,
                    fix: canRaise ? .raiseStorage : .settings("storage"), fixTitle: canRaise ? L("提高上限") : L("管理"))
    }

    /// Probes the sources behind the three model roles. Cloud sources are not probed (that would need their key,
    /// and the keychain may ask); local and self-hosted ones get one unauthenticated request.
    private func probeModels() async {
        let config = AIRuntime.shared.config()
        let refs = ["vision", "summary", "agent_chat"].compactMap { config.assignment($0)?.model }
        let providers = Set(refs.map(\.providerID)).compactMap { id in config.providers.first { $0.providerID == id } }
        checkedModels = Date()
        guard session?.isDemo == false, !providers.isEmpty else { models = nil; return }
        var unreachable: [String] = []
        for provider in providers where provider.destination != .cloud && provider.enabled {
            let path = provider.api == .ollama ? "/api/tags" : "/models"
            let base = provider.baseURL.hasSuffix("/") ? String(provider.baseURL.dropLast()) : provider.baseURL
            let (found, _) = await OnboardingModel.probe(base + path, timeout: 3)
            if !found { unreachable.append(provider.name) }
        }
        models = Item(id: "models", symbol: "sparkles", title: L("模型"),
                      detail: unreachable.isEmpty ? L("已分配的模型源可以连上") : L("连不上：%@", unreachable.joined(separator: L("、"))),
                      ok: unreachable.isEmpty, fix: .settings("models"), fixTitle: L("设置"))
    }

    func perform(_ fix: Item.Fix) {
        guard let session else { return }
        switch fix {
        case .grant(let permission):
            Task { await permission.request(session: session); refresh() }
        case .settings(let tab): OnboardingLinks.openSettings(tab)
        case .raiseStorage: session.raiseStorageLimit(); refresh()
        }
    }
}

/// Problems only, for the menu bar panel; each row fixes in one click. Skips what `StatusAlerts` already shows.
struct HealthProblemsCard: View {
    var excluding: Set<String> = []
    var openApp: () -> Void = {}
    private var monitor: HealthMonitor { .shared }

    var body: some View {
        let problems = monitor.problems.filter { !excluding.contains($0.id) }
        if !problems.isEmpty {
            VStack(spacing: 6) {
                ForEach(problems) { item in
                    HStack(alignment: .top, spacing: 9) {
                        Image(systemName: item.symbol).typography(.secondary, .semibold).foregroundStyle(Palette.warning).frame(width: 16)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(item.title).typography(.secondary, .semibold)
                            Text(item.detail).typography(.caption).foregroundStyle(.secondary).lineLimit(3)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        if let fix = item.fix, let title = item.fixTitle {
                            Button(title) {
                                if case .settings = fix { openApp() }
                                monitor.perform(fix)
                            }
                            .buttonStyle(.borderless).typography(.secondary, .semibold)
                        }
                    }
                    .padding(10)
                    .background(Palette.warning.opacity(0.1), in: RoundedRectangle(cornerRadius: Radius.medium, style: .continuous))
                }
            }
        }
    }
}

/// Every check with its state, for 设置 → 关于.
struct HealthSettingsGroup: View {
    private var monitor: HealthMonitor { .shared }

    var body: some View {
        SettingsGroup(title: L("权限与健康")) {
            ForEach(monitor.items) { item in
                SettingRow(title: item.title, detail: item.detail) {
                    StatusDot(state: item.ok ? .on : .attention, text: item.ok ? L("正常") : L("需要处理"))
                    if !item.ok, let fix = item.fix, let title = item.fixTitle {
                        QuietButton(title: title, emphasized: true) { monitor.perform(fix) }
                    }
                }
                if item.id != monitor.items.last?.id { HairlineDivider() }
            }
        } accessory: {
            QuietButton(title: L("重新检查")) { monitor.recheck() }
        }
        .onAppear { monitor.refresh() }
    }
}
