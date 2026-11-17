import SwiftUI
import CandidLocalization
import AppKit
import DesktopCore
import UniformTypeIdentifiers

/// Routes one settings section to its page. The home settings page owns the section list.
struct DesktopSettingsView: View {
    @Bindable var session: DesktopSession
    var deviceSettings: AnyView?
    let section: String

    var body: some View {
        switch section {
        case "appearance": AppearanceSettings()
        case "scope": AppScopeSettings(session: session)
        case "storage": StorageSettings(session: session)
        case "recognition": DesktopVisionSettings(session: session.recognition, isDemo: session.isDemo)
        case "diagnostics": DesktopSystemView(session: session)
        case "access": TimelineAccessView()
        case "console":
            if let deviceSettings { deviceSettings } else { CaptureSettings(session: session) }
        default: CaptureSettings(session: session)
        }
    }
}

/// A setting that is on or off.
struct SwitchRow: View {
    let title: String
    var detail: String?
    @Binding var isOn: Bool

    var body: some View {
        SettingRow(title: title, detail: detail) {
            Toggle(title, isOn: $isOn).toggleStyle(.switch).labelsHidden().controlSize(.small)
        }
    }
}

// MARK: - 记录

private struct CaptureSettings: View {
    @Bindable var session: DesktopSession
    @AppStorage("desktop.settings.tab") private var selectedTab = "capture"

    var body: some View {
        SettingsPage(title: L("记录")) {
            SettingsGroup(title: L("电脑采集")) {
                SettingRow(title: L("状态")) {
                    StatusDot(state: session.isRecording ? .on : .off, text: session.statusText)
                    if session.isRecording {
                        QuietButton(title: L("暂停")) { session.pauseRecording() }
                    } else {
                        PrimaryButton(title: L("开始记录")) { session.startRecording() }
                    }
                }
                .disabled(session.isDemo)
                HairlineDivider()
                SettingRow(title: L("采集密度"), detail: L("文字停稳后保存；连续变化时按最长等待间隔保留中途内容。")) {
                    CapsuleChoice(options: [(DesktopSamplingMode.balanced, L("平衡")), (.detailed, L("较完整"))],
                                  selection: Binding(get: { session.samplingMode }, set: session.setSamplingMode))
                }
                .disabled(session.isDemo)
            }
            SettingsGroup(title: L("截图")) { imageRows }.disabled(session.isDemo)
            SettingsGroup(title: L("采集内容")) {
                contentRows
                HairlineDivider()
                SettingRow(title: L("持续变化最长等待"), detail: L("连续接收或生成内容时保留中途版本；秒数为采样设置。")) {
                    MenuChoice(options: [5.0, 10, 20, 30, 60].map { ($0, L("%@ 秒", Int($0))) },
                               selection: Binding(get: { session.contentOptions.maximumChangeWait ?? 10 }, set: { session.updateContentOption(\.maximumChangeWait, value: Optional($0)) }))
                }
            }.disabled(session.isDemo)
            SettingsGroup(title: L("其他触发")) { triggerRows }.disabled(session.isDemo)
            FoldRow(title: L("频率与省电")) { frequencyRows }.disabled(session.isDemo)
            SettingsGroup(title: L("权限")) {
                permissionRow(L("屏幕录制"), detail: L("获取允许应用的窗口画面。"), granted: session.screenPermission, action: session.requestScreenPermission)
                HairlineDivider()
                permissionRow(L("辅助功能"), detail: L("确认当前窗口，并读取你允许的文字和焦点信息。"), granted: session.accessibilityPermission,
                              action: session.requestAccessibilityPermission)
            } accessory: {
                QuietButton(title: L("重新检查"), action: session.refresh)
            }
        }
    }

