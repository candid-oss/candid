import DesktopPlatform
import CandidLocalization
import SwiftUI
import ConsoleCore

// Disambiguate the established wrapper from the SDK 27 State macro, which CLT omits.
typealias ViewState<Value> = SwiftUI.State<Value>

@main
struct CandidApp: App {
    @NSApplicationDelegateAdaptor(DesktopApplicationDelegate.self) private var delegate
    @ViewState private var store: ConsoleStore?
    @ViewState private var desktop: DesktopSession
    @ViewState private var media: MediaWorkspaceStore
    private let locations: TimelineService.Locations
    private let configurationError: String?
    private let showMediaOnLaunch: Bool
    @AppStorage("console.appearance") private var preferredAppearance = "system"
    private let forceDark: Bool
    private var appearance: ColorScheme? { forceDark || preferredAppearance == "dark" ? .dark : preferredAppearance == "light" ? .light : nil }
    init() {
        DesktopAXProbe.runIfRequested()   // diagnostics only; never returns when requested
        // Slim overlay scroll bars that appear while scrolling, in this app only: the system's always-visible
        // legacy bars (a grey track) clash with the app's surfaces. Set before any scroll view exists.
        UserDefaults.standard.set("WhenScrolling", forKey: "AppleShowScrollBars")
        let args = ProcessInfo.processInfo.arguments
        forceDark = args.contains("--dark")
        let dataIndex = args.firstIndex(of: "--data-dir")
        let dataRoot = dataIndex.flatMap { $0 + 1 < args.count ? URL(fileURLWithPath: args[$0 + 1], isDirectory: true) : nil }
        let mediaIndex = args.firstIndex(of: "--media-profile")
        let mediaProfile = mediaIndex.flatMap { $0 + 1 < args.count ? URL(fileURLWithPath: args[$0 + 1], isDirectory: true) : nil }
        let isMock = args.contains("--mock")
        showMediaOnLaunch = mediaProfile != nil
        let profile = mediaProfile ?? (isMock
            ? FileManager.default.temporaryDirectory.appendingPathComponent("candid-media-demo-" + UUID().uuidString)
            : MediaWorkspaceStore.defaultProfile())
        // Demo and QA launches get a throwaway core and capture folder (the synthetic day is generated relative to
        // launch); `--data-dir` names an old desktop ledger, usually a copy, for that core to import.
        let locations = TimelineService.locations(isDemo: isMock, dataRoot: dataRoot, mediaProfile: profile)
        self.locations = locations
        _media = ViewState(initialValue: MediaWorkspaceStore(profile: profile,
                                                             demoRecords: isMock && mediaProfile == nil ? DesktopDemoContent.mediaRecords(now: Date()) : nil))
        _desktop = ViewState(initialValue: DesktopSession(isDemo: args.contains("--mock"), dataRoot: locations.capture, automaticCaptureAvailable: args.contains("--experimental-auto-capture")))
        do {
            let service: any ConsoleService
            if args.contains("--mock") || (!args.contains("--service") && UserDefaults.standard.bool(forKey: "console.demo")) { service = MockService() }
            else {
                let index = args.firstIndex(of: "--service")
                let endpoint = index.flatMap { $0 + 1 < args.count ? args[$0 + 1] : nil } ?? UserDefaults.standard.string(forKey: "console.endpoint") ?? "http://127.0.0.1:8766"
                service = try DebugClient(endpoint: endpoint)
            }
            _store = State(initialValue: ConsoleStore(service: service)); configurationError = nil
        } catch { _store = State(initialValue: nil); configurationError = error.localizedDescription }
    }
    var body: some Scene {
        Window("Candid", id: "desktop") {
            DesktopRootView(session: desktop, media: media, store: store, configurationError: configurationError,
                            showMediaOnLaunch: showMediaOnLaunch, delegate: delegate, locations: locations)
                .frame(minWidth: 1100, minHeight: 700)
                .preferredColorScheme(appearance)
        }
        .defaultSize(width: 1280, height: 840)
        .windowStyle(.hiddenTitleBar)
        .commands {
            CommandGroup(replacing: .newItem) { }
            CommandMenu(L("电脑采集")) {
                Button(desktop.isRecording ? L("暂停电脑采集") : L("开始电脑采集")) { desktop.isRecording ? desktop.pauseRecording() : desktop.startRecording() }
                Button(L("标记此刻"), action: desktop.addMarker).keyboardShortcut("m", modifiers: [.command, .shift])
                Button(L("截取允许的窗口"), action: desktop.captureNow).disabled(!desktop.isRecording)
            }
            CommandMenu(L("快捷操作")) {
                ForEach(QuickAction.allCases) { action in
                    Button(action.title) { NotificationCenter.default.post(name: .candidQuickAction, object: action) }
                }
                Divider() // style-exempt: native command menu separator
                Button(L("快捷键与悬浮控件设置")) {
                    UserDefaults.standard.set("shortcuts", forKey: "desktop.settings.tab")
                    NotificationCenter.default.post(name: .candidQuickAction, object: "settings")
                }
            }
            CommandMenu(L("资料")) {
                Button(L("开始系统声音与麦克风录音")) { Task { await media.startRecording(input: .systemAndMicrophone) } }
                    .disabled(!media.canCapture || !RecordingInput.systemAvailable)
                Toggle(L("通话时提醒录音"), isOn: Binding(get: { media.callRecordingReminders }, set: { media.callRecordingReminders = $0 }))
                    .disabled(media.isDemo || !RecordingInput.systemAvailable)
                Button(L("停止 Mac 录音")) { Task { await media.stopRecording() } }
                    .disabled(!media.isRecording || media.recordingFinishing)
            }
        }
        Settings {
            // ⌘, opens the same settings page as the main window, on the same backdrop.
            ZStack {
                WindowBackdrop(tint: .accentColor)
                SettingsHomeView(session: desktop, deviceSettings: store.map { AnyView(ConsoleConnectionSettings(store: $0)) })
            }
            .visualStyleFromSettings()
            .frame(minWidth: 860, minHeight: 600)
            .preferredColorScheme(appearance)
        }
    }
}

