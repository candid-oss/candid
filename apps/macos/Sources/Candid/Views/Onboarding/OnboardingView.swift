import AIProviders
import AppKit
import Combine
import CandidLocalization
import DesktopCore
import SwiftUI

/// Shows the first-run guide over the main window on first launch (never in a self-test copy), and again whenever
/// 关于 or the menu bar panel asks for it. `--onboarding` forces it, e.g. with `--mock`.
struct OnboardingGate: View {
    let session: DesktopSession
    let media: MediaWorkspaceStore
    @AppStorage(OnboardingModel.doneKey) private var done = false
    @ViewState private var model: OnboardingModel?

    private var shouldShow: Bool {
        let args = ProcessInfo.processInfo.arguments
        if args.contains("--onboarding") { return true }
        return !done && !session.isDemo && !args.contains("--qa-snapshot-dir")
    }

    var body: some View {
        ZStack {
            if let model {
                Color.black.opacity(0.28).ignoresSafeArea().onTapGesture { }
                OnboardingView(model: model, session: session, media: media) {
                    done = true
                    self.model = nil
                }
                .transition(.scale(scale: 0.96).combined(with: .opacity))
            }
        }
        .animation(Motion.spring, value: model == nil)
        .onAppear { if shouldShow, model == nil { model = OnboardingModel() } }
        .onReceive(NotificationCenter.default.publisher(for: .candidShowOnboarding)) { _ in model = OnboardingModel() }
        // Self-test only: `onboarding <step>` jumps to a page (0 welcome … 4 done).
        .onReceive(NotificationCenter.default.publisher(for: QASnapshot.command)) { note in
            guard let line = note.object as? String, line.hasPrefix("onboarding "),
                  let step = Int(line.dropFirst(11)).flatMap(OnboardingModel.Step.init(rawValue:)) else { return }
            if model == nil { model = OnboardingModel() }
            model?.step = step
        }
    }
}

struct OnboardingView: View {
    @Bindable var model: OnboardingModel
    let session: DesktopSession
    let media: MediaWorkspaceStore
    let close: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                StepDots(count: OnboardingModel.Step.allCases.count, current: model.step.rawValue)
                Spacer()
                if model.step != .done {
                    QuietButton(title: L("跳过引导")) { close() }
                }
            }
            .frame(height: ControlHeight.button)
            .padding(.bottom, Space.l)
            ScrollView {
                page.frame(maxWidth: .infinity, alignment: .leading)
            }
            .scrollIndicators(.hidden)
            .frame(maxHeight: .infinity)
            HairlineDivider().padding(.vertical, Space.m)
            footer
        }
        .padding(Space.xl)
        .frame(width: 600, height: 560)
        .floatingSurface(radius: Radius.card)
        .animation(Motion.snappy, value: model.step)
        .onChange(of: model.step) { _, step in
            if step == .ai { Task { await model.detectServers() } }
        }
    }

    @ViewBuilder private var page: some View {
        switch model.step {
        case .welcome: welcome
        case .capture: OnboardingCaptureStep(model: model, session: session)
        case .permissions: OnboardingPermissionStep(model: model, session: session)
        case .ai: OnboardingAIStep(model: model, session: session)
        case .done: finish
        }
    }

    private var footer: some View {
        HStack(spacing: Space.xs) {
            if model.step != .welcome && model.step != .done {
                QuietButton(title: L("上一步")) { move(-1) }
            }
            Spacer()
            switch model.step {
            case .done:
                QuietButton(title: L("稍后再说")) { close() }
                PrimaryButton(title: session.isRecording ? L("完成") : L("开始记录"), isDefault: true) {
                    if !session.isRecording { session.startRecording() }
                    close()
                }
                .disabled(session.isDemo && !session.isRecording)
            case .permissions:
                PrimaryButton(title: L("应用并继续"), isDefault: true) {
                    model.apply(session: session, media: media); move(1)
                }
            default:
                PrimaryButton(title: model.step == .welcome ? L("开始设置") : L("继续"), isDefault: true) { move(1) }
            }
        }
    }

    private func move(_ delta: Int) {
        if let next = OnboardingModel.Step(rawValue: model.step.rawValue + delta) { model.step = next }
    }

    private var welcome: some View {
        VStack(alignment: .leading, spacing: Space.m) {
            Image(nsImage: NSApp.applicationIconImage).resizable().frame(width: 64, height: 64)
            Text(L("欢迎使用 Candid")).typography(.title)
            Text(L("Candid 在这台 Mac 上记下你做过的事，整理成一条时间线，供你回看，也可以交给你信任的 AI 工具。所有资料只保存在本机，没有账号和遥测。"))
                .typography(.body).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            VStack(alignment: .leading, spacing: Space.s) {
                bullet("pause.circle", L("每次启动默认暂停，由你决定何时开始记录。"))
                bullet("lock", L("只记录你允许的应用；密码框和隐私窗口自动跳过。"))
                bullet("record.circle", L("菜单栏圆点始终显示是否在记录。"))
            }
            .padding(.top, Space.xs)
            Text(L("接下来用一分钟选好记录范围、授权和 AI 连接，之后都能在设置里改。"))
                .typography(.secondary).foregroundStyle(.secondary).padding(.top, Space.xs)
        }
    }

    private var finish: some View {
        VStack(alignment: .leading, spacing: Space.m) {
            Image(systemName: "checkmark.circle.fill").typography(.display).foregroundStyle(Palette.success)
            Text(L("准备好了")).typography(.title)
            Text(L("已按「%@」设置记录范围。Candid 目前处于暂停状态，点「开始记录」后菜单栏圆点会变红。", model.preset.title))
                .typography(.body).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            if session.isDemo {
                InlineMessage(kind: .note, text: L("这是合成样例，引导不会改动任何设置，也不能开始记录。"))
            }
            ForEach(model.skipped, id: \.self) { InlineMessage(kind: .warning, text: $0) }
            if !model.skipped.isEmpty {
                QuietButton(title: L("打开记录设置"), emphasized: true) { OnboardingLinks.openSettings("capture"); close() }.offset(x: -9)
            }
            Text(L("之后可以在「设置 → 关于」或菜单栏面板里重新打开这份引导；权限丢失、核心断开等问题会在菜单栏圆点上提示。"))
                .typography(.secondary).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
    }

    private func bullet(_ symbol: String, _ text: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: Space.xs) {
            Image(systemName: symbol).foregroundStyle(Palette.accent).frame(width: 18)
            Text(text).typography(.body)
        }
    }
}