    @ViewBuilder private var imageRows: some View {
        SwitchRow(title: L("窗口截图"), detail: L("只截「应用」里允许的软件；关闭后自动和手动截图都停止。"),
                  isOn: Binding(get: { session.screenshotsEnabled }, set: session.setScreenshotsEnabled))
        HairlineDivider()
        SwitchRow(title: L("自动截图"), detail: L("开始记录后按触发条件自动截图，重开后保留。"),
                  isOn: Binding(get: { session.automaticCaptureAvailable }, set: session.setAutomaticCaptureEnabled))
            .disabled(!session.screenshotsEnabled)
        if session.automaticCaptureAvailable {
            InlineMessage(kind: .warning, text: L("锁屏保护待验证。")).padding(.bottom, Space.xs)
        }
        HairlineDivider()
        SwitchRow(title: L("切换窗口时拍一张"), detail: L("切到允许的窗口后约 0.75 秒再截；同窗频率和省电限制可能延后。"),
                  isOn: optionBinding(\.windowTriggersEnabled))
            .disabled(!session.screenshotsEnabled || !session.automaticCaptureAvailable)
        HairlineDivider()
        FoldRow(title: L("画质与定时补图")) {
            SettingRow(title: L("定时补图间隔"), detail: L("平衡模式下到期后在下一次定时检查补图。")) {
                secondsMenu(optionBinding(\.imageFallbackInterval), choices: [5, 10, 15, 30, 60, 120, 300, 600, 1800, 3600])
            }
            .disabled(session.samplingMode != .balanced || !session.periodicEnabled || !session.screenshotsEnabled)
            HairlineDivider()
            SettingRow(title: L("截图尺寸"), detail: L("图片最长边，不放大原窗口；越大越清晰，也越占空间。")) {
                MenuChoice(options: [1280, 1920, 2560, 3840].map { ($0, "\($0) px") }, selection: imageBinding(\.maximumEdge))
            }
            HairlineDivider()
            SettingRow(title: L("JPEG 质量"), detail: L("越高文字越清楚，文件也越大。")) {
                MenuChoice(options: [0.5, 0.7, 0.8, 0.9, 0.95].map { ($0, "\(Int($0 * 100))%") }, selection: imageBinding(\.jpegQuality))
            }
            HairlineDivider()
            SwitchRow(title: L("图片过大时降低质量"), detail: L("单图超过 2 MiB 时降低压缩质量；关闭后超限图片直接跳过。"),
                      isOn: imageBinding(\.reduceOversizedImages))
        }
        QuietButton(title: L("管理可截图的应用"), emphasized: true) { selectedTab = "scope" }.offset(x: -9)
    }

    @ViewBuilder private var contentRows: some View {
        SwitchRow(title: L("窗口标题"), detail: L("保存窗口或页面名称，便于搜索；关闭只影响文字字段。"), isOn: contentBinding(\.windowTitleEnabled))
        HairlineDivider()
        SwitchRow(title: L("读取文字"), detail: L("只读取「应用」里允许的软件。"), isOn: Binding(get: { session.textEnabled }, set: session.setTextEnabled))
        HairlineDivider()
        SwitchRow(title: L("正文"), detail: L("保存系统能提供的可读正文；部分软件只提供少量文字。"), isOn: contentBinding(\.readableTextEnabled))
            .disabled(!session.textEnabled)
        HairlineDivider()
        SwitchRow(title: L("选中文字"), detail: L("保存采样时选中的文字，不会因选区变化立即截图。"), isOn: contentBinding(\.selectedTextEnabled))
            .disabled(!session.textEnabled)
        HairlineDivider()
        SwitchRow(title: L("焦点信息"), detail: L("保存当前控件的类型和名称；不保存按键。"), isOn: contentBinding(\.focusEnabled))
        HairlineDivider()
        SwitchRow(title: L("系统状态"), detail: L("附上电源、温度和空闲时长；锁屏和温度检查始终在内部进行。"), isOn: contentBinding(\.systemStateEnabled))
        if session.textAllowedBundleIDs.isEmpty {
            QuietButton(title: L("选择允许读取文字的应用"), emphasized: true) { selectedTab = "scope" }.offset(x: -9)
        }
    }