private struct DesktopRootView: View {
    @Environment(\.openWindow) private var openWindow
    let session: DesktopSession
    let media: MediaWorkspaceStore
    let store: ConsoleStore?
    let configurationError: String?
    let showMediaOnLaunch: Bool
    let delegate: DesktopApplicationDelegate
    let locations: TimelineService.Locations
    var body: some View {
        HomeRootView(session: session, media: media, deviceStore: store, configurationError: configurationError,
                     showMediaOnLaunch: showMediaOnLaunch)
            .task { await media.syncRecordings() }
            .task {
                if !session.isDemo && !QASnapshot.isActive { await SkillRunner.shared.runLoop() }
            }
            .task {
                delegate.session = session; delegate.media = media
                let open = openWindow
                delegate.installStatusItem { open(id: "desktop") }
                // The core first: capture settings may still live in the old ledger, which only the core opens.
                TimelineService.shared.start(locations)
                await session.bootstrap()
                HealthMonitor.shared.start(session: session)
            }
    }
}

@MainActor final class DesktopApplicationDelegate: NSObject, NSApplicationDelegate {
    var session: DesktopSession?
    var media: MediaWorkspaceStore?
    private var statusItem: StatusItemController?
    private var callReminder: CallRecordingReminder?
    private var reopenMainWindow: (() -> Void)?
    private var launchedAtLogin = false

