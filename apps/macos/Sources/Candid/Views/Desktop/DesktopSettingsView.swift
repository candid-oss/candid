import SwiftUI
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
        SettingsPage(title: "记录") {
            SettingsGroup(title: "电脑采集") {
                SettingRow(title: "状态") {
                    StatusDot(state: session.isRecording ? .on : .off, text: session.statusText)
                    if session.isRecording {
                        QuietButton(title: "暂停") { session.pauseRecording() }
                    } else {
                        PrimaryButton(title: "开始记录") { session.startRecording() }
                    }
                }
                .disabled(session.isDemo)
                HairlineDivider()
                SettingRow(title: "采集密度", detail: "文字停稳后保存；连续变化时按最长等待间隔保留中途内容。") {
                    CapsuleChoice(options: [(DesktopSamplingMode.balanced, "平衡"), (.detailed, "较完整")],
                                  selection: Binding(get: { session.samplingMode }, set: session.setSamplingMode))
                }
                .disabled(session.isDemo)
            }
            SettingsGroup(title: "截图") { imageRows }.disabled(session.isDemo)
            SettingsGroup(title: "采集内容") {
                contentRows
                HairlineDivider()
                SettingRow(title: "持续变化最长等待", detail: "连续接收或生成内容时保留中途版本；秒数为采样设置。") {
                    MenuChoice(options: [5.0, 10, 20, 30, 60].map { ($0, "\(Int($0)) 秒") },
                               selection: Binding(get: { session.contentOptions.maximumChangeWait ?? 10 }, set: { session.updateContentOption(\.maximumChangeWait, value: Optional($0)) }))
                }
            }.disabled(session.isDemo)
            SettingsGroup(title: "其他触发") { triggerRows }.disabled(session.isDemo)
            FoldRow(title: "频率与省电") { frequencyRows }.disabled(session.isDemo)
            SettingsGroup(title: "权限") {
                permissionRow("屏幕录制", detail: "获取允许应用的窗口画面。", granted: session.screenPermission, action: session.requestScreenPermission)
                HairlineDivider()
                permissionRow("辅助功能", detail: "确认当前窗口，并读取你允许的文字和焦点信息。", granted: session.accessibilityPermission,
                              action: session.requestAccessibilityPermission)
            } accessory: {
                QuietButton(title: "重新检查", action: session.refresh)
            }
        }
    }

    @ViewBuilder private var imageRows: some View {
        SwitchRow(title: "窗口截图", detail: "只截「应用」里允许的软件；关闭后自动和手动截图都停止。",
                  isOn: Binding(get: { session.screenshotsEnabled }, set: session.setScreenshotsEnabled))
        HairlineDivider()
        SwitchRow(title: "自动截图", detail: "开始记录后按触发条件自动截图，重开后保留。",
                  isOn: Binding(get: { session.automaticCaptureAvailable }, set: session.setAutomaticCaptureEnabled))
            .disabled(!session.screenshotsEnabled)
        if session.automaticCaptureAvailable {
            InlineMessage(kind: .warning, text: "锁屏保护待验证。").padding(.bottom, Space.xs)
        }
        HairlineDivider()
        SwitchRow(title: "切换窗口时拍一张", detail: "切到允许的窗口后约 0.75 秒再截；同窗频率和省电限制可能延后。",
                  isOn: optionBinding(\.windowTriggersEnabled))
            .disabled(!session.screenshotsEnabled || !session.automaticCaptureAvailable)
        HairlineDivider()
        FoldRow(title: "画质与定时补图") {
            SettingRow(title: "定时补图间隔", detail: "平衡模式下到期后在下一次定时检查补图。") {
                secondsMenu(optionBinding(\.imageFallbackInterval), choices: [5, 10, 15, 30, 60, 120, 300, 600, 1800, 3600])
            }
            .disabled(session.samplingMode != .balanced || !session.periodicEnabled || !session.screenshotsEnabled)
            HairlineDivider()
            SettingRow(title: "截图尺寸", detail: "图片最长边，不放大原窗口；越大越清晰，也越占空间。") {
                MenuChoice(options: [1280, 1920, 2560, 3840].map { ($0, "\($0) px") }, selection: imageBinding(\.maximumEdge))
            }
            HairlineDivider()
            SettingRow(title: "JPEG 质量", detail: "越高文字越清楚，文件也越大。") {
                MenuChoice(options: [0.5, 0.7, 0.8, 0.9, 0.95].map { ($0, "\(Int($0 * 100))%") }, selection: imageBinding(\.jpegQuality))
            }
            HairlineDivider()
            SwitchRow(title: "图片过大时降低质量", detail: "单图超过 2 MiB 时降低压缩质量；关闭后超限图片直接跳过。",
                      isOn: imageBinding(\.reduceOversizedImages))
        }
        QuietButton(title: "管理可截图的应用", emphasized: true) { selectedTab = "scope" }.offset(x: -9)
    }

    @ViewBuilder private var contentRows: some View {
        SwitchRow(title: "窗口标题", detail: "保存窗口或页面名称，便于搜索；关闭只影响文字字段。", isOn: contentBinding(\.windowTitleEnabled))
        HairlineDivider()
        SwitchRow(title: "读取文字", detail: "只读取「应用」里允许的软件。", isOn: Binding(get: { session.textEnabled }, set: session.setTextEnabled))
        HairlineDivider()
        SwitchRow(title: "正文", detail: "保存系统能提供的可读正文；部分软件只提供少量文字。", isOn: contentBinding(\.readableTextEnabled))
            .disabled(!session.textEnabled)
        HairlineDivider()
        SwitchRow(title: "选中文字", detail: "保存采样时选中的文字，不会因选区变化立即截图。", isOn: contentBinding(\.selectedTextEnabled))
            .disabled(!session.textEnabled)
        HairlineDivider()
        SwitchRow(title: "焦点信息", detail: "保存当前控件的类型和名称；不保存按键。", isOn: contentBinding(\.focusEnabled))
        HairlineDivider()
        SwitchRow(title: "系统状态", detail: "附上电源、温度和空闲时长；锁屏和温度检查始终在内部进行。", isOn: contentBinding(\.systemStateEnabled))
        if session.textAllowedBundleIDs.isEmpty {
            QuietButton(title: "选择允许读取文字的应用", emphasized: true) { selectedTab = "scope" }.offset(x: -9)
        }
    }

    @ViewBuilder private var triggerRows: some View {
        SwitchRow(title: "定时记录", detail: "停留在同一窗口时也检查，补上阅读和收到回复等情况。",
                  isOn: Binding(get: { session.periodicEnabled }, set: session.setPeriodicEnabled))
        if session.periodicEnabled {
            HairlineDivider()
            SettingRow(title: "定时检查间隔", detail: "文字没变化时不重复保存。") {
                secondsMenu(Binding(get: { session.periodicInterval }, set: session.setPeriodicInterval),
                            choices: [5, 10, 15, 30, 60, 120, 300, 600, 1800, 3600])
            }
        }
        HairlineDivider()
        interactionRow("回车触发", detail: "回车或点击后检查内容；只有来源消息证据才确认发出。", trigger: "return", selected: session.samplingOptions.returnEnabled)
        HairlineDivider()
        interactionRow("滚动停止后触发", detail: "停止滚动约 1 秒后采样，连续滚动会合并。", trigger: "scroll", selected: session.samplingOptions.scrollEnabled)
        if !session.inputMonitoringAuthorized {
            HairlineDivider()
            SettingRow(title: "输入监控", detail: "回车与滚动触发需要这个权限。") {
                QuietButton(title: "授权", emphasized: true, action: session.requestInputMonitoringPermission)
            }
        } else if session.interactionTriggersEnabled && session.isRecording && !session.interactionTriggersActive {
            InlineMessage(kind: .warning, text: "回车与滚动监听未运行。").padding(.vertical, Space.xs)
        }
    }

    @ViewBuilder private var frequencyRows: some View {
        let intervals: [Double] = [1, 2, 3, 5, 10, 15, 20, 30, 60, 120, 300]
        SettingRow(title: "内容检查最短间隔",
                   detail: session.samplingMode == .balanced ? "平衡模式先检查内容再安排截图。" : "仅平衡模式生效。") {
            secondsMenu(optionBinding(\.minimumContentInterval), choices: intervals)
        }
        .disabled(session.samplingMode != .balanced)
        HairlineDivider()
        SettingRow(title: "内容检查每分钟上限", detail: "每个窗口分别计数。") { capacityStepper(optionBinding(\.contentCapacity)) }
            .disabled(session.samplingMode != .balanced)
        HairlineDivider()
        SettingRow(title: "同一窗口最短截图间隔", detail: "相同画面只保留一张。") {
            secondsMenu(Binding(get: { session.minimumCaptureInterval }, set: session.setMinimumCaptureInterval), choices: intervals)
        }
        HairlineDivider()
        SettingRow(title: "同一窗口每分钟上限", detail: "失败也计数；手动截图每分钟最多 12 次。") { capacityStepper(optionBinding(\.imageCapacity)) }
        HairlineDivider()
        SwitchRow(title: "电池供电时降低频率", detail: "定时至少 60 秒，自动文字和截图至少 30 秒、每分钟最多 2 次。",
                  isOn: optionBinding(\.conserveBattery))
        HStack(spacing: Space.xxs) {
            QuietButton(title: "快速试采", action: session.applyFastSamplingFrequency)
                .help("内容检查和截图 3 秒、每分钟 20 次；定时和补图 10 秒")
            QuietButton(title: "恢复采样默认值", action: session.restoreSamplingDefaults)
        }
        .padding(.top, Space.xs).offset(x: -9)
    }

    private func permissionRow(_ title: String, detail: String, granted: Bool, action: @escaping () -> Void) -> some View {
        SettingRow(title: title, detail: detail) {
            if granted {
                StatusDot(state: .on, text: "已授权")
            } else {
                QuietButton(title: "授权", emphasized: true, action: action).disabled(session.isDemo)
                    .accessibilityLabel("申请\(title)权限")
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
        seconds < 60 || seconds.truncatingRemainder(dividingBy: 60) != 0 ? "\(Int(seconds)) 秒" : "\(Int(seconds / 60)) 分钟"
    }

    private func capacityStepper(_ value: Binding<Int>) -> some View {
        StepperControl(text: "\(value.wrappedValue) 次", value: value, range: DesktopSamplingOptions.samplingCapacityRange)
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
        SettingsPage(title: "应用") {
            VStack(alignment: .leading, spacing: Space.s) {
                HStack(spacing: Space.xs) {
                    SearchField(prompt: "搜索应用", text: $search).frame(maxWidth: 280)
                    if session.scanningApplications { ProgressView().controlSize(.small).accessibilityLabel("扫描已安装应用") }
                    Spacer()
                    MenuButton(title: "批量设置") {
                        Button("允许列表中的截图和文字") { session.setContentAllowedForApps(Set(apps.map(\.bundleID)), allowed: true) }
                        Button("停止列表中的截图和文字") { session.setContentAllowedForApps(Set(apps.map(\.bundleID)), allowed: false) }
                    }
                    .disabled(session.isDemo || apps.isEmpty)
                    QuietButton(title: "添加应用…", action: addApplication)
                    QuietButton(title: "刷新", action: session.refresh)
                }
                Text("「兼容」用于缺少控件焦点的应用（如 Notion）：仍只截该应用的窗口，但系统未报告的敏感内容可能入图。")
                    .typography(.secondary).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            if apps.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    Text("没有匹配的应用").typography(.headline)
                    QuietButton(title: "刷新", action: session.refresh).offset(x: -9)
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
        panel.title = "添加允许配置的应用"; panel.allowedContentTypes = [.application]
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
                    Text([app.pid == 0 ? (app.bundlePath == nil ? "未找到安装位置" : "未运行") : nil, showsBundleID ? app.bundleID : nil]
                        .compactMap { $0 }.joined(separator: " · "))
                        .typography(.caption, .regular).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            Spacer(minLength: Space.s)
            HStack(spacing: Space.xxs) {
                ToggleChip(title: "截图", isOn: Binding(get: { screenshots }, set: { session.setAllowedApp(app.bundleID, $0) }))
                    .disabled(excluded).accessibilityLabel("允许 \(app.name) 截图")
                if screenshots && !excluded {
                    ToggleChip(title: "兼容", isOn: Binding(get: { session.windowCompatibilityBundleIDs.contains(app.bundleID) },
                                                           set: { session.setWindowCompatibility(app.bundleID, $0) }))
                        .accessibilityLabel("启用 \(app.name) 的截图兼容")
                        .help("无法确定窗口内控件焦点时仍可截这个窗口。系统未报告的敏感内容也可能入图。")
                }
                ToggleChip(title: "文字", isOn: Binding(get: { session.textAllowedBundleIDs.contains(app.bundleID) },
                                                       set: { session.setTextAllowedApp(app.bundleID, $0) }))
                    .disabled(excluded).accessibilityLabel("允许读取 \(app.name) 的文字")
                ToggleChip(title: "排除", isOn: Binding(get: { excluded }, set: { session.setExcludedApp(app.bundleID, $0) }), tint: Palette.danger)
                    .accessibilityLabel("排除 \(app.name) 的记录")
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
        SettingsPage(title: "数据") {
            TimelineLibraryGroup()
            SettingsGroup(title: "保存位置") {
                SettingRow(title: "采集设置与待写队列", detail: session.dataLocation) {
                    QuietButton(title: "在 Finder 中显示") {
                        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: session.dataLocation)])
                    }
                }
            }
            SettingsGroup(title: "清理") {
                SettingRow(title: "保存天数", detail: "下次启动时清理超期记录和不再被引用的截图。") {
                    MenuChoice(options: Array(Set([1, 3, 7, 14, 30, 90, 365, session.retentionDays])).sorted().map { ($0, "\($0) 天") },
                               selection: Binding(get: { proposal ?? session.retentionDays }, set: propose))
                }
                if let proposal {
                    InlinePanel {
                        VStack(alignment: .leading, spacing: Space.s) {
                            InlineMessage(kind: .warning, text: "改为 \(proposal) 天后，按当前记录有 \(affected) 条已超过期限，下次启动将被清理，应用内无法恢复；已导出的副本不受影响。")
                            HStack(spacing: Space.xxs) {
                                Spacer()
                                QuietButton(title: "取消") { withAnimation(Motion.spring) { self.proposal = nil } }
                                QuietButton(title: "确认修改", role: .destructive) {
                                    session.setRetentionDays(proposal)
                                    withAnimation(Motion.spring) { self.proposal = nil }
                                }
                            }
                        }
                    }
                    .padding(.bottom, Space.s)
                }
                HairlineDivider()
                SettingRow(title: "截图空间上限", detail: "达到上限后停止新增截图，不提前删除未过期内容。") {
                    MenuChoice(options: [1, 2, 5, 10, 20, 50, 100].map { (Int64($0) * 1024 * 1024 * 1024, "\($0) GiB") },
                               selection: Binding(get: { session.maximumBytes }, set: session.setMaximumBytes))
                }
                HairlineDivider()
                SettingRow(title: "已用空间", detail: "数据卷可用空间不足 2 GiB 时也会停止新截图。") {
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