    @ViewBuilder private var triggerRows: some View {
        SwitchRow(title: L("定时记录"), detail: L("停留在同一窗口时也检查，补上阅读和收到回复等情况。"),
                  isOn: Binding(get: { session.periodicEnabled }, set: session.setPeriodicEnabled))
        if session.periodicEnabled {
            HairlineDivider()
            SettingRow(title: L("定时检查间隔"), detail: L("文字没变化时不重复保存。")) {
                secondsMenu(Binding(get: { session.periodicInterval }, set: session.setPeriodicInterval),
                            choices: [5, 10, 15, 30, 60, 120, 300, 600, 1800, 3600])
            }
        }
        HairlineDivider()
        interactionRow(L("回车触发"), detail: L("回车或点击后检查内容；只有来源消息证据才确认发出。"), trigger: "return", selected: session.samplingOptions.returnEnabled)
        HairlineDivider()
        interactionRow(L("滚动停止后触发"), detail: L("停止滚动约 1 秒后采样，连续滚动会合并。"), trigger: "scroll", selected: session.samplingOptions.scrollEnabled)
        if !session.inputMonitoringAuthorized {
            HairlineDivider()
            SettingRow(title: L("输入监控"), detail: L("回车与滚动触发需要这个权限。")) {
                QuietButton(title: L("授权"), emphasized: true, action: session.requestInputMonitoringPermission)
            }
        } else if session.interactionTriggersEnabled && session.isRecording && !session.interactionTriggersActive {
            InlineMessage(kind: .warning, text: L("回车与滚动监听未运行。")).padding(.vertical, Space.xs)
        }
    }

    @ViewBuilder private var frequencyRows: some View {
        let intervals: [Double] = [1, 2, 3, 5, 10, 15, 20, 30, 60, 120, 300]
        SettingRow(title: L("内容检查最短间隔"),
                   detail: session.samplingMode == .balanced ? L("平衡模式先检查内容再安排截图。") : L("仅平衡模式生效。")) {
            secondsMenu(optionBinding(\.minimumContentInterval), choices: intervals)
        }
        .disabled(session.samplingMode != .balanced)
        HairlineDivider()
        SettingRow(title: L("内容检查每分钟上限"), detail: L("每个窗口分别计数。")) { capacityStepper(optionBinding(\.contentCapacity)) }
            .disabled(session.samplingMode != .balanced)
        HairlineDivider()
        SettingRow(title: L("同一窗口最短截图间隔"), detail: L("相同画面只保留一张。")) {
            secondsMenu(Binding(get: { session.minimumCaptureInterval }, set: session.setMinimumCaptureInterval), choices: intervals)
        }
        HairlineDivider()
        SettingRow(title: L("同一窗口每分钟上限"), detail: L("失败也计数；手动截图每分钟最多 12 次。")) { capacityStepper(optionBinding(\.imageCapacity)) }
        HairlineDivider()
        SwitchRow(title: L("电池供电时降低频率"), detail: L("定时至少 60 秒，自动文字和截图至少 30 秒、每分钟最多 2 次。"),
                  isOn: optionBinding(\.conserveBattery))
        HStack(spacing: Space.xxs) {
            QuietButton(title: L("快速试采"), action: session.applyFastSamplingFrequency)
                .help(L("内容检查和截图 3 秒、每分钟 20 次；定时和补图 10 秒"))
            QuietButton(title: L("恢复采样默认值"), action: session.restoreSamplingDefaults)
        }
        .padding(.top, Space.xs).offset(x: -9)
    }

    private func permissionRow(_ title: String, detail: String, granted: Bool, action: @escaping () -> Void) -> some View {
        SettingRow(title: title, detail: detail) {
            if granted {
                StatusDot(state: .on, text: L("已授权"))
            } else {
                QuietButton(title: L("授权"), emphasized: true, action: action).disabled(session.isDemo)
                    .accessibilityLabel(L("申请%@权限", title))
            }
        }
    }

    private func interactionRow(_ title: String, detail: String, trigger: String, selected: Bool) -> some View {
        SwitchRow(title: title, detail: detail, isOn: Binding(get: { session.interactionTriggersEnabled && selected },
                                                              set: { session.setInteractionTrigger(trigger, enabled: $0) }))
            .disabled(!session.inputMonitoringAuthorized && !(session.interactionTriggersEnabled && selected))
    }

    private func secondsMenu(_ value: Binding<Double>, choices: [Double]) -> some View {
        MenuChoice(options: Array(Set(choices + [value.wrappedValue])).sorted().map { ($0, Self.secondsText($0)) }, selection: value)
    }

    static func secondsText(_ seconds: Double) -> String {
        seconds < 60 || seconds.truncatingRemainder(dividingBy: 60) != 0 ? L("%@ 秒", Int(seconds)) : L("%@ 分钟", Int(seconds / 60))
    }

    private func capacityStepper(_ value: Binding<Int>) -> some View {
        StepperControl(text: L("%@ 次", value.wrappedValue), value: value, range: DesktopSamplingOptions.samplingCapacityRange)
    }