    /// The menu bar icon and its panel: a transient popover, so a click anywhere else, another app or Esc closes it.
    func installStatusItem(openMainWindow: @escaping () -> Void) {
        reopenMainWindow = openMainWindow
        guard statusItem == nil, let session, let media else { return }
        statusItem = StatusItemController(session: session, media: media, openMainWindow: openMainWindow)
        RecordingControl.shared.attach(session: session, media: media)
        // A login launch stays in the menu bar; the window opens from the icon or the Dock.
        if launchedAtLogin { NSApp.windows.filter { $0.identifier?.rawValue.hasPrefix("desktop") == true }.forEach { $0.close() } }
        if !session.isDemo && !QASnapshot.isActive {
            callReminder = CallRecordingReminder(media: media)
            callReminder?.start()
        }
    }
    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        // Older builds kept their own vision endpoint and prompt; move them onto the model roles once.
        if !ProcessInfo.processInfo.arguments.contains("--qa-snapshot-dir") { AIRuntime.shared.migrateLegacyVision() }
        // A QA snapshot instance renders off to the side and never steals focus.
        if ProcessInfo.processInfo.arguments.contains("--qa-floating-dir") { QASnapshot.startIfRequested() }
        if ProcessInfo.processInfo.arguments.contains("--qa-snapshot-dir") {
            QASnapshot.startIfRequested()
        } else if LoginItem.launchedAtLogin {
            launchedAtLogin = true
        } else {
            NSApp.activate(ignoringOtherApps: true)
        }
    }
    /// `candid://` links (Sources/Candid/System/CandidURL.swift).
    func application(_ application: NSApplication, open urls: [URL]) {
        for url in urls {
            CandidURLHandler.handle(url) { [weak self] in self?.reopenMainWindow?(); NSApp.activate(ignoringOtherApps: true) }
        }
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag { reopenMainWindow?() }
        return true
    }
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        if media?.isRecording == true || media?.recordingFinishing == true || media?.busy == true {
            let alert = NSAlert()
            alert.messageText = media?.isRecording == true ? L("请先停止 Mac 录音") : L("资料操作正在进行")
            alert.informativeText = L("操作完成后可以退出 Candid。")
            alert.runModal()
            return .terminateCancel
        }
        callReminder?.stop()
        GlobalHotkeys.shared.stop()
        session?.prepareToQuit()
        TimelineService.shared.stop()
        return .terminateNow
    }
}

/// Menu bar icon whose panel is an `NSPopover` with transient behaviour (closes on any outside click, app switch or
/// Esc). The icon follows the session through observation tracking; nothing polls.
@MainActor final class StatusItemController: NSObject {
    private let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
    private let popover = NSPopover()
    private let session: DesktopSession
    private let media: MediaWorkspaceStore

    init(session: DesktopSession, media: MediaWorkspaceStore, openMainWindow: @escaping () -> Void) {
        self.session = session; self.media = media
        super.init()
        popover.behavior = .transient
        popover.animates = true
        let close: () -> Void = { [weak popover] in popover?.performClose(nil) }
        popover.contentViewController = NSHostingController(rootView: MenuBarPanel(session: session, media: media, openApp: {
            close(); openMainWindow(); NSApp.activate(ignoringOtherApps: true)
        }))
        item.button?.target = self
        item.button?.action = #selector(toggle(_:))
        item.button?.imagePosition = .imageLeading
        updateLabel()
    }

    @objc private func toggle(_ sender: NSStatusBarButton) {
        if popover.isShown { popover.performClose(sender); return }
        popover.show(relativeTo: sender.bounds, of: sender, preferredEdge: .minY)
        popover.contentViewController?.view.window?.makeKey()
    }

    private func updateLabel() {
        withObservationTracking {
            let alerting = !StatusAlerts.current(session: session, media: media).isEmpty || !HealthMonitor.shared.problems.isEmpty
            let state = CandidMark.state(recordingScreen: session.isRecording, recordingAudio: media.isRecording, alerting: alerting)
            item.button?.image = CandidMark.menuBarImage(state)
            item.button?.title = media.recordingClock.map { " " + $0 } ?? ""
            item.button?.setAccessibilityLabel(media.isRecording ? L("Candid 正在录音") : session.isRecording ? L("Candid 正在记录") : L("Candid 已暂停"))
        } onChange: { [weak self] in
            Task { @MainActor in self?.updateLabel() }
        }
    }
}
