import AppKit
import CandidLocalization
import Combine
import CryptoKit
import DesktopCore
import DesktopPlatform
import Foundation
import Observation
import CandidClient

@MainActor @Observable
final class DesktopSession {
    /// The 后台处理 pace that capture throttling follows (power v1 §5: no separate battery switch). Tests replace it.
    @ObservationIgnored var capturePace: @MainActor () -> BackgroundPace = { BackgroundPower.shared.pace }

    /// Sample sparingly on battery (or unknown power) unless the pace is 随时, and always in Low Power Mode.
    private func capturesSparingly(onBattery: Bool?, lowPower: Bool?) -> Bool {
        lowPower == true || (capturePace() != .anytime && onBattery != false)
    }

    var records: [DesktopRecord] = []
    /// The day's activities as summaries. Selecting one never changes this list (the timeline is drawn from it).
    private(set) var timelineEvents: [DesktopTimelineEvent] = []
    /// The selected activity with its observations; loaded on its own when the selection changes.
    private(set) var selectedDetail: DesktopTimelineEvent?
    var selectedID: String? {
        didSet {
            if oldValue != selectedID {
                observationPageGeneration &+= 1; observationPageTask?.cancel(); observationPageTask = nil; observationPageLoading = false
                selectedObservationID = nil; selectedScreenshotID = nil
                if !applyingList { scheduleDetail() }
            }
            loadSelectedImage()
        }
    }
    var selectedObservationID: String? { didSet { if oldValue != selectedObservationID { selectedScreenshotID = nil }; loadSelectedImage() } }
    var selectedScreenshotID: String? { didSet { loadSelectedImage() } }
    var searchText = "" { didSet { scheduleReload() } }
    var selectedDate = Date() { didSet { scheduleReload() } }
    private(set) var isRecording = false
    private(set) var screenshotsEnabled = false
    private(set) var statusText = L("已暂停")
    private(set) var statusDetail = L("记录由你开启，所有资料保存在本机。")
    private(set) var error: String?
    private(set) var notice: String?
    private(set) var statistics = DesktopStatistics()
    @ObservationIgnored private var statisticsLoaded: Date?
    private(set) var systemRows: [DesktopInfoRow] = []
    private(set) var applications: [DesktopApplication] = []
    private(set) var allowedBundleIDs: Set<String> = []
    private(set) var windowCompatibilityBundleIDs: Set<String> = []
    private(set) var excludedBundleIDs: Set<String> = []
    private(set) var textEnabled = false
    private(set) var textAllowedBundleIDs: Set<String> = []
    private(set) var periodicEnabled = false
    private(set) var periodicInterval: Double = 60
    private(set) var contentOptions = DesktopContentOptions()
    private(set) var imageOptions = DesktopImageOptions()
    private(set) var retentionDays = 7
    private(set) var maximumBytes: Int64 = 5 * 1024 * 1024 * 1024
    /// A screenshot was refused for the storage limit and freeing space did not make room; cleared by the next save
    /// or a new limit.
    private(set) var storageLimitRefused = false
    @ObservationIgnored private var lastSpaceFreeAttempt: Date?
    /// How full the screenshot storage limit is, for the status line, the menu bar and the settings page.
    var storageBudget: DesktopStorageBudget {
        DesktopStorageBudget(usedBytes: statistics.storageBytes, limitBytes: maximumBytes, refused: storageLimitRefused,
                             freedBytes: statistics.freedBytes, freedScreenshots: statistics.freedScreenshots)
    }
    private(set) var scanningApplications = false
    @ObservationIgnored private var catalogTask: Task<Void, Never>?
    private(set) var samplingOptions = DesktopSamplingOptions()
    private(set) var minimumCaptureInterval: Double = 15
    private(set) var samplingMode: DesktopSamplingMode = .balanced
    private(set) var interactionTriggersEnabled = false
    private(set) var interactionTriggersActive = false
    private(set) var inputMonitoringAuthorized = false
    private(set) var samplingDiagnostics = DesktopSamplingDiagnostics()
    private(set) var screenPermission = false
    private(set) var accessibilityPermission = false
    private(set) var captureBusy = false
    let isDemo: Bool
    private(set) var automaticCaptureAvailable: Bool
    @ObservationIgnored private var automaticCaptureQAOverride: Bool
    let dataLocation: String
    private(set) var lastSaved: Date?
    private(set) var droppedEvents = 0
    private(set) var captureAttempts = 0
    private(set) var captureFailures = 0
    private(set) var imageLocations: [String: URL] = [:]
    private(set) var imageLoading = false
    let recognition = DesktopRecognitionSession()
    private(set) var observationPageLoading = false
    var selectedEvent: DesktopTimelineEvent? {
        if let selectedDetail, selectedDetail.id == selectedID { return selectedDetail }
        return timelineEvents.first { $0.id == selectedID }
    }
    var selectedRecord: DesktopRecord? {
        guard let event = selectedEvent else { return nil }
        return event.observations.first { $0.id == selectedObservationID } ?? event.latestRecord
    }
    var selectedImageRecord: DesktopRecord? {
        guard let event = selectedEvent, let observation = selectedRecord else { return nil }
        let images = event.screenshots.filter { $0.parentID == observation.id }
        return images.first { $0.id == selectedScreenshotID }
            ?? images.last ?? (observation.imageHash != nil ? observation : nil)
    }

    @ObservationIgnored private let monitor: any DesktopMonitoring
    @ObservationIgnored private let capture: any DesktopCapturing
    @ObservationIgnored private let gate = CaptureCommitGate()
    @ObservationIgnored private let eventGate = CaptureCommitGate()
    @ObservationIgnored private var limiter = CaptureRateLimiter()
    @ObservationIgnored private var preferences = DesktopPreferences()
    @ObservationIgnored private var desiredPreferences = DesktopPreferences()
    @ObservationIgnored private var preferencesTask: Task<Void, Never>?
    /// Writes through the durable outbox into the timeline core; reads come back from the core.
    @ObservationIgnored private(set) var store: DesktopStore?
    @ObservationIgnored private let core: any DesktopCoreAccess
    @ObservationIgnored private let outboxLimits: DesktopOutbox.Limits
    @ObservationIgnored private var bootstrapped = false
    @ObservationIgnored private var storageReady = false
    /// Launch has finished loading settings; start and pause do nothing before this.
    var isReady: Bool { storageReady && store != nil }
    @ObservationIgnored private var scheduledCapture: Task<Void, Never>?
    @ObservationIgnored private var manualCaptureTask: Task<Void, Never>?
    @ObservationIgnored private var manualCaptureGeneration: UInt64 = 0
    @ObservationIgnored private var reloadTask: Task<Void, Never>?
    @ObservationIgnored private var selectedImageTask: Task<Void, Never>?
    @ObservationIgnored private var selectedImageRequestID: String?
    @ObservationIgnored private var selectedImageGeneration: UInt64 = 0
    private struct PendingEvent { let record: DesktopRecord; let epoch: UInt64? }
    @ObservationIgnored private var pendingEvents: [PendingEvent] = []
    @ObservationIgnored private var draining = false
    @ObservationIgnored private var generation: UInt64 = 0
    @ObservationIgnored private var queryGeneration: UInt64 = 0
    @ObservationIgnored private var consecutiveFailures = 0
    @ObservationIgnored private var cooldownUntil = Date.distantPast
    @ObservationIgnored private let runID = UUID().uuidString
    @ObservationIgnored private let root: URL
    @ObservationIgnored private let periodicSampler: DesktopPeriodicSampler
    @ObservationIgnored private var contextBusy = false
    @ObservationIgnored private var observationPageTask: Task<Void, Never>?
    @ObservationIgnored private var observationPageGeneration: UInt64 = 0
    private struct Activity { let id: String; let bundle: String; var title: String; let protected: Bool; let pid: Int32?; var windowIdentity: UInt? }
    @ObservationIgnored private var currentActivity: Activity?
    @ObservationIgnored private var pendingCapture: DesktopRecord?
    @ObservationIgnored private var pendingCaptureReadyAt: Double = 0
    @ObservationIgnored private var pendingCaptureRevision: UInt64 = 0
    @ObservationIgnored private var capturePumpGeneration: UInt64 = 0
    private var captureMessages: [String: String] = [:]
    typealias CaptureSleep = @MainActor (Duration) async throws -> Void
    @ObservationIgnored private let captureSleep: CaptureSleep
    @ObservationIgnored private let captureClock: @MainActor () -> Double
    @ObservationIgnored private var recentRecords: [String: DesktopRecord] = [:]
    @ObservationIgnored private var timelineVisible = true
    @ObservationIgnored private var applyingList = false
    @ObservationIgnored private var detailTask: Task<Void, Never>?
    @ObservationIgnored private var deliveryReloadPending = false
    @ObservationIgnored private var balancedCheckTask: Task<Void, Never>?
    @ObservationIgnored private var pendingBalancedCheck: (trigger: String, image: Bool)?
    @ObservationIgnored private var balancedCheckGeneration: UInt64 = 0
    @ObservationIgnored private var contentLimiter = CaptureRateLimiter()
    @ObservationIgnored private var lastSignatureActivityID: String?
    @ObservationIgnored private var lastContentSignature: String?
    @ObservationIgnored private var lastImageAttemptClock = -Double.infinity
    @ObservationIgnored private var sourceConfirmation = DesktopSourceConfirmation()
    @ObservationIgnored private var agentConsentSubscription: AnyCancellable?
    @ObservationIgnored private var nativeProofTask: Task<Void, Never>?
    @ObservationIgnored private var nativeDrafts: [String: DesktopRecord] = [:]
    @ObservationIgnored private var nativeConfirmed: Set<String> = []
    @ObservationIgnored private var nativeExamined: Set<String> = []
    private struct RecentImage {
        let digest: SHA256.Digest
        let contentSignature: String?
        var repeated = 0
    }
    @ObservationIgnored private var recentImages: [String: RecentImage] = [:]