/// Page dots for the guide.
private struct StepDots: View {
    let count: Int
    let current: Int
    var body: some View {
        HStack(spacing: 6) {
            ForEach(0..<count, id: \.self) { index in
                Capsule().fill(index == current ? Palette.accent : Surface.hover.color)
                    .frame(width: index == current ? 18 : 6, height: 6)
            }
        }
        .accessibilityLabel(L("第 %@ 步，共 %@ 步", current + 1, count))
    }
}

/// Opens a settings section of the main window, or the guide itself.
enum OnboardingLinks {
    @MainActor static func showGuide() {
        NSApp.windows.first { $0.identifier?.rawValue.hasPrefix("desktop") == true }?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        NotificationCenter.default.post(name: .candidShowOnboarding, object: nil)
    }
    @MainActor static func openSettings(_ tab: String) {
        UserDefaults.standard.set(tab, forKey: "desktop.settings.tab")
        NotificationCenter.default.post(name: .candidQuickAction, object: "settings")
    }
}

// MARK: - Step 2: what to record

private struct OnboardingCaptureStep: View {
    @Bindable var model: OnboardingModel
    let session: DesktopSession

    /// Apps open right now, as a quick first choice; the full list stays under 设置 → 应用.
    private var apps: [DesktopApplication] {
        session.applications.filter { $0.pid > 0 && $0.bundleID != Bundle.main.bundleIdentifier }.prefix(18).map { $0 }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Space.m) {
            SectionHeader(title: L("记录什么"), note: L("选一档作为起点，之后可在「设置 → 记录」里逐项调整。"))
            VStack(spacing: Space.xs) {
                ForEach(CapturePreset.allCases) { preset in presetRow(preset) }
            }
            if model.preset.needsApps {
                VStack(alignment: .leading, spacing: Space.xs) {
                    Text(L("允许读取文字和截图的应用")).typography(.headline)
                    Text(L("只记录这里选中的应用。下面是正在运行的应用，更多应用在「设置 → 应用」里添加。"))
                        .typography(.secondary).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    if apps.isEmpty {
                        Text(session.isDemo ? L("合成样例不读取本机应用。") : L("没有找到正在运行的应用。")).typography(.secondary).foregroundStyle(.secondary)
                    } else {
                        LazyVGrid(columns: [GridItem(.adaptive(minimum: 120), spacing: Space.xs, alignment: .leading)],
                                  alignment: .leading, spacing: Space.xs) {
                            ForEach(apps) { app in
                                ToggleChip(title: app.name, isOn: Binding(
                                    get: { session.allowedBundleIDs.contains(app.bundleID) || session.textAllowedBundleIDs.contains(app.bundleID) },
                                    set: { session.setContentAllowedForApps([app.bundleID], allowed: $0) }))
                            }
                        }
                    }
                }
                .padding(.top, Space.xs)
                .transition(.opacity)
            }
        }
        .animation(Motion.snappy, value: model.preset)
    }

    private func presetRow(_ preset: CapturePreset) -> some View {
        let selected = model.preset == preset
        return Button { model.preset = preset } label: {
            HStack(alignment: .top, spacing: Space.s) {
                Image(systemName: preset.symbol).typography(.headline).foregroundStyle(selected ? Palette.accent : .secondary).frame(width: 24)
                VStack(alignment: .leading, spacing: 2) {
                    Text(preset.title).typography(.headline)
                    Text(preset.detail).typography(.secondary).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                }
                Spacer()
                Image(systemName: selected ? "checkmark.circle.fill" : "circle").foregroundStyle(selected ? Palette.accent : .secondary)
            }
            .padding(Space.s)
            .frame(maxWidth: .infinity, alignment: .leading)
            .panelSurface(radius: Radius.medium, level: selected ? .hover : .wash)
            .selectionEdge(selected)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Step 3: permissions

private struct OnboardingPermissionStep: View {
    @Bindable var model: OnboardingModel
    let session: DesktopSession

    var body: some View {
        VStack(alignment: .leading, spacing: Space.m) {
            SectionHeader(title: L("授权"), note: L("「%@」只需要下面这些权限。授权后状态会自动刷新；屏幕录制有时要重开 Candid 才生效。", model.preset.title))
            VStack(spacing: 0) {
                ForEach(model.preset.permissions) { permission in
                    row(permission)
                    if permission != model.preset.permissions.last { HairlineDivider() }
                }
            }
            if session.isDemo {
                InlineMessage(kind: .note, text: L("合成样例只显示状态，不会请求授权。"))
            }
            Text(L("没授权的项目会先保持关闭，不影响其他记录。"))
                .typography(.secondary).foregroundStyle(.secondary)
        }
        .onAppear { model.refreshPermissions() }
        // Permissions are granted in System Settings: read them again when the user comes back to Candid, and when
        // macOS announces an accessibility change. No timer.
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in model.refreshPermissions() }
        .onReceive(DistributedNotificationCenter.default().publisher(for: Notification.Name("com.apple.accessibility.api"))) { _ in
            model.refreshPermissions()
        }
    }

    private func row(_ permission: SystemPermission) -> some View {
        let state = model.state(permission)
        return SettingRow(title: permission.title, detail: permission.detail) {
            StatusDot(state: state == .granted ? .on : state == .denied ? .attention : .off,
                      text: state == .granted ? L("已允许") : state == .denied ? L("已拒绝") : L("未允许"))
            if state != .granted {
                QuietButton(title: state == .notDetermined ? L("授权") : L("打开系统设置"), emphasized: true) {
                    Task { await permission.request(session: session); model.refreshPermissions() }
                }
                .disabled(session.isDemo)
            }
        }
    }
}

// MARK: - Step 4: AI

private struct OnboardingAIStep: View {
    @Bindable var model: OnboardingModel
    let session: DesktopSession

    /// Read-only summary of 设置 → 模型: sources and the three roles.
    private var configured: (sources: Int, roles: Int) {
        let config = AIRuntime.shared.config()
        return (config.providers.count, ["vision", "summary", "agent_chat"].filter { config.assignment($0) != nil }.count)
    }
    private var isolated: Bool { TimelineService.isolated(isDemo: session.isDemo) }

    var body: some View {
        VStack(alignment: .leading, spacing: Space.l) {
            SectionHeader(title: L("连接 AI（可选）"), note: L("Candid 不用模型也能记录。接上本机模型后可以做图像理解和整理；接上 Claude Code 或 Codex 后，它们能读你的时间线。"))
            SettingsGroup(title: L("本机模型")) {
                ForEach(model.servers) { server in
                    SettingRow(title: server.name, detail: server.url) {
                        if server.found == nil || model.detecting {
                            ProgressView().controlSize(.small)
                        } else if server.found == true {
                            StatusDot(state: .on, text: server.models.map { L("可用，%@ 个模型", $0) } ?? L("可用"))
                        } else {
                            StatusDot(state: .off, text: L("未运行"))
                        }
                    }
                    HairlineDivider()
                }
                SettingRow(title: L("当前配置"), detail: L("%@ 个模型源，%@/3 个角色已分配", configured.sources, configured.roles)) {
                    QuietButton(title: L("重新检测")) { Task { await model.detectServers() } }
                    QuietButton(title: L("去设置模型"), emphasized: true) { OnboardingLinks.openSettings("models") }
                }
            }
            SettingsGroup(title: L("让 AI 工具读取时间线")) {
                ForEach(OnboardingModel.AgentTarget.allCases) { target in
                    agentRow(target)
                    if target != OnboardingModel.AgentTarget.allCases.last { HairlineDivider() }
                }
            }
            Text(isolated ? L("当前是隔离的数据目录，只能复制命令，不会写入你的 Claude Code 或 Codex 配置。")
                          : L("每个工具会拿到自己的只读令牌，可在「设置 → 外部访问」里随时撤销。跳过也没关系，之后在那里接入。"))
                .typography(.secondary).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
    }

    private func agentRow(_ target: OnboardingModel.AgentTarget) -> some View {
        let detail: String? = switch model.agentResult[target] {
        case .copied: target == .claudeCode ? L("命令已复制，在终端运行一次即可。") : L("配置已复制，加到 ~/.codex/config.toml。")
        case .written(let text): text
        case .failed(let text): text
        case nil: target == .claudeCode ? L("通过 claude mcp add 加入") : L("写入 ~/.codex/config.toml")
        }
        return SettingRow(title: target.name, detail: detail) {
            if case .written = model.agentResult[target] { StatusDot(state: .on, text: L("已接入")) }
            QuietButton(title: target == .claudeCode ? L("复制命令") : L("复制配置")) { Task { await model.copy(target) } }
            QuietButton(title: L("一键写入"), emphasized: true) { Task { await model.write(target) } }
                .disabled(isolated)
        }
    }
}