    private func contentBinding<Value>(_ keyPath: WritableKeyPath<DesktopContentOptions, Value>) -> Binding<Value> {
        Binding(get: { session.contentOptions[keyPath: keyPath] }, set: { session.updateContentOption(keyPath, value: $0) })
    }

    private func imageBinding<Value>(_ keyPath: WritableKeyPath<DesktopImageOptions, Value>) -> Binding<Value> {
        Binding(get: { session.imageOptions[keyPath: keyPath] }, set: { session.updateImageOption(keyPath, value: $0) })
    }

    private func optionBinding<Value>(_ keyPath: WritableKeyPath<DesktopSamplingOptions, Value>) -> Binding<Value> {
        Binding(get: { session.samplingOptions[keyPath: keyPath] }, set: { session.updateSamplingOption(keyPath, value: $0) })
    }
}

// MARK: - 应用

private struct AppScopeSettings: View {
    @Bindable var session: DesktopSession
    @ViewState private var search = ""

    private var apps: [DesktopApplication] {
        session.applications.filter { search.isEmpty || $0.name.localizedCaseInsensitiveContains(search) || $0.bundleID.localizedCaseInsensitiveContains(search) }
    }

    var body: some View {
        SettingsPage(title: L("应用")) {
            VStack(alignment: .leading, spacing: Space.s) {
                HStack(spacing: Space.xs) {
                    SearchField(prompt: L("搜索应用"), text: $search).frame(maxWidth: 280)
                    if session.scanningApplications { ProgressView().controlSize(.small).accessibilityLabel(L("扫描已安装应用")) }
                    Spacer()
                    MenuButton(title: L("批量设置")) {
                        Button(L("允许列表中的截图和文字")) { session.setContentAllowedForApps(Set(apps.map(\.bundleID)), allowed: true) }
                        Button(L("停止列表中的截图和文字")) { session.setContentAllowedForApps(Set(apps.map(\.bundleID)), allowed: false) }
                    }
                    .disabled(session.isDemo || apps.isEmpty)
                    QuietButton(title: L("添加应用…"), action: addApplication)
                    QuietButton(title: L("刷新"), action: session.refresh)
                }
                Text(L("「兼容」用于缺少控件焦点的应用（如 Notion）：仍只截该应用的窗口，但系统未报告的敏感内容可能入图。"))
                    .typography(.secondary).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            if apps.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    Text(L("没有匹配的应用")).typography(.headline)
                    QuietButton(title: L("刷新"), action: session.refresh).offset(x: -9)
                }
            } else {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(apps) { app in
                        AppScopeRow(session: session, app: app,
                                    showsBundleID: session.applications.filter { $0.name == app.name }.count > 1)
                        HairlineDivider()
                    }
                }
            }
        }
    }

    private func addApplication() {
        let panel = NSOpenPanel()
        panel.title = L("添加允许配置的应用"); panel.allowedContentTypes = [.application]
        panel.directoryURL = URL(fileURLWithPath: "/Applications", isDirectory: true)
        panel.allowsMultipleSelection = false; panel.canChooseDirectories = false
        panel.begin { response in
            guard response == .OK, let url = panel.url else { return }
            Task { @MainActor in session.addApplication(url) }
        }
    }
}

private struct AppScopeRow: View {
    @Bindable var session: DesktopSession
    let app: DesktopApplication
    let showsBundleID: Bool

    private var excluded: Bool { session.excludedBundleIDs.contains(app.bundleID) }
    private var screenshots: Bool { session.allowedBundleIDs.contains(app.bundleID) }