    /// `dataRoot` is the capture folder (settings and outbox); `core` defaults to the App's timeline service.
    init(isDemo: Bool = false, dataRoot: URL? = nil, core: (any DesktopCoreAccess)? = nil, outboxLimits: DesktopOutbox.Limits = .init(), monitor: (any DesktopMonitoring)? = nil, capture: (any DesktopCapturing)? = nil, automaticCaptureAvailable: Bool = false, periodicSleep: DesktopPeriodicSampler.Sleep? = nil, captureSleep: CaptureSleep? = nil, captureClock: (@MainActor () -> Double)? = nil) {
        self.isDemo = isDemo
        self.automaticCaptureAvailable = automaticCaptureAvailable && !isDemo
        self.automaticCaptureQAOverride = automaticCaptureAvailable && !isDemo
        self.monitor = monitor ?? DesktopSystemMonitor()
        self.capture = capture ?? DesktopCaptureProvider()
        self.periodicSampler = periodicSleep.map { DesktopPeriodicSampler(sleep: $0) } ?? DesktopPeriodicSampler()
        self.captureSleep = captureSleep ?? { try await Task.sleep(for: $0) }
        self.captureClock = captureClock ?? { ProcessInfo.processInfo.systemUptime }
        self.core = core ?? TimelineService.shared
        self.outboxLimits = outboxLimits
        let root = dataRoot ?? (isDemo
            ? FileManager.default.temporaryDirectory.appendingPathComponent("candid-capture-demo-" + UUID().uuidString, isDirectory: true)
            : FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("Candid/capture", isDirectory: true))
        dataLocation = root.path
        self.root = root
        agentConsentSubscription = NotificationCenter.default.publisher(for: .candidAgentConsentChanged).receive(on: DispatchQueue.main).sink { [weak self] _ in
            MainActor.assumeIsolated { self?.invalidateContentPolicy() }
        }
        updateSystem()
    }

    func bootstrap() async {
        guard !bootstrapped else { return }; bootstrapped = true
        do {
            let store = try DesktopStore(directory: root, core: core, limits: outboxLimits)
            store.onDelivered = { [weak self] in self?.deliveredReload() }
            self.store = store
            (core as? TimelineService)?.desktopStore = store
            preferences = try await store.loadPreferences(wait: isDemo ? .zero : .seconds(5))
            contentOptions = preferences.contentOptions; imageOptions = preferences.imageOptions
            retentionDays = preferences.retentionDays; maximumBytes = preferences.maximumBytes
            capture.setImageOptions(preferences.imageOptions)
            samplingOptions = preferences.samplingOptions; minimumCaptureInterval = preferences.minimumCaptureInterval
            samplingMode = preferences.samplingMode
            interactionTriggersEnabled = preferences.interactionTriggersEnabled
            desiredPreferences = preferences
            updateWindowContentScope()
            allowedBundleIDs = preferences.allowedBundleIDs; excludedBundleIDs = preferences.excludedBundleIDs
            windowCompatibilityBundleIDs = preferences.windowCompatibilityBundleIDs
            screenshotsEnabled = preferences.screenshotEnabled
            automaticCaptureAvailable = !isDemo && (automaticCaptureQAOverride || preferences.automaticScreenshotEnabled)
            textEnabled = preferences.textEnabled; textAllowedBundleIDs = preferences.textAllowedBundleIDs
            periodicEnabled = preferences.periodicEnabled; periodicInterval = preferences.periodicInterval
            updateApplicationList()
            // Every launch starts paused; nothing restores an earlier capture state.
            if isDemo, await store.outbox.count == 0 { try await seedDemo() }
            try await store.deleteBefore(Calendar.current.date(byAdding: .day, value: -preferences.retentionDays, to: Date())!)
            storageReady = true
            recognition.attach(to: store, automaticEnabled: !isDemo)
            refreshApplicationCatalog()
            await reload()
        } catch { fail(error) }
    }

