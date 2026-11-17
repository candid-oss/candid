import AppKit
import CandidLocalization
import Observation
import SwiftUI

/// State of the runtimes Candid does not ship (Settings → 组件).
@MainActor @Observable
final class ComponentsModel {
    struct VoiceStatus: Equatable { var running: Bool; var detail: String }

    private(set) var node: NodeRuntime?
    private(set) var nodeManaged = false
    private(set) var piVersion: String?
    private(set) var piPinned: String?
    private(set) var userPi: (version: String, location: String)?
    private(set) var piBytes: Int64 = 0
    private(set) var nodeBytes: Int64 = 0
    private(set) var voice = VoiceStatus(running: false, detail: L("正在检查…"))
    private(set) var working: String?
    var error: String?

    static let voiceGuide = URL(string: "https://github.com/candid-oss/candid/blob/main/services/voice/README.md")!

    func refresh() async {
        let snapshot = await Task.detached { () -> (NodeRuntime?, Bool, String?, String?, (String, String)?, Int64, Int64) in
            let node = NodeRuntime.resolve()
            let managed = NodeRuntime.version(of: NodeRuntime.managedNode) != nil
            return (node, managed, PiComponent.installedVersion(in: PiComponent.managedHome), PiComponent.pinned,
                    PiComponent.userInstall(), Components.size(of: PiComponent.managedHome), Components.size(of: NodeRuntime.managedDirectory))
        }.value
        node = snapshot.0; nodeManaged = snapshot.1; piVersion = snapshot.2; piPinned = snapshot.3
        userPi = snapshot.4.map { (version: $0.0, location: $0.1) }; piBytes = snapshot.5; nodeBytes = snapshot.6
        voice = await Self.checkVoice()
    }

    var piReady: Bool { piVersion != nil && piVersion == piPinned && node != nil }

    func installNode() { perform { progress in _ = try await NodeRuntime.installManaged(progress: progress) } }

    func removeNode() {
        perform { _ in try FileManager.default.removeItem(at: NodeRuntime.managedDirectory) }
    }

    func installPi() {
        perform { progress in
            var node = NodeRuntime.resolve()
            if node == nil { node = try await NodeRuntime.installManaged(progress: progress) }
            guard let node else { throw ComponentError.noNode }
            try await PiComponent.installManaged(node: node, progress: progress)
        }
    }

    func removePi() { perform { _ in try PiComponent.removeManaged() } }

    private func perform(_ job: @escaping @Sendable (@escaping @Sendable (String) -> Void) async throws -> Void) {
        guard working == nil else { return }
        error = nil
        working = L("正在准备…")
        Task {
            do {
                try FileManager.default.createDirectory(at: Components.root, withIntermediateDirectories: true)
                try await job { message in Task { @MainActor in self.working = message } }
            } catch {
                self.error = error.localizedDescription
            }
            working = nil
            await refresh()
        }
    }

    private static func checkVoice() async -> VoiceStatus {
        var request = URLRequest(url: URL(string: "http://127.0.0.1:8767/health")!)
        request.timeoutInterval = 1.5
        guard let (data, response) = try? await URLSession.shared.data(for: request),
              (response as? HTTPURLResponse)?.statusCode == 200 else {
            return VoiceStatus(running: false, detail: L("未运行。不安装也能录音、保存和回放；转写与说话人需要它。"))
        }
        let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        let model = (json?["model"] as? String) ?? (json?["model_id"] as? String)
        return VoiceStatus(running: true, detail: model.map { L("运行中 · %@", $0) } ?? L("运行中"))
    }
}

/// Settings → 组件: Node and Pi for the attached agent, and the local voice service.
struct ComponentsSettingsView: View {
    @ViewState private var model = ComponentsModel()

    var body: some View {
        SettingsPage(title: L("组件")) {
            Text(L("这些运行环境不随 App 打包，只在你需要时安装到「应用程序支持/Candid/components」，按固定版本下载并校验。"))
                .typography(.secondary).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            if let working = model.working { InlineMessage(kind: .note, text: working) }
            if let error = model.error { InlineMessage(kind: .error, text: error) }

            SettingsGroup(title: L("AI 助手运行环境")) {
                SettingRow(title: "Node", detail: nodeDetail) {
                    if model.nodeManaged {
                        QuietButton(title: L("移除"), role: .destructive) { model.removeNode() }
                    } else if model.node == nil {
                        SecondaryButton(title: L("下载 Node %@", NodeRuntime.pinned)) { model.installNode() }
                    }
                }
                HairlineDivider()
                SettingRow(title: "Pi", detail: piDetail) {
                    if model.piReady {
                        QuietButton(title: L("移除"), role: .destructive) { model.removePi() }
                    } else {
                        PrimaryButton(title: model.piVersion == nil ? L("安装") : L("更新到 %@", model.piPinned ?? "")) { model.installPi() }
                    }
                }
                if let userPi = model.userPi {
                    HairlineDivider()
                    SettingRow(title: L("你自己的 Pi"), detail: userPiDetail(userPi)) { EmptyView() }
                }
            }
            .disabled(model.working != nil)

            SettingsGroup(title: L("本地语音服务")) {
                SettingRow(title: model.voice.running ? L("已连接") : L("未连接"), detail: model.voice.detail) {
                    QuietButton(title: L("安装说明"), emphasized: !model.voice.running) { NSWorkspace.shared.open(ComponentsModel.voiceGuide) }
                }
            }
        }
        .task { await model.refresh() }
    }

    private var nodeDetail: String {
        guard let node = model.node else { return L("未找到 Node 22.19 或更新版本。安装 Pi 时会一并下载 Node %@。", NodeRuntime.pinned) }
        switch node.origin {
        case .managed: return L("%@ · Candid 下载 · %@", node.version, size(model.nodeBytes))
        case .system: return L("%@ · 使用系统已有的 %@", node.version, node.url.path)
        case .override: return L("%@ · 开发设置 CANDID_PI_NODE", node.version)
        }
    }

    private var piDetail: String {
        let pinned = model.piPinned ?? "?"
        guard let installed = model.piVersion else { return L("未安装。AI 页的对话和技能需要它（固定版本 %@）。", pinned) }
        if installed != pinned { return L("已安装 %@，Candid 需要 %@", installed, pinned) }
        return model.node == nil ? L("%@ · 缺少 Node", installed) : L("%@ · 已就绪 · %@", installed, size(model.piBytes))
    }

    private func userPiDetail(_ pi: (version: String, location: String)) -> String {
        PiComponent.isCompatible(pi.version)
            ? L("%@（%@）与 Candid 兼容；为保证隔离与扩展一致，AI 页仍使用上面的固定副本。", pi.version, pi.location)
            : L("%@（%@）与 Candid 需要的 %@ 不兼容，不会使用。", pi.version, pi.location, model.piPinned ?? "?")
    }

    private func size(_ bytes: Int64) -> String { ByteCountFormatter.string(fromByteCount: bytes, countStyle: .file) }
}