    var body: some View {
        HStack(spacing: Space.s) {
            DesktopApplicationIcon(bundleID: app.bundleID, fallback: "app", size: 22)
            VStack(alignment: .leading, spacing: 1) {
                Text(app.name).typography(.body, .medium).lineLimit(1).help(app.bundleID)
                if app.pid == 0 || showsBundleID {
                    Text([app.pid == 0 ? (app.bundlePath == nil ? L("未找到安装位置") : L("未运行")) : nil, showsBundleID ? app.bundleID : nil]
                        .compactMap { $0 }.joined(separator: " · "))
                        .typography(.caption, .regular).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            Spacer(minLength: Space.s)
            HStack(spacing: Space.xxs) {
                ToggleChip(title: L("截图"), isOn: Binding(get: { screenshots }, set: { session.setAllowedApp(app.bundleID, $0) }))
                    .disabled(excluded).accessibilityLabel(L("允许 %@ 截图", app.name))
                if screenshots && !excluded {
                    ToggleChip(title: L("兼容"), isOn: Binding(get: { session.windowCompatibilityBundleIDs.contains(app.bundleID) },
                                                           set: { session.setWindowCompatibility(app.bundleID, $0) }))
                        .accessibilityLabel(L("启用 %@ 的截图兼容", app.name))
                        .help(L("无法确定窗口内控件焦点时仍可截这个窗口。系统未报告的敏感内容也可能入图。"))
                }
                ToggleChip(title: L("文字"), isOn: Binding(get: { session.textAllowedBundleIDs.contains(app.bundleID) },
                                                       set: { session.setTextAllowedApp(app.bundleID, $0) }))
                    .disabled(excluded).accessibilityLabel(L("允许读取 %@ 的文字", app.name))
                ToggleChip(title: L("排除"), isOn: Binding(get: { excluded }, set: { session.setExcludedApp(app.bundleID, $0) }), tint: Palette.danger)
                    .accessibilityLabel(L("排除 %@ 的记录", app.name))
            }
            .opacity(session.isDemo ? 0.5 : 1)
            .disabled(session.isDemo)
        }
        .padding(.vertical, Space.xs)
    }
}

// MARK: - 数据

private struct StorageSettings: View {
    @Bindable var session: DesktopSession
    @ViewState private var proposal: Int?
    @ViewState private var affected = 0

    var body: some View {
        SettingsPage(title: L("数据")) {
            TimelineLibraryGroup()
            SettingsGroup(title: L("保存位置")) {
                SettingRow(title: L("采集设置与待写队列"), detail: session.dataLocation) {
                    QuietButton(title: L("在 Finder 中显示")) {
                        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: session.dataLocation)])
                    }
                }
            }
            SettingsGroup(title: L("清理")) {
                SettingRow(title: L("保存天数"), detail: L("下次启动时清理超期记录和不再被引用的截图。")) {
                    MenuChoice(options: Array(Set([1, 3, 7, 14, 30, 90, 365, session.retentionDays])).sorted().map { ($0, L("%@ 天", $0)) },
                               selection: Binding(get: { proposal ?? session.retentionDays }, set: propose))
                }
                if let proposal {
                    InlinePanel {
                        VStack(alignment: .leading, spacing: Space.s) {
                            InlineMessage(kind: .warning, text: L("改为 %@ 天后，按当前记录有 %@ 条已超过期限，下次启动将被清理，应用内无法恢复；已导出的副本不受影响。", proposal, affected))
                            HStack(spacing: Space.xxs) {
                                Spacer()
                                QuietButton(title: L("取消")) { withAnimation(Motion.spring) { self.proposal = nil } }
                                QuietButton(title: L("确认修改"), role: .destructive) {
                                    session.setRetentionDays(proposal)
                                    withAnimation(Motion.spring) { self.proposal = nil }
                                }
                            }
                        }
                    }
                    .padding(.bottom, Space.s)
                }
                HairlineDivider()
                SettingRow(title: L("截图空间上限"), detail: L("达到上限后停止新增截图，不提前删除未过期内容。")) {
                    MenuChoice(options: [1, 2, 5, 10, 20, 50, 100].map { (Int64($0) * 1024 * 1024 * 1024, "\($0) GiB") },
                               selection: Binding(get: { session.maximumBytes }, set: session.setMaximumBytes))
                }
                HairlineDivider()
                SettingRow(title: L("已用空间"), detail: L("数据卷可用空间不足 2 GiB 时也会停止新截图。")) {
                    Text(ByteCountFormatter.string(fromByteCount: session.statistics.storageBytes, countStyle: .file))
                        .typography(.body, .medium, digits: true)
                }
            }
            .disabled(session.isDemo)
        }
        .animation(Motion.spring, value: proposal)
    }

    private func propose(_ days: Int) {
        guard days < session.retentionDays else { session.setRetentionDays(days); proposal = nil; return }
        Task {
            guard let count = await session.retentionImpact(days: days) else { return }
            affected = count; proposal = days
        }
    }
}