    func refresh() { updateSystem(); refreshApplicationCatalog(); selectedImageRequestID = nil; imageLocations.removeAll(); Task { await bootstrap(); await reload() } }
    private func refreshApplicationCatalog() {
        guard catalogTask == nil, !isDemo else { return }
        scanningApplications = true
        catalogTask = Task {
            await monitor.refreshApplicationCatalog()
            updateApplicationList(); scanningApplications = false; catalogTask = nil
        }
    }
    func addApplication(_ url: URL) {
        guard !isDemo, storageReady else { return }
        guard monitor.addApplication(url: url) else { error = L("请选择独立应用；辅助程序或扩展不能单独添加。"); return }
        updateApplicationList()
    }
    private func updateSystem() {
        let info = monitor.snapshot()
        systemRows = info.rows; screenPermission = info.screenPermission; accessibilityPermission = info.accessibilityPermission
        inputMonitoringAuthorized = monitor.inputMonitoringAuthorized
        interactionTriggersEnabled = desiredPreferences.interactionTriggersEnabled
        interactionTriggersActive = isRecording && monitor.interactionTriggersActive
        updateApplicationList()
    }
    private func updateApplicationList() {
        let running = monitor.applications()
        var known = Dictionary(running.map { ($0.bundleID, $0) }, uniquingKeysWith: { first, _ in first })
        let configured = preferences.allowedBundleIDs.union(preferences.textAllowedBundleIDs).union(preferences.excludedBundleIDs)
            .union(desiredPreferences.allowedBundleIDs).union(desiredPreferences.textAllowedBundleIDs).union(desiredPreferences.excludedBundleIDs)
        for bundle in configured where known[bundle] == nil {
            let resolved = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundle)
            let installed = resolved.flatMap { FileManager.default.fileExists(atPath: $0.path) ? $0 : nil }
            let name = installed?.deletingPathExtension().lastPathComponent ?? bundle
            known[bundle] = DesktopApplication(bundleID: bundle, name: name, pid: 0, bundlePath: installed?.path)
        }
        applications = known.values.sorted {
            let comparison = $0.name.localizedStandardCompare($1.name)
            return comparison == .orderedSame ? $0.bundleID < $1.bundleID : comparison == .orderedAscending
        }
    }

    func startRecording() {
        guard storageReady, store != nil, !isRecording else { return }
        generation &+= 1
        isRecording = true; _ = gate.enable(); _ = eventGate.enable()
        statusText = L("正在记录"); statusDetail = screenshotsEnabled ? (automaticCaptureAvailable ? L("实验模式：允许范围内的窗口按事件采样。") : L("记录应用活动；可手动测试允许范围内的窗口截图。")) : L("轻量应用事件已开启，截图仍由你单独控制。")
        enqueue(DesktopRecord(kind: .system, detail: L("用户开启应用活动记录"), runID: runID))
        if !isDemo {
            updateWindowContentScope()
            monitor.setInteractionTriggerSelection(returnEnabled: preferences.samplingOptions.returnEnabled, scrollEnabled: preferences.samplingOptions.scrollEnabled)
            _ = monitor.setInteractionTriggersEnabled(preferences.interactionTriggersEnabled)
            monitor.start { [weak self] in self?.handle($0) }
            restartPeriodicSampling(); updateSystem()
        }
    }

    func pauseRecording() {
        inputWindows.removeAll(); windowLinks.removeAll(); sourceConfirmation.reset(); nativeProofTask?.cancel(); nativeProofTask = nil; nativeDrafts.removeAll(); nativeConfirmed.removeAll(); nativeExamined.removeAll()
        resetSamplingActivity(); currentActivity = nil
        recentImages.removeAll()
        invalidateCapture(reason: L("已暂停，未截图"))
        generation &+= 1; eventGate.invalidate(); gate.invalidate(); periodicSampler.stop(); capture.cancelPendingCapture(); scheduledCapture?.cancel(); scheduledCapture = nil; cancelManualCapture()
        monitor.stop(); isRecording = false
        statusText = L("已暂停"); statusDetail = L("不会新增活动或截图。已有记录仍可查看和导出。")
    }

    func prepareToQuit() { recognition.stop(); resetSamplingActivity(); currentActivity = nil; invalidateCapture(reason: L("应用已退出，未截图")); eventGate.invalidate(); gate.invalidate(); periodicSampler.stop(); capture.cancelPendingCapture(); monitor.stop(); scheduledCapture?.cancel(); cancelManualCapture(); selectedImageTask?.cancel(); isRecording = false }

    func requestScreenPermission() {
        guard !isDemo else { return }
        if !capture.requestScreenPermission(), let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture") { NSWorkspace.shared.open(url) }
        updateSystem()
    }
    func requestAccessibilityPermission() {
        guard !isDemo else { return }
        if !capture.requestAccessibilityPermission(), let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") { NSWorkspace.shared.open(url) }
        updateSystem()
    }
    func setScreenshotsEnabled(_ enabled: Bool) {
        guard !isDemo, storageReady else { return }
        invalidateCapture()
        if enabled {
            updateSystem()
            guard screenPermission && accessibilityPermission else { error = L("请先允许屏幕录制和辅助功能，再启用窗口截图。应用活动不受影响。"); return }
            guard !allowedBundleIDs.isEmpty else { error = L("请先选择至少一个允许截图的应用。"); return }
        }
        desiredPreferences.screenshotEnabled = enabled
        if !enabled {
            desiredPreferences.automaticScreenshotEnabled = false
            automaticCaptureQAOverride = false
            automaticCaptureAvailable = false
        }
        savePreferences()
    }
    func setAutomaticCaptureEnabled(_ enabled: Bool) {
        guard !isDemo, storageReady else { return }
        invalidateCapture()
        if enabled {
            updateSystem()
            guard screenshotsEnabled, desiredPreferences.screenshotEnabled, screenPermission, accessibilityPermission,
                  !preferences.allowedBundleIDs.intersection(desiredPreferences.allowedBundleIDs)
                    .subtracting(preferences.excludedBundleIDs.union(desiredPreferences.excludedBundleIDs)).isEmpty else {
                error = L("请先授权、选择应用并开启窗口截图。"); return
            }
        }
        automaticCaptureQAOverride = false
        desiredPreferences.automaticScreenshotEnabled = enabled
        if !enabled { automaticCaptureAvailable = false }
        savePreferences()
    }
    func setAllowedApp(_ bundle: String, _ allowed: Bool) {
        guard !isDemo, storageReady else { return }
        currentActivity = nil
        invalidateCapture()
        eventGate.invalidate(); if isRecording { _ = eventGate.enable() }
        if allowed { desiredPreferences.allowedBundleIDs.insert(bundle) }
        else {
            desiredPreferences.allowedBundleIDs.remove(bundle)
            desiredPreferences.windowCompatibilityBundleIDs.remove(bundle)
        }
        updateWindowContentScope()
        savePreferences()
    }
    func setWindowCompatibility(_ bundle: String, _ enabled: Bool) {
        guard !isDemo, storageReady, desiredPreferences.allowedBundleIDs.contains(bundle),
              !desiredPreferences.excludedBundleIDs.contains(bundle) else { return }
        invalidateCapture()
        if enabled { desiredPreferences.windowCompatibilityBundleIDs.insert(bundle) }
        else { desiredPreferences.windowCompatibilityBundleIDs.remove(bundle) }
        savePreferences()
    }
    func setExcludedApp(_ bundle: String, _ excluded: Bool) {
        guard !isDemo, storageReady else { return }
        currentActivity = nil
        invalidateCapture()
        eventGate.invalidate(); if isRecording { _ = eventGate.enable() }
        if excluded {
            desiredPreferences.excludedBundleIDs.insert(bundle)
            desiredPreferences.windowCompatibilityBundleIDs.remove(bundle)
        } else { desiredPreferences.excludedBundleIDs.remove(bundle) }
        updateWindowContentScope()
        savePreferences()
    }
    func setTextEnabled(_ enabled: Bool) {
        guard !isDemo, storageReady else { return }
        if enabled {
            updateSystem()
            guard accessibilityPermission else { error = L("请先允许辅助功能。"); return }
            guard !textAllowedBundleIDs.isEmpty else { error = L("请先在应用设置中选择允许读取文字的软件。"); return }
        }
        invalidateContentPolicy()
        desiredPreferences.textEnabled = enabled; updateWindowContentScope(); savePreferences()
    }
    func setInputValueAllowedApp(_ bundle: String, _ allowed: Bool) {
        guard !isDemo, storageReady, !allowed || effectiveTextScope.contains(bundle) else { return }
        invalidateContentPolicy()
        var scope = desiredPreferences.contentOptions.inputValueBundleIDs ?? []
        if allowed { scope.insert(bundle) } else { scope.remove(bundle) }
        desiredPreferences.contentOptions.inputValueBundleIDs = scope
        updateWindowContentScope(); savePreferences()
    }
    func setTextAllowedApp(_ bundle: String, _ allowed: Bool) {
        guard !isDemo, storageReady else { return }
        invalidateContentPolicy()
        if allowed { desiredPreferences.textAllowedBundleIDs.insert(bundle) }
        else { desiredPreferences.textAllowedBundleIDs.remove(bundle) }
        updateWindowContentScope(); savePreferences()
    }
    func setContentAllowedForApps(_ bundles: Set<String>, allowed: Bool) {
        guard !isDemo, storageReady, bundles.count <= 256 else { return }
        let targets = bundles.intersection(Set(applications.map(\.bundleID)))
            .subtracting(desiredPreferences.excludedBundleIDs)
        invalidateContentPolicy()
        if allowed {
            desiredPreferences.allowedBundleIDs.formUnion(targets)
            desiredPreferences.textAllowedBundleIDs.formUnion(targets)
        } else {
            desiredPreferences.allowedBundleIDs.subtract(bundles)
            desiredPreferences.windowCompatibilityBundleIDs.subtract(bundles)
            desiredPreferences.textAllowedBundleIDs.subtract(bundles)
        }
        updateWindowContentScope(); savePreferences()
    }
    func setPeriodicEnabled(_ enabled: Bool) {
        guard !isDemo, storageReady else { return }
        periodicSampler.stop(); invalidateContentPolicy()
        desiredPreferences.periodicEnabled = enabled; savePreferences()
    }
    func setPeriodicInterval(_ interval: Double) {
        guard !isDemo, storageReady, DesktopSamplingOptions.periodicIntervalRange.contains(interval) else { return }
        periodicSampler.stop(); desiredPreferences.periodicInterval = interval; savePreferences()
    }
    func setSamplingMode(_ mode: DesktopSamplingMode) {
        guard !isDemo, storageReady else { return }
        invalidateContentPolicy()
        desiredPreferences.samplingMode = mode
        savePreferences()
    }
    func setInteractionTriggersEnabled(_ enabled: Bool) {
        guard !isDemo, storageReady else { return }
        inputMonitoringAuthorized = monitor.inputMonitoringAuthorized
        guard !enabled || inputMonitoringAuthorized else { error = L("请先允许输入监控。"); return }
        if !monitor.setInteractionTriggersEnabled(enabled), enabled { error = L("回车与滚动监听暂不可用。"); return }
        desiredPreferences.interactionTriggersEnabled = enabled; savePreferences()
    }
    func setSamplingOptions(_ options: DesktopSamplingOptions) {
        guard !isDemo, storageReady, options.isValid else { return }
        invalidateContentPolicy()
        desiredPreferences.samplingOptions = options; savePreferences()
    }
    func updateSamplingOption<Value>(_ keyPath: WritableKeyPath<DesktopSamplingOptions, Value>, value: Value) {
        var options = desiredPreferences.samplingOptions
        options[keyPath: keyPath] = value
        setSamplingOptions(options)
    }
    func updateContentOption<Value>(_ keyPath: WritableKeyPath<DesktopContentOptions, Value>, value: Value) {
        guard !isDemo, storageReady else { return }
        invalidateContentPolicy()
        desiredPreferences.contentOptions[keyPath: keyPath] = value
        monitor.setContentOptions(desiredPreferences.contentOptions); savePreferences()
    }
    func updateImageOption<Value>(_ keyPath: WritableKeyPath<DesktopImageOptions, Value>, value: Value) {
        guard !isDemo, storageReady else { return }
        var options = desiredPreferences.imageOptions; options[keyPath: keyPath] = value
        guard options.isValid else { return }
        invalidateContentPolicy(); desiredPreferences.imageOptions = options; savePreferences()
    }
    func retentionImpact(days: Int) async -> Int? {
        guard (1...3650).contains(days), let cutoff = Calendar.current.date(byAdding: .day, value: -days, to: Date()) else { return nil }
        do { return try await store?.retentionCount(before: cutoff) }
        catch { self.error = error.localizedDescription; return nil }
    }
    func setRetentionDays(_ days: Int) {
        guard !isDemo, storageReady, (1...3650).contains(days) else { return }
        desiredPreferences.retentionDays = days; savePreferences()
    }
    func setMaximumBytes(_ bytes: Int64) {
        guard !isDemo, storageReady, (1024 * 1024 * 1024...100 * 1024 * 1024 * 1024).contains(bytes) else { return }
        invalidateCapture(); desiredPreferences.maximumBytes = bytes; savePreferences()
        storageLimitRefused = false; lastSpaceFreeAttempt = nil
    }
    /// Raises the screenshot storage limit to the next choice the settings offer.
    func raiseStorageLimit() {
        guard let next = storageBudget.raisedLimit else { return }
        setMaximumBytes(next)
    }
    /// Over the limit, the core keeps only the text of the oldest screenshots (over 7 days old) until the library is
    /// back under ~90% of it. Returns what the limit counts afterwards (library originals and the outbox).
    private func freeScreenshotSpace(store: DesktopStore, limit: Int64) async -> Int64 {
        let outbox = await store.outbox.bytes
        // At most once a minute: when only recent screenshots are left, every refused capture would ask again.
        if let last = lastSpaceFreeAttempt, Date().timeIntervalSince(last) < 60 { return statistics.storageBytes + outbox }
        lastSpaceFreeAttempt = Date()
        do {
            let report = try await store.freeScreenshotSpace(targetBytes: DesktopStorageBudget.target(limit: limit) - outbox)
            statistics.storageBytes = report.storage_bytes
            statistics.freedBytes = report.total_freed_bytes; statistics.freedScreenshots = report.total_screenshots
            if report.screenshots > 0 {
                notice = L("截图空间已满：%@ 张 7 天前的截图改为只留文字，腾出 %@。", report.screenshots,
                           ByteCountFormatter.string(fromByteCount: report.freed_bytes, countStyle: .file))
            }
            return report.storage_bytes + outbox
        } catch {
            return statistics.storageBytes + outbox
        }
    }
    struct InputWindow: Identifiable {
        var id: String { record.id }
        let record: DesktopRecord
        var label: String { record.appName + " · " + record.windowTitle }
    }
    private(set) var inputWindows: [InputWindow] = []
    private struct WindowLink { let object: String; let session: String; let expires: Date }
    private var windowLinks: [String: WindowLink] = [:]
    private func windowKey(_ record: DesktopRecord) -> String? {
        guard let field = record.context?.recordEvidence?.field else { return nil }
        let identity = field.split(separator: ":").prefix(2).joined(separator: ":")
        guard !identity.isEmpty else { return nil }
        return record.runID + "|" + record.bundleID + "|" + identity
    }
    func clearWindowLinks() { windowLinks.removeAll(); invalidateContentPolicy() }
    func associateWindow(_ id: String, with reference: String) async -> Bool {
        guard let record = inputWindows.first(where: { $0.id == id })?.record,
              let key = windowKey(record), let object = record.context?.recordEvidence?.object.key,
              let channel = core.channel, isRecording else { return false }
        do {
            struct Settings: Decodable { struct Consent: Decodable { var agent: String; var read: Bool; var activity: Bool }; var consents: [Consent] }
            let agent = reference.split(separator: ":").dropFirst().first.map(String.init) ?? ""
            let settings = try JSONDecoder().decode(Settings.self, from: await channel.send("GET", "/local/agents").checked())
            guard settings.consents.contains(where: { $0.agent == agent && $0.read && $0.activity }) else { return false }
            let args: [String: CoreJSON] = ["agent": .string(agent), "action": .string("read"), "reference": .string(reference), "limit": .int(1)]
            _ = try await channel.send("POST", "/local/agents/query", body: JSONEncoder().encode(args)).checked()
            guard isRecording, inputWindows.contains(where: { $0.id == id }), record.runID == runID else { return false }
            windowLinks[key] = WindowLink(object: object, session: reference, expires: Date().addingTimeInterval(600))
            return true
        } catch { self.error = L("会话尚不可关联，请核对来源授权和会话引用"); return false }
    }
    /// Explicit user confirmation is screen evidence under the screen source grant.
    @discardableResult func confirmScreenInput(_ id: String) -> Bool {
        guard isRecording, let record = inputWindows.first(where: { $0.id == id })?.record,
              let text = record.context?.recordEvidence?.input_value, !text.isEmpty else { return false }
        var proof = record; proof.id = "screen-confirmed-" + record.id; proof.trigger = "send_confirmed"
        proof.context?.recordEvidence?.confirmation = .init(observation: record.id, messageID: "screen-user-" + proof.id, text: text, basis: "outgoing_message")
        queueWrite(PendingEvent(record: publicationRecord(proof), epoch: eventGate.currentEpoch()))
        return true
    }
    private func publicationRecord(_ record: DesktopRecord) -> DesktopRecord {
        var published = DesktopContentRedaction.apply(record, options: desiredPreferences.contentOptions)
        if let e = published.context?.recordEvidence, e.input_status == "captured", e.input_value?.isEmpty == false, e.confirmation == nil, let key = windowKey(published) {
            inputWindows.removeAll { windowKey($0.record) == key }
            inputWindows.append(InputWindow(record: published))
            if inputWindows.count > 16 { inputWindows.removeFirst() }
            if let link = windowLinks[key], link.object == e.object.key, link.expires > Date() {
                published.context?.recordEvidence?.object = .init(key: link.session, basis: "sidebar")
            }
        }
        if let e = published.context?.recordEvidence, e.input_status == "captured", e.input_value?.isEmpty == false,
           e.confirmation == nil, DesktopNativeSendProof.session(for: published) == nil {
            published.detail = L("输入已保存，发出未确认：来源未提供稳定会话身份")
        }
        return published
    }
    func setMinimumCaptureInterval(_ interval: Double) {
        guard !isDemo, storageReady, DesktopSamplingOptions.samplingIntervalRange.contains(interval) else { return }
        invalidateContentPolicy()
        desiredPreferences.minimumCaptureInterval = interval; savePreferences()
    }
    func setInteractionTrigger(_ trigger: String, enabled: Bool) {
        guard !isDemo, storageReady, trigger == "return" || trigger == "scroll" else { return }
        inputMonitoringAuthorized = monitor.inputMonitoringAuthorized
        guard !enabled || inputMonitoringAuthorized else { error = L("请先允许输入监控。"); return }
        var options = desiredPreferences.samplingOptions
        // Disabled legacy master represents both triggers off.
        if !desiredPreferences.interactionTriggersEnabled { options.returnEnabled = false; options.scrollEnabled = false }
        if trigger == "return" { options.returnEnabled = enabled } else { options.scrollEnabled = enabled }
        invalidateContentPolicy()
        desiredPreferences.samplingOptions = options
        desiredPreferences.interactionTriggersEnabled = options.returnEnabled || options.scrollEnabled
        savePreferences()
    }
    func restoreSamplingDefaults() {
        guard !isDemo, storageReady else { return }
        invalidateContentPolicy()
        desiredPreferences.samplingMode = .balanced
        desiredPreferences.samplingOptions = DesktopSamplingOptions()
        desiredPreferences.minimumCaptureInterval = 15; desiredPreferences.periodicInterval = 60
        desiredPreferences.interactionTriggersEnabled = false
        savePreferences()
    }
    func applyFastSamplingFrequency() {
        guard !isDemo, storageReady else { return }
        invalidateContentPolicy()
        desiredPreferences.minimumCaptureInterval = 3
        desiredPreferences.periodicInterval = 10
        desiredPreferences.samplingOptions.minimumContentInterval = 3
        desiredPreferences.samplingOptions.contentCapacity = 20
        desiredPreferences.samplingOptions.imageCapacity = 20
        desiredPreferences.samplingOptions.imageFallbackInterval = 10
        savePreferences()
    }
    func requestInputMonitoringPermission() {
        guard !isDemo else { return }
        if !monitor.requestInputMonitoringPermission(), let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent") { NSWorkspace.shared.open(url) }
        updateSystem()
    }
    /// Per-day amounts for the date jumper; throws while the core is not ready.
    func dayAmounts(_ days: [DateInterval]) async throws -> [Date: DayAmount] {
        guard let store else { throw DesktopStoreError.unavailable(L("时间线尚未就绪")) }
        return try await store.dayAmounts(days)
    }
    func setTimelineVisible(_ visible: Bool) {
        guard timelineVisible != visible else { return }
        timelineVisible = visible
        if visible { updateLoadedRecords(); scheduleReload() } else { reloadTask?.cancel(); queryGeneration &+= 1 }
    }
    private func resetSamplingActivity() {
        balancedCheckGeneration &+= 1; balancedCheckTask?.cancel(); balancedCheckTask = nil; pendingBalancedCheck = nil
        lastContentSignature = nil; lastImageAttemptClock = -Double.infinity
    }
    private func invalidateContentPolicy() {
        inputWindows.removeAll(); windowLinks.removeAll(); sourceConfirmation.reset(); nativeProofTask?.cancel(); nativeProofTask = nil; nativeDrafts.removeAll(); nativeConfirmed.removeAll(); nativeExamined.removeAll()
        resetSamplingActivity(); currentActivity = nil
        invalidateCapture(); eventGate.invalidate(); if isRecording { _ = eventGate.enable() }
    }
    private func invalidateCapture(cancelManual: Bool = true, reason: String = L("采集设置已变化，未截图")) {
        capturePumpGeneration &+= 1
        if let pendingCapture { setCaptureMessage(pendingCapture.id, reason) }
        pendingCapture = nil
        gate.invalidate(); scheduledCapture?.cancel(); scheduledCapture = nil; capture.cancelPendingCapture()
        if cancelManual { cancelManualCapture() }
        if isRecording { _ = gate.enable() }
    }
    private func captureSwitchReason(to app: DesktopApplication) -> String {
        let allowed = app.bundleID != Bundle.main.bundleIdentifier &&
            preferences.allowedBundleIDs.contains(app.bundleID) && desiredPreferences.allowedBundleIDs.contains(app.bundleID) &&
            !preferences.excludedBundleIDs.contains(app.bundleID) && !desiredPreferences.excludedBundleIDs.contains(app.bundleID)
        return allowed ? L("已切换应用，取消上一窗口待截图") : L("已切到截图范围外，取消上一窗口待截图")
    }
    private func cancelManualCapture() {
        manualCaptureGeneration &+= 1
        manualCaptureTask?.cancel(); manualCaptureTask = nil; notice = nil
    }
    private func savePreferences() {
        let desired = desiredPreferences
        let previous = preferencesTask
        preferencesTask = Task {
            await previous?.value
            do {
                try store?.savePreferences(desired)
                // Serial actor writes preserve request order; only display the latest desired revision.
                if desiredPreferences == desired {
                    preferences = desired
                    screenshotsEnabled = desired.screenshotEnabled
                    automaticCaptureAvailable = !isDemo && (automaticCaptureQAOverride || desired.automaticScreenshotEnabled)
                    allowedBundleIDs = desired.allowedBundleIDs; excludedBundleIDs = desired.excludedBundleIDs
                    windowCompatibilityBundleIDs = desired.windowCompatibilityBundleIDs
                    textEnabled = desired.textEnabled; textAllowedBundleIDs = desired.textAllowedBundleIDs
                    periodicEnabled = desired.periodicEnabled; periodicInterval = desired.periodicInterval
                    contentOptions = desired.contentOptions; imageOptions = desired.imageOptions
                    retentionDays = desired.retentionDays; maximumBytes = desired.maximumBytes
                    capture.setImageOptions(desired.imageOptions)
                    samplingOptions = desired.samplingOptions; minimumCaptureInterval = desired.minimumCaptureInterval
                    samplingMode = desired.samplingMode
                    monitor.setInteractionTriggerSelection(returnEnabled: desired.samplingOptions.returnEnabled, scrollEnabled: desired.samplingOptions.scrollEnabled)
                    _ = monitor.setInteractionTriggersEnabled(desired.interactionTriggersEnabled)
                    interactionTriggersEnabled = desired.interactionTriggersEnabled
                    interactionTriggersActive = isRecording && monitor.interactionTriggersActive
                    updateApplicationList()
                    updateWindowContentScope()
                    restartPeriodicSampling()
                }
            } catch { gate.invalidate(); fail(error) }
        }
    }
    private func updateWindowContentScope() {
        monitor.setContentOptions(desiredPreferences.contentOptions)
        monitor.setSamplingPolicy(balanced: desiredPreferences.samplingMode == .balanced)
        monitor.setTextContentScope(allowedBundleIDs: effectiveTextScope)
        monitor.setWindowContentScope(allowedBundleIDs: effectiveWindowScope)
    }
    private var effectiveTextScope: Set<String> {
        guard preferences.textEnabled, desiredPreferences.textEnabled else { return [] }
        return preferences.textAllowedBundleIDs.intersection(desiredPreferences.textAllowedBundleIDs)
            .subtracting(preferences.excludedBundleIDs.union(desiredPreferences.excludedBundleIDs))
    }
    private var effectiveWindowScope: Set<String> {
        preferences.allowedBundleIDs.intersection(desiredPreferences.allowedBundleIDs)
            .subtracting(preferences.excludedBundleIDs.union(desiredPreferences.excludedBundleIDs)).union(effectiveTextScope)
    }
    private func stateContext(_ existing: DesktopContextSnapshot? = nil) -> DesktopContextSnapshot {
        var result = existing ?? DesktopContextSnapshot()
        let state = monitor.currentContextState()
        result.sessionActive = state.sessionActive; result.onBattery = state.onBattery; result.lowPower = state.lowPower
        result.thermalState = state.thermalState; result.idleSeconds = state.idleSeconds
        return result
    }
    private func scopedContext(_ original: DesktopContextSnapshot?, bundle: String) -> DesktopContextSnapshot {
        var result = stateContext(original)
        if !effectiveTextScope.contains(bundle) {
            result.recordEvidence = nil; result.selectedText = nil; result.readableText = nil; result.axEvidence = nil
            result.textStatus = preferences.textEnabled && desiredPreferences.textEnabled ? "not_allowed" : "disabled"
        }
        if preferences.contentOptions.inputValueBundleIDs?.contains(bundle) != true || desiredPreferences.contentOptions.inputValueBundleIDs?.contains(bundle) != true {
            result.recordEvidence?.confirmation = nil; result.recordEvidence?.input_value = nil; result.recordEvidence?.input_status = "disabled"
        }
        if !effectiveWindowScope.contains(bundle) { result.focusedRole = nil; result.focusedLabel = nil }
        if result.isProtected == true { result.recordEvidence = nil; result.selectedText = nil; result.readableText = nil; result.axEvidence = nil; result.focusedLabel = nil; result.textStatus = "protected" }
        if result.isComposing == true { result.recordEvidence = nil; result.selectedText = nil; result.readableText = nil; result.axEvidence = nil; result.textStatus = "composing" }
        return result
    }
    private func restartPeriodicSampling() {
        periodicSampler.stop()
        guard !isDemo, isRecording, monitor.isSessionActive, preferences.periodicEnabled, desiredPreferences.periodicEnabled else { return }
        let state = monitor.snapshot()
        let interval = capturesSparingly(onBattery: state.onBattery, lowPower: state.lowPower) ? max(60, preferences.periodicInterval) : preferences.periodicInterval
        periodicSampler.start(interval: interval) { [weak self] in await self?.samplePeriodicContext() }
    }
    func samplePeriodicContext() async {
        if samplingMode == .balanced {
            guard isRecording, monitor.isSessionActive, periodicEnabled else { return }
            requestBalancedCheck(trigger: "timer", image: false)
            return
        }
        guard isRecording, monitor.isSessionActive, preferences.periodicEnabled, desiredPreferences.periodicEnabled, !contextBusy, !monitor.isContentSettling else { return }
        guard let app = monitor.frontmostApplication, app.bundleID != Bundle.main.bundleIdentifier,
              !preferences.excludedBundleIDs.union(desiredPreferences.excludedBundleIDs).contains(app.bundleID) else { return }
        contextBusy = true; defer { contextBusy = false }
        let epoch = eventGate.currentEpoch()
        var title = "", context = scopedContext(nil, bundle: app.bundleID)
        var windowIdentity: UInt?
        if effectiveWindowScope.contains(app.bundleID), !monitor.snapshot().thermalBlocked {
            do {
                let reading = try await monitor.readContext(allowedBundleIDs: effectiveWindowScope, includeText: effectiveTextScope.contains(app.bundleID))
                guard reading.application.pid == app.pid else { return }
                title = reading.title; context = scopedContext(reading.context, bundle: app.bundleID); windowIdentity = reading.windowIdentity
            } catch {
                context.textStatus = error as? DesktopCaptureError == .accessibilityPermissionDenied ? "permission_denied" : "unavailable"
            }
        }
        guard !Task.isCancelled, isRecording, monitor.isSessionActive, eventGate.permits(epoch), desiredPreferences.periodicEnabled else { return }
        guard monitor.frontmostApplication?.pid == app.pid else { return }
        guard !monitor.isContentSettling, context.isComposing != true else { return }
        let record = assignActivity(DesktopRecord(kind: .context, appName: app.name, bundleID: app.bundleID, windowTitle: title, runID: runID, trigger: "timer", context: context), windowIdentity: windowIdentity)
        enqueue(record)
        if automaticCaptureAvailable, screenshotsEnabled, desiredPreferences.screenshotEnabled,
           context.isProtected != true, allowedBundleIDs.contains(app.bundleID), manualCaptureTask == nil,
           periodicImageIsDue(baseInterval: preferences.periodicInterval) {
            scheduleCapture(parent: record, automatic: true)
        }
    }

    private var samplingWindow: String? {
        guard let app = monitor.frontmostApplication else { return nil }
        let window = currentActivity?.pid == app.pid ? currentActivity?.windowIdentity : nil
        return DesktopContentSignature.source(bundleID: app.bundleID, pid: app.pid, windowIdentity: window, title: "")
    }

    private func periodicImageIsDue(baseInterval: Double) -> Bool {
        let repeated = samplingWindow.flatMap { recentImages[$0]?.repeated } ?? 0
        let interval = max(baseInterval, min(300, baseInterval * Double(1 << repeated)))
        return captureClock() - lastImageAttemptClock >= interval
    }

    /// Raw change notifications are cheap hints. Only source changes, explicit
    /// interaction hints and periodic deadlines request a bounded content read.
    private func handleBalancedSignal(_ event: DesktopSystemEvent) -> Bool {
        guard event.kind == .application || event.kind == .window || event.kind == .context else { return false }
        guard let app = event.application, monitor.isSessionActive else { return true }
        guard app.bundleID != Bundle.main.bundleIdentifier,
              !preferences.excludedBundleIDs.union(desiredPreferences.excludedBundleIDs).contains(app.bundleID) else {
            resetSamplingActivity(); currentActivity = nil
            invalidateCapture(cancelManual: false, reason: captureSwitchReason(to: app)); return true
        }
        if ["content", "selection", "return", "mouse", "scroll"].contains(event.trigger ?? ""), event.context?.recordEvidence != nil {
            let context = scopedContext(event.context, bundle: app.bundleID)
            guard effectiveTextScope.contains(app.bundleID), context.isProtected != true, context.isComposing != true else { return true }
            let record = assignActivity(DesktopRecord(kind: .context, appName: app.name, bundleID: app.bundleID,
                windowTitle: event.title, detail: ["return", "mouse"].contains(event.trigger ?? "") ? "输入操作已记录，发出未确认" : "", runID: runID, trigger: event.trigger, context: context), windowIdentity: event.windowIdentity)
            let signature = DesktopContentSignature.make(record)
            if signature != lastContentSignature {
                lastContentSignature = signature; enqueue(record)
                if automaticCaptureAvailable, screenshotsEnabled, desiredPreferences.screenshotEnabled,
                   allowedBundleIDs.contains(app.bundleID), manualCaptureTask == nil {
                    scheduleCapture(parent: record, automatic: true, delay: 0)
                }
            }
            return true
        }
        if event.trigger == "dirty" || event.trigger == "content" || event.trigger == "selection" || event.trigger == "settled" { return true }
        if event.trigger == "return" || event.trigger == "mouse" || event.trigger == "scroll" {
            guard effectiveWindowScope.contains(app.bundleID) else { return true }
            requestBalancedCheck(trigger: event.trigger!, image: true)
            return true
        }
        let title = effectiveWindowScope.contains(app.bundleID) ? event.title : ""
        let context = scopedContext(event.context, bundle: app.bundleID)
        if event.kind == .application {
            resetSamplingActivity(); invalidateCapture(cancelManual: false, reason: captureSwitchReason(to: app))
            let record = assignActivity(DesktopRecord(kind: .application, appName: app.name, bundleID: app.bundleID,
                detail: event.detail, runID: runID, trigger: "application", context: context), forceNew: true)
            enqueue(record)
            return true
        }
        guard context.isComposing != true else { return true }
        let newSource = currentActivity == nil || currentActivity?.bundle != app.bundleID || currentActivity?.pid != app.pid ||
            (event.windowIdentity != nil && event.windowIdentity != currentActivity?.windowIdentity) ||
            (!title.isEmpty && title != currentActivity?.title) || currentActivity?.protected != (context.isProtected == true)
        if newSource {
            resetSamplingActivity(); invalidateCapture(cancelManual: false, reason: L("窗口或保护状态已变化，未截图"))
            let record = assignActivity(DesktopRecord(kind: event.kind == .window ? .window : .context,
                appName: app.name, bundleID: app.bundleID, windowTitle: title, detail: event.detail,
                runID: runID, trigger: event.trigger ?? "window", context: context), windowIdentity: event.windowIdentity)
            enqueue(record)
            if preferences.samplingOptions.windowTriggersEnabled, context.isProtected != true, effectiveWindowScope.contains(app.bundleID) {
                requestBalancedCheck(trigger: "window", image: true)
            }
        }
        return true
    }

    private func requestBalancedCheck(trigger: String, image: Bool) {
        guard isRecording, monitor.isSessionActive, samplingMode == .balanced else { return }
        // A timer must not downgrade an already pending interaction screenshot.
        pendingBalancedCheck = (trigger, image || (pendingBalancedCheck?.image ?? false))
        guard balancedCheckTask == nil else { return }
        let expected = balancedCheckGeneration
        balancedCheckTask = Task {
            defer { if balancedCheckGeneration == expected { balancedCheckTask = nil } }
            do { try await captureSleep(.milliseconds(750)) } catch { return }
            while !Task.isCancelled, expected == balancedCheckGeneration, let pending = pendingBalancedCheck {
                let state = monitor.currentContextState()
                let lowPower = capturesSparingly(onBattery: state.onBattery, lowPower: state.lowPower)
                let delay = contentLimiter.nextAvailableDelay(now: captureClock(), automatic: true, interval: preferences.samplingOptions.minimumContentInterval,
                    lowPower: lowPower, automaticCapacity: preferences.samplingOptions.contentCapacity, source: samplingWindow)
                if contextBusy || delay > 0 {
                    do { try await captureSleep(.seconds(max(0.1, delay))) } catch { return }
                    continue
                }
                pendingBalancedCheck = nil
                await performBalancedCheck(trigger: pending.trigger, forceImage: pending.image)
            }
        }
    }

    private func performBalancedCheck(trigger: String, forceImage: Bool) async {
        guard isRecording, monitor.isSessionActive, let app = monitor.frontmostApplication,
              effectiveWindowScope.contains(app.bundleID), app.bundleID != Bundle.main.bundleIdentifier,
              !contextBusy, store != nil else { return }
        let state = monitor.currentContextState()
        let lowPower = capturesSparingly(onBattery: state.onBattery, lowPower: state.lowPower)
        guard !monitor.snapshot().thermalBlocked,
              contentLimiter.admit(now: captureClock(), automatic: true, interval: preferences.samplingOptions.minimumContentInterval, lowPower: lowPower, automaticCapacity: preferences.samplingOptions.contentCapacity, source: samplingWindow) else { return }
        let epoch = eventGate.currentEpoch()
        contextBusy = true; samplingDiagnostics.contentReads += 1
        defer { contextBusy = false }
        do {
            let reading = try await monitor.readContext(allowedBundleIDs: effectiveWindowScope, includeText: effectiveTextScope.contains(app.bundleID))
            guard !Task.isCancelled, eventGate.permits(epoch), isRecording,
                  reading.application.pid == monitor.frontmostApplication?.pid,
                  effectiveWindowScope.contains(reading.application.bundleID) else { return }
            let context = scopedContext(reading.context, bundle: reading.application.bundleID)
            guard context.isComposing != true else { return }
            let priorActivity = currentActivity?.id
            let record = assignActivity(DesktopRecord(kind: .context, appName: reading.application.name,
                bundleID: reading.application.bundleID, windowTitle: reading.title, runID: runID,
                trigger: trigger, context: context), windowIdentity: reading.windowIdentity)
            if priorActivity != record.activityID { invalidateCapture(cancelManual: false, reason: L("窗口已变化，未截图")) }
            let signature = DesktopContentSignature.make(record)
            let imageDue = forceImage || periodicImageIsDue(baseInterval: preferences.samplingOptions.imageFallbackInterval)
            let canImage = automaticCaptureAvailable && screenshotsEnabled && desiredPreferences.screenshotEnabled &&
                allowedBundleIDs.contains(app.bundleID) && context.isProtected != true
            if signature == lastContentSignature, !(imageDue && canImage) {
                samplingDiagnostics.unchangedChecks += 1
                return
            }
            lastContentSignature = signature
            enqueue(record)
            if imageDue && canImage && manualCaptureTask == nil {
                // The window trigger already waited before the balanced read.
                scheduleCapture(parent: record, automatic: true, delay: trigger == "window" ? 0 : 0.75)
            }
            else if canImage { setCaptureMessage(record.id, L("尚未到图片采样间隔")) }
        } catch {
            guard !Task.isCancelled, eventGate.permits(epoch), isRecording else { return }
            // Do not call an unreadable page unchanged. Preserve a bounded gap.
            enqueue(DesktopRecord(kind: .gap, appName: app.name, bundleID: app.bundleID,
                detail: L("内容检查未完成：") + error.localizedDescription, runID: runID, trigger: trigger))
        }
    }

    private func handle(_ event: DesktopSystemEvent) {
        guard isRecording else { return }
        if event.trigger == "return", !desiredPreferences.interactionTriggersEnabled || !desiredPreferences.samplingOptions.returnEnabled { return }
        if event.trigger == "scroll", !desiredPreferences.interactionTriggersEnabled || !desiredPreferences.samplingOptions.scrollEnabled { return }
        samplingDiagnostics.signals += 1
        if samplingMode == .balanced, handleBalancedSignal(event) { return }
        switch event.kind {
        case .blocked:
            resetSamplingActivity(); currentActivity = nil
            periodicSampler.stop(); eventGate.invalidate(); _ = eventGate.enable()
            invalidateCapture(); gate.invalidate(); statusText = L("系统暂停"); statusDetail = event.detail
            enqueue(DesktopRecord(kind: .gap, detail: event.detail, runID: runID, trigger: "system", context: stateContext()))
        case .resumed:
            if monitor.isSessionActive { _ = gate.enable(); statusText = L("正在记录"); statusDetail = L("系统会话已恢复，截图继续遵守当前模式与允许范围。") }
            enqueue(DesktopRecord(kind: .system, detail: event.detail, runID: runID, trigger: "system", context: stateContext())); updateSystem(); restartPeriodicSampling()
        case .application, .window, .context:
            guard let app = event.application, monitor.isSessionActive else { return }
            guard app.bundleID != Bundle.main.bundleIdentifier, !preferences.excludedBundleIDs.union(desiredPreferences.excludedBundleIDs).contains(app.bundleID) else {
                currentActivity = nil; invalidateCapture(cancelManual: false, reason: captureSwitchReason(to: app)); return
            }
            let title = effectiveWindowScope.contains(app.bundleID) ? event.title : ""
            let context = scopedContext(event.context, bundle: app.bundleID)
            guard context.isComposing != true else { return }
            if event.trigger == "settled" {
                if let pendingCapture, pendingCapture.bundleID == app.bundleID,
                   pendingCapture.windowTitle.isEmpty || pendingCapture.windowTitle == title {
                    pendingCaptureRevision = monitor.contextRevision
                }
                return
            }
            let kind: DesktopRecordKind = event.kind == .application ? .application : (event.kind == .window ? .window : .context)
            let oldActivity = currentActivity?.id
            let record = assignActivity(DesktopRecord(kind: kind, appName: app.name, bundleID: app.bundleID, windowTitle: title, detail: event.detail, runID: runID, trigger: event.trigger ?? (event.kind == .application ? "application" : "window"), context: context), forceNew: event.kind == .application, windowIdentity: event.windowIdentity)
            if oldActivity != record.activityID || context.isProtected == true {
                invalidateCapture(cancelManual: false, reason: event.kind == .application ? captureSwitchReason(to: app) : L("窗口或保护状态已变化，未截图"))
            }
            enqueue(record)
            let sourceTrigger = event.kind == .application || event.kind == .window || event.trigger == "window" || oldActivity != record.activityID
            if (!sourceTrigger || preferences.samplingOptions.windowTriggersEnabled), context.isProtected != true, automaticCaptureAvailable, manualCaptureTask == nil, screenshotsEnabled, desiredPreferences.screenshotEnabled, allowedBundleIDs.contains(app.bundleID) { scheduleCapture(parent: record, automatic: true) }
        case .system:
            updateSystem(); enqueue(DesktopRecord(kind: .system, detail: event.detail, runID: runID, trigger: "system", context: stateContext())); restartPeriodicSampling()
        }
    }

    private func assignActivity(_ original: DesktopRecord, forceNew: Bool = false, windowIdentity: UInt? = nil) -> DesktopRecord {
        var record = original
        let protected = record.context?.isProtected == true
        let pid = monitor.frontmostApplication.flatMap { $0.bundleID == record.bundleID ? $0.pid : nil }
        let differentWindow = windowIdentity != nil && currentActivity?.windowIdentity != nil && windowIdentity != currentActivity?.windowIdentity
        if forceNew || differentWindow || currentActivity?.pid != pid || currentActivity == nil || currentActivity?.bundle != record.bundleID || currentActivity?.protected != protected ||
            (!(currentActivity?.title ?? "").isEmpty && !record.windowTitle.isEmpty && currentActivity?.title != record.windowTitle) {
            currentActivity = Activity(id: record.id, bundle: record.bundleID, title: record.windowTitle, protected: protected, pid: pid, windowIdentity: windowIdentity)
        } else if !record.windowTitle.isEmpty { currentActivity?.title = record.windowTitle }
        if let windowIdentity { currentActivity?.windowIdentity = windowIdentity }
        if lastSignatureActivityID != currentActivity?.id {
            lastContentSignature = nil; lastImageAttemptClock = -Double.infinity
            lastSignatureActivityID = currentActivity?.id
        }
        record.activityID = currentActivity?.id
        return record
    }

    private func enqueue(_ record: DesktopRecord) {
        let published = publicationRecord(record)
        queueWrite(PendingEvent(record: published, epoch: record.kind == .marker ? nil : eventGate.currentEpoch()))
        scheduleNativeProof(published)
        for proof in sourceConfirmation.confirmations(for: published) {
            queueWrite(PendingEvent(record: publicationRecord(proof), epoch: eventGate.currentEpoch()))
        }
    }
    private func scheduleNativeProof(_ record: DesktopRecord) {
        guard isRecording, !isDemo, DesktopNativeSendProof.session(for: record) != nil,
              let e = record.context?.recordEvidence, e.input_status == "captured",
              let value = e.input_value, !value.isEmpty, e.confirmation == nil, let field = e.field else { return }
        let key = record.bundleID + "|" + e.object.key + "|" + field
        if nativeDrafts[key]?.context?.recordEvidence?.input_value != value { nativeDrafts[key] = record }
        if nativeDrafts.count > 16 { nativeDrafts.removeValue(forKey: nativeDrafts.min { $0.value.timestamp < $1.value.timestamp }!.key) }
        guard nativeProofTask == nil else { return }
        let epoch = eventGate.currentEpoch()
        nativeProofTask = Task { [weak self] in
            guard let self else { return }
            defer { if self.eventGate.permits(epoch) { self.nativeProofTask = nil } }
            for _ in 0..<15 {
                do {
                    try await Task.sleep(for: .seconds(2))
                    guard !Task.isCancelled, self.isRecording, self.eventGate.permits(epoch), let channel = self.core.channel else { return }
                    let now = Date()
                    self.nativeDrafts = self.nativeDrafts.filter { now.timeIntervalSince($0.value.timestamp) <= 30 }
                    guard let since = self.nativeDrafts.values.map(\.timestamp).min() else { return }
                    let sessions = Set(self.nativeDrafts.values.compactMap { DesktopNativeSendProof.session(for: $0) })
                    guard self.nativeExamined.count < 4096 else { return }
                    let messages = try await DesktopNativeSendProof.messages(channel: channel, since: since, now: now, sessions: sessions, excluding: self.nativeExamined)
                    self.nativeExamined.formUnion(messages.map(\.reference))
                    guard !Task.isCancelled, self.isRecording, self.eventGate.permits(epoch) else { return }
                    let matches = self.nativeDrafts.compactMap { key, draft -> (String, DesktopRecord)? in
                        DesktopNativeSendProof.confirmation(draft: draft, messages: messages, now: now).map { (key, $0) }
                    }
                    for (key, proof) in matches {
                        guard let id = proof.context?.recordEvidence?.confirmation?.message_id,
                              matches.filter({ $0.1.context?.recordEvidence?.confirmation?.message_id == id }).count == 1,
                              !self.nativeConfirmed.contains(id), self.nativeConfirmed.count < 4096 else { continue }
                        self.nativeConfirmed.insert(id); self.nativeDrafts.removeValue(forKey: key)
                        self.queueWrite(PendingEvent(record: self.publicationRecord(proof), epoch: epoch))
                    }
                } catch is CancellationError { return }
                catch { /* Keep the bounded draft for the next index retry. */ }
            }
            self.nativeDrafts.removeAll()
        }
    }
    private func queueWrite(_ item: PendingEvent) {
        guard pendingEvents.count < 256 else {
            droppedEvents += 1; pauseRecording(); error = L("写入队列达到上限，采集已停止。请检查磁盘后恢复。"); return
        }
        pendingEvents.append(item)
        guard !draining else { return }; draining = true
        Task {
            while !pendingEvents.isEmpty {
                let batch = Array(pendingEvents.prefix(32)); pendingEvents.removeFirst(batch.count)
                do {
                    for item in batch {
                        do {
                            try await store?.append(item.record, gate: item.epoch == nil ? nil : eventGate, epoch: item.epoch ?? 0)
                            rememberRecord(item.record)
                            samplingDiagnostics.snapshotsSaved += 1; lastSaved = Date()
                        } catch is CaptureAdmissionError { /* invalidated writes cannot publish */ }
                    }
                } catch { droppedEvents += batch.count; pauseRecording(); fail(error); pendingEvents.removeAll(); break }
            }
            draining = false
        }
    }

    func captureNow() {
        guard !isDemo else { notice = L("演示空间仅使用合成记录，不访问屏幕。"); return }
        guard isRecording else { error = L("请先开始记录；手动截图也遵守暂停和应用范围。"); return }
        guard screenshotsEnabled, desiredPreferences.screenshotEnabled else { error = L("请在记录设置中开启允许范围内的截图。"); return }
        notice = L("3 秒后截取当前允许的窗口，请切换到目标应用。")
        manualCaptureTask?.cancel()
        manualCaptureGeneration &+= 1
        let manualGeneration = manualCaptureGeneration
        manualCaptureTask = Task {
            defer { if manualCaptureGeneration == manualGeneration { manualCaptureTask = nil; notice = nil } }
            try? await Task.sleep(for: .seconds(3))
            guard !Task.isCancelled else { return }
            await performCapture(parent: nil, automatic: false)
        }
    }
    private enum CaptureDisposition { case complete, retry(Double) }

    func captureFeedback(for record: DesktopRecord) -> String? {
        if let message = captureMessages[record.id] { return message }
        if record.context?.isProtected == true { return L("受保护，未采集截图") }
        if record.context?.isComposing == true { return L("输入法尚未确认，未截图") }
        return selectedEvent?.issues.last { $0.parentID == record.id }?.detail
    }
    private func setCaptureMessage(_ id: String, _ message: String?) {
        guard captureMessages[id] != message else { return }
        captureMessages[id] = message
        if captureMessages.count > 512, let first = captureMessages.keys.first { captureMessages[first] = nil }
    }
    private func scheduleCapture(parent: DesktopRecord, automatic: Bool, delay: Double = 0.75) {
        if let previous = pendingCapture, previous.id != parent.id {
            setCaptureMessage(previous.id, L("已合并到后续快照"))
        }
        pendingCapture = parent
        pendingCaptureRevision = monitor.contextRevision
        pendingCaptureReadyAt = captureClock() + delay
        setCaptureMessage(parent.id, L("等待内容稳定后截图"))
        // Keep one pump. A later notification replaces only the pending value,
        // never cancels a valid native request already in progress.
        guard scheduledCapture == nil else { return }
        let generation = capturePumpGeneration
        scheduledCapture = Task {
            defer { if capturePumpGeneration == generation { scheduledCapture = nil } }
            while !Task.isCancelled, capturePumpGeneration == generation, let parent = pendingCapture {
                guard isRecording, monitor.isSessionActive else { return }
                let state = monitor.currentContextState()
                var delay = max(0, pendingCaptureReadyAt - captureClock())
                delay = max(delay, limiter.nextAvailableDelay(now: captureClock(), automatic: automatic,
                    interval: preferences.minimumCaptureInterval, lowPower: capturesSparingly(onBattery: state.onBattery, lowPower: state.lowPower),
                    automaticCapacity: preferences.samplingOptions.imageCapacity, source: samplingWindow))
                let cooldownDelay = cooldownUntil.timeIntervalSinceNow
                delay = max(delay, cooldownDelay)
                if !automatic, samplingMode == .balanced, !monitor.isContentSettling, pendingCaptureRevision != monitor.contextRevision {
                    setCaptureMessage(parent.id, L("内容已变化，等待重新核对"))
                    pendingCapture = nil
                    requestBalancedCheck(trigger: "refresh", image: true)
                    return
                }
                if !automatic && (monitor.isContentSettling || pendingCaptureRevision != monitor.contextRevision) {
                    setCaptureMessage(parent.id, L("正在输入，等待文字稳定"))
                    delay = max(delay, 1.5)
                } else if cooldownDelay > 0 {
                    setCaptureMessage(parent.id, L("系统截图请求连续失败，暂缓重试"))
                } else if delay > 0.8 {
                    setCaptureMessage(parent.id, L("已达截图频率限制，仍在此窗口时补图"))
                }
                if captureBusy { delay = max(delay, 0.1) }
                if delay > 0 {
                    do { try await captureSleep(.seconds(delay)) } catch { return }
                    continue
                }
                let result = await performCapture(parent: parent, automatic: automatic)
                guard !Task.isCancelled, capturePumpGeneration == generation else { return }
                switch result {
                case .complete:
                    if pendingCapture?.id == parent.id { pendingCapture = nil }
                case .retry(let seconds):
                    pendingCaptureReadyAt = max(pendingCaptureReadyAt, captureClock() + seconds)
                }
            }
        }
    }
    @discardableResult private func performCapture(parent: DesktopRecord?, automatic: Bool) async -> CaptureDisposition {
        guard isRecording, preferences.screenshotEnabled, desiredPreferences.screenshotEnabled,
              !automatic || automaticCaptureAvailable, monitor.isSessionActive else { return .complete }
        if automatic, let parent, parent.activityID != currentActivity?.id { return .complete }
        if captureBusy { return .retry(0.1) }
        if Date() < cooldownUntil {
            if parent == nil { error = L("系统截图请求连续失败，请稍后重试。") }
            return .retry(cooldownUntil.timeIntervalSinceNow)
        }
        if monitor.isContentSettling, !automatic {
            if let parent { setCaptureMessage(parent.id, L("正在输入，等待文字稳定")) }
            else { error = L("窗口内容仍在变化，请等它稳定后再截图。") }
            return .retry(1.5)
        }
        let state = monitor.snapshot()
        guard !state.thermalBlocked else {
            if let parent { setCaptureMessage(parent.id, L("温度较高，等待恢复后截图")) }
            else { error = L("系统温度较高，请稍后再截图。") }
            return .retry(60)
        }
        let source = automatic ? samplingWindow : nil
        let now = captureClock()
        let lowPower = capturesSparingly(onBattery: state.onBattery, lowPower: state.lowPower)
        let interval = preferences.minimumCaptureInterval
        let capacity = preferences.samplingOptions.imageCapacity
        guard limiter.admit(now: now, automatic: automatic, interval: interval, lowPower: lowPower, automaticCapacity: capacity, source: source) else {
            let delay = limiter.nextAvailableDelay(now: now, automatic: automatic, interval: interval, lowPower: lowPower, automaticCapacity: capacity, source: source)
            if let parent { setCaptureMessage(parent.id, L("已达截图频率限制，仍在此窗口时补图")) }
            else { error = L("截图频率已达上限，请稍后重试。") }
            return .retry(max(0.1, delay))
        }
        let epoch = gate.currentEpoch(); guard gate.permits(epoch) else { return .complete }
        let contextRevision = monitor.contextRevision
        captureBusy = true; captureAttempts += 1; samplingDiagnostics.imageAttempts += 1; lastImageAttemptClock = captureClock()
        if let parent { setCaptureMessage(parent.id, L("正在截图")) }
        defer { captureBusy = false }
        do {
            guard let store else { return .complete }
            let free = try? URL(fileURLWithPath: dataLocation).resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey]).volumeAvailableCapacityForImportantUsage
            guard free == nil || free! >= 2 * 1024 * 1024 * 1024 else { throw DesktopStoreError.quotaExceeded }
            let effectiveAllowed = preferences.allowedBundleIDs.intersection(desiredPreferences.allowedBundleIDs).subtracting(preferences.excludedBundleIDs.union(desiredPreferences.excludedBundleIDs))
            let compatibility = preferences.windowCompatibilityBundleIDs
                .intersection(desiredPreferences.windowCompatibilityBundleIDs).intersection(effectiveAllowed)
            let axScope = preferences.contentOptions.readableTextEnabled && desiredPreferences.contentOptions.readableTextEnabled
                ? effectiveTextScope.intersection(effectiveAllowed) : []
            // Changed content/manual frames take the original direct encoder. Stable automatic frames may reuse it.
            let reusable = source.flatMap { recentImages[$0] }.map { $0.contentSignature == parent.map(DesktopContentSignature.make) } ?? false
            capture.setImageEncodingReuseEnabled(reusable)
            capture.setRawFrameDelivery(recognition.wantsRawFrames)
            let frame = try await capture.captureFrontmost(allowedBundleIDs: effectiveAllowed,
                windowCompatibilityBundleIDs: compatibility, axTextBundleIDs: axScope,
                includeAXFocus: preferences.contentOptions.focusEnabled && desiredPreferences.contentOptions.focusEnabled)
            guard !Task.isCancelled, gate.permits(epoch), isRecording else { return .complete }
            if !automatic { guard contextRevision == monitor.contextRevision, !monitor.isContentSettling else { throw DesktopCaptureError.contentChanging } }
            if automatic, let parent, parent.activityID != currentActivity?.id { return .complete }
            if let parent, parent.bundleID != frame.application.bundleID { return .complete }
            if let parent, !parent.windowTitle.isEmpty, !frame.title.isEmpty, parent.windowTitle != frame.title { throw DesktopCaptureError.focusChanged }
            if automatic, let actual = frame.windowIdentity, let expected = currentActivity?.windowIdentity, actual != expected { throw DesktopCaptureError.focusChanged }
            let digest = source.map { _ in SHA256.hash(data: frame.data) }
            if automatic, let source, let digest, var previous = recentImages[source], previous.digest == digest, previous.contentSignature == parent.map(DesktopContentSignature.make) {
                if parent?.trigger == "timer" { previous.repeated = min(3, previous.repeated + 1) }
                recentImages[source] = previous
                consecutiveFailures = 0
                if let parent { setCaptureMessage(parent.id, L("画面与上一张相同，已略过")) }
                return .complete
            }
            var imageContext = parent?.context ?? DesktopContextSnapshot()
            if contextRevision != monitor.contextRevision || monitor.isContentSettling {
                imageContext.readableText = nil; imageContext.selectedText = nil; imageContext.recordEvidence = nil
            }
            imageContext.axEvidence = frame.axEvidence
            let record = DesktopRecord(timestamp: frame.capturedAt, kind: .screenshot, appName: frame.application.name, bundleID: frame.application.bundleID, windowTitle: frame.title, detail: automatic ? (parent?.trigger == "timer" ? L("定时触发") : L("事件触发")) : L("用户手动触发"), imageWidth: frame.width, imageHeight: frame.height, parentID: parent?.id, runID: runID, trigger: automatic ? parent?.trigger : "manual", context: scopedContext(imageContext, bundle: frame.application.bundleID), activityID: parent?.activityID)
            // The configured storage limit counts the library's desktop originals and what still waits in the outbox.
            var used = statistics.storageBytes + (await store.outbox.bytes)
            if used + Int64(frame.data.count) > preferences.maximumBytes {
                used = await freeScreenshotSpace(store: store, limit: preferences.maximumBytes)
            }
            guard used + Int64(frame.data.count) <= preferences.maximumBytes else { storageLimitRefused = true; throw DesktopStoreError.quotaExceeded }
            let savedImage = try await store.storeScreenshot(record: publicationRecord(record), data: frame.data, gate: gate, epoch: epoch)
            if let source, let digest {
                if recentImages[source] == nil, recentImages.count >= 128, let evicted = recentImages.keys.first {
                    recentImages.removeValue(forKey: evicted)
                }
                recentImages[source] = RecentImage(digest: digest, contentSignature: parent.map(DesktopContentSignature.make))
            }
            rememberRecord(savedImage)
            if let raw = frame.frame { recognition.recognizeRawFrame(raw, screenshotID: savedImage.id) }
            recognition.wake()
            samplingDiagnostics.imageSaved += 1
            consecutiveFailures = 0; lastSaved = Date(); statusDetail = L("截图已保存"); storageLimitRefused = false
            if let parent { setCaptureMessage(parent.id, nil) }
            return .complete
        } catch is CaptureAdmissionError { return .complete }
        catch {
            guard gate.permits(epoch), isRecording else { return .complete }
            if let transient = error as? DesktopCaptureError, automatic,
               transient == .contentChanging || transient == .busy || (transient == .focusChanged && (monitor.isContentSettling || contextRevision != monitor.contextRevision)) {
                if let parent { setCaptureMessage(parent.id, L("内容仍在变化，等待稳定后补图")) }
                return .retry(1.5)
            }
            captureFailures += 1
            if (error as? DesktopCaptureError)?.shouldBackOffCapture == true {
                consecutiveFailures += 1
                if consecutiveFailures >= 3 { cooldownUntil = Date().addingTimeInterval(60) }
            } else {
                consecutiveFailures = 0
            }
            let message = L("截图未保存：") + error.localizedDescription
            if let parent { setCaptureMessage(parent.id, message) }
            enqueue(DesktopRecord(kind: .gap, detail: message, parentID: parent?.id, runID: runID, activityID: parent?.activityID))
            if !automatic { self.error = error.localizedDescription }
            return .complete
        }
    }

    func addMarker() {
        guard storageReady else { error = L("本地账本尚未准备好，请检查启动错误后重试。"); return }
        enqueue(DesktopRecord(kind: .marker, windowTitle: L("标记"), detail: L("用户手动标记"), runID: runID))
    }
    func dismissError() { error = nil; notice = nil }
    func deleteSelected() {
        guard let selectedID else { return }; recognition.cancel(); invalidateContentPolicy(); imageLocations.removeAll(); recentRecords.removeAll(); records.removeAll()
        let anchor = timelineEvents.first { $0.id == selectedID }?.record ?? DesktopRecord(id: selectedID, kind: .marker)
        Task { do { try await store?.deleteActivity(anchor); recentImages.removeAll(); self.selectedID = nil } catch { fail(error) } }
    }
    func exportSelection() {
        let selected = selectedEvent.map { [$0] } ?? timelineEvents
        guard !selected.isEmpty else { notice = L("当前范围没有可导出的记录。"); return }
        let panel = NSOpenPanel(); panel.canChooseFiles = false; panel.canChooseDirectories = true; panel.canCreateDirectories = true; panel.prompt = L("导出到此目录"); panel.message = L("将在此目录创建一个独立可读的记录包。")
        guard panel.runModal() == .OK, let folder = panel.url else { return }
        let destination = folder.appendingPathComponent("Candid-" + UUID().uuidString.prefix(8), isDirectory: true)
        Task {
            do {
                guard let store else { return }
                try await DesktopExport.write(events: selected, store: store, to: destination)
                NSWorkspace.shared.activateFileViewerSelecting([destination])
            } catch { fail(error) }
        }
    }
    func imageURL(for record: DesktopRecord) -> URL? { imageLocations[record.id] }

    /// Read-only lookups for thumbnails; they never change selection or paging.
    func screenshotRecords(forEventID id: String) async -> [DesktopRecord] {
        (try? await store?.activity(id: id, limit: 120))??.screenshots ?? []
    }
    func eventDetail(id: String) async -> DesktopTimelineEvent? {
        (try? await store?.activity(id: id, limit: 120)) ?? nil
    }
    /// The original's file in the timeline library (checked to exist; its hash was checked when it was stored).
    func verifiedImageURL(hash: String) async -> URL? {
        await store?.imageURL(hash: hash)
    }

    func loadEarlierObservations() {
        guard let event = selectedEvent, event.hasEarlier, let boundary = event.observations.first?.id else { return }
        loadObservationPage(eventID: event.id, before: boundary, after: nil)
    }
    func loadLaterObservations() {
        guard let event = selectedEvent, event.hasLater, let boundary = event.observations.last?.id else { return }
        loadObservationPage(eventID: event.id, before: nil, after: boundary)
    }
    private func loadObservationPage(eventID: String, before: String?, after: String?) {
        guard !observationPageLoading, let store else { return }
        observationPageLoading = true; queryGeneration &+= 1
        observationPageGeneration &+= 1; let generation = observationPageGeneration
        observationPageTask = Task {
            defer { if observationPageGeneration == generation { observationPageLoading = false; observationPageTask = nil } }
            do {
                guard let page = try await store.activity(id: eventID, before: before, after: after),
                      !Task.isCancelled, observationPageGeneration == generation, selectedID == eventID,
                      !page.observations.isEmpty else { return }
                selectedDetail = page; updateLoadedRecords()
                selectedObservationID = before != nil ? page.observations.last?.id : page.observations.first?.id
                loadSelectedImage()
            } catch { fail(error) }
        }
    }
    private func loadSelectedImage() {
        guard let record = selectedImageRecord, let hash = record.imageHash else {
            selectedImageGeneration &+= 1; selectedImageTask?.cancel(); selectedImageTask = nil
            selectedImageRequestID = nil; imageLoading = false; imageLocations.removeAll(); return
        }
        let id = record.id
        guard selectedImageRequestID != id else { return }
        selectedImageTask?.cancel(); selectedImageGeneration &+= 1
        let generation = selectedImageGeneration
        selectedImageRequestID = id; imageLoading = true; imageLocations.removeAll()
        selectedImageTask = Task {
            defer { if selectedImageGeneration == generation { imageLoading = false; selectedImageTask = nil } }
            do {
                let url = await store?.imageURL(hash: hash)
                guard !Task.isCancelled, selectedImageGeneration == generation, selectedImageRecord?.id == id else { return }
                guard let url else { throw DesktopStoreError.missingImage }
                imageLocations = [id: url]
            } catch {
                guard !Task.isCancelled, selectedImageGeneration == generation, selectedImageRecord?.id == id else { return }
                self.error = error.localizedDescription
            }
        }
    }
    private func rememberRecord(_ record: DesktopRecord) {
        recentRecords[record.id] = record
        if recentRecords.count > 1000, let oldest = recentRecords.values.min(by: { $0.timestamp < $1.timestamp }) { recentRecords[oldest.id] = nil }
        updateLoadedRecords()
    }
    private func updateLoadedRecords() {
        guard timelineVisible else { return }
        let merged = Dictionary((Array(recentRecords.values) + timelineEvents.flatMap(\.allRecords) + (selectedDetail?.allRecords ?? [])).map { ($0.id, $0) }, uniquingKeysWith: { _, latest in latest })
        let loaded = Array(merged.values.sorted { $0.timestamp > $1.timestamp }.prefix(1000))
        if loaded != records { records = loaded }
    }
    /// New records reached the core: refresh at most once a second while deliveries keep coming (a debounce would
    /// starve under steady capture), and only the counts while the timeline is hidden.
    private func deliveredReload() {
        guard !deliveryReloadPending else { return }
        deliveryReloadPending = true
        Task {
            try? await Task.sleep(for: .seconds(1))
            deliveryReloadPending = false
            if timelineVisible { await reload() }
            else { await refreshStatistics(maxAge: 60) }
        }
    }
    /// The library counts. While the timeline is hidden they only feed the screenshot storage limit and the settings
    /// pages, so deliveries refresh them at most once a minute (each refresh counts the whole desktop library);
    /// a settings page on screen asks with a shorter age.
    func refreshStatistics(maxAge: TimeInterval = 5) async {
        if let last = statisticsLoaded, Date().timeIntervalSince(last) < maxAge { return }
        statisticsLoaded = Date()
        if let stats = try? await store?.statistics(), stats != statistics { statistics = stats }
    }
    /// Only the selected activity's detail; the list and the timeline drawn from it stay as they are.
    private func scheduleDetail() {
        detailTask?.cancel()
        guard let id = selectedID else { selectedDetail = nil; updateLoadedRecords(); loadSelectedImage(); return }
        detailTask = Task {
            try? await Task.sleep(for: .milliseconds(60))
            guard !Task.isCancelled, let store else { return }
            let detail = try? await store.activity(id: id, around: selectedObservationID)
            guard !Task.isCancelled, selectedID == id else { return }
            selectedDetail = detail; updateLoadedRecords(); loadSelectedImage()
        }
    }
    private func scheduleReload() {
        reloadTask?.cancel(); reloadTask = Task { try? await Task.sleep(for: .milliseconds(180)); guard !Task.isCancelled else { return }; await reload() }
    }
    private func reload() async {
        guard timelineVisible, let store, core.channel != nil else { return }
        while observationPageLoading, let task = observationPageTask { await task.value }
        queryGeneration &+= 1; let expected = queryGeneration
        let day = Calendar.current.startOfDay(for: selectedDate), end = Calendar.current.date(byAdding: .day, value: 1, to: day)!
        do {
            samplingDiagnostics.listReloads += 1
            let loaded = try await store.activities(from: day, to: end, query: searchText)
            let items = loaded.0
            let stats = loaded.1
            let selection = items.contains { $0.id == selectedID } ? selectedID : items.first?.id
            var detail: DesktopTimelineEvent?
            if let id = selection { detail = try await store.activity(id: id, around: id == selectedID ? selectedObservationID : nil) }
            guard timelineVisible, queryGeneration == expected else { return }
            applyingList = true
            // Unchanged lists are not reassigned, so views drawn from them do not rebuild on every delivery.
            if items != timelineEvents { timelineEvents = items }
            if detail != selectedDetail { selectedDetail = detail }
            updateLoadedRecords(); statisticsLoaded = Date(); if stats != statistics { statistics = stats }
            selectedID = selection
            applyingList = false
            loadSelectedImage()
        } catch { applyingList = false; fail(error) }
    }
    private func fail(_ failure: Error) { error = failure.localizedDescription }

    /// The demo's synthetic day goes into the throwaway core through the same outbox as real capture.
    private func seedDemo() async throws {
        guard let store else { return }
        for item in DesktopDemoContent.items(now: Date()) {
            if let image = item.image { _ = try await store.storeScreenshot(record: item.record, data: image) }
            else { try await store.append(item.record) }
        }
    }
}
