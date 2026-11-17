import AppKit
import CandidLocalization
import ApplicationServices
import DesktopCore
import Foundation
import Darwin
import IOKit.ps

/// System/AX observer with opt-in passive interaction hints. Never captures images
/// or requests permissions implicitly.
@MainActor
public final class DesktopSystemMonitor {
    private var installedApplications: [DesktopApplication] = []
    private var catalogRefreshing = false
    private var manuallyAdded: [DesktopApplication] = []
    private var workspaceObservers: [NSObjectProtocol] = []
    private var defaultObservers: [NSObjectProtocol] = []
    private var distributedObservers: [NSObjectProtocol] = []
    private var onEvent: (@MainActor (DesktopSystemEvent) -> Void)?
    private var focusRefreshTask: Task<Void, Never>?
    private var pendingFocus: DesktopApplication?
    private var focusQuietPeriod = DesktopAXQuietPeriod()
    private var samplingActivity = DesktopSamplingActivity()
    private var balancedSampling = false
    private var contentOptions = DesktopContentOptions()
    private var interactionTriggersRequested = false
    private var returnEnabled = true
    private var scrollEnabled = true
    private let interactionTap = DesktopInteractionTap()
    private var textContentScope: Set<String> = []
    private var contextReadInFlight = false
    private var textReadsInFlight = 0
    public private(set) var contextRevision: UInt64 = 0
    public var isContentSettling: Bool { textReadsInFlight > 0 || (balancedSampling ? samplingActivity.isSettling(at: ProcessInfo.processInfo.systemUptime) : focusQuietPeriod.isTextSettling(at: ProcessInfo.processInfo.systemUptime)) }
    private var focusOwnerID = UUID()
    private var focusGeneration: UInt64 = 0
    private var powerSource: CFRunLoopSource?
    private static weak var activeAXMonitor: DesktopSystemMonitor?
    private static weak var activePowerMonitor: DesktopSystemMonitor?
    private var lastFocus: String = ""
    private var blockedReasons: Set<String> = []
    private var windowContentScope: Set<String> = []
    public private(set) var isSessionActive = false
    public var frontmostApplication: DesktopApplication? { DesktopFocusReader.application(NSWorkspace.shared.frontmostApplication) }

    public init() {}

    public var inputMonitoringAuthorized: Bool { CGPreflightListenEventAccess() }
    public var interactionTriggersActive: Bool { interactionTriggersRequested && isSessionActive && inputMonitoringAuthorized && interactionTap.isActive }
    public var interactionTriggerStatus: String {
        if interactionTriggersRequested && !inputMonitoringAuthorized { return "permission_denied" }
        if interactionTap.status == "active", !interactionTap.isActive { return "system_disabled" }
        return interactionTap.status
    }

    /// Changing requested content invalidates in-flight AX reads before a
    /// replacement observer is installed. Protection checks remain mandatory.
    public func setContentOptions(_ options: DesktopContentOptions) {
        guard contentOptions != options else { return }
        contentOptions = options; contextRevision &+= 1
        let wait = options.maximumChangeWait ?? 10
        focusQuietPeriod.maximumWait = wait.isFinite ? min(60, max(5, wait)) : 10
        removeAXObserver(); lastFocus = ""
        if isSessionActive, let app = frontmostApplication, windowContentScope.contains(app.bundleID) { requestFocusRefresh(app) }
    }

    /// Selected by Session; false preserves the detailed comparison behavior.
    public func setSamplingPolicy(balanced: Bool) {
        guard balancedSampling != balanced else { return }
        balancedSampling = balanced; contextRevision &+= 1
        removeAXObserver(); lastFocus = ""
        if isSessionActive, let app = frontmostApplication, windowContentScope.contains(app.bundleID) { requestFocusRefresh(app) }
    }

    public func setInteractionTriggerSelection(returnEnabled: Bool, scrollEnabled: Bool) {
        self.returnEnabled = returnEnabled; self.scrollEnabled = scrollEnabled
    }

    @discardableResult public func setInteractionTriggersEnabled(_ enabled: Bool) -> Bool {
        interactionTriggersRequested = enabled
        interactionTap.stop()
        guard enabled else { return true }
        guard inputMonitoringAuthorized else { return false }
        return !isSessionActive || startInteractionTap()
    }

    /// Explicit UI action only. No start/configuration path invokes this method.
    @discardableResult public func requestInputMonitoringPermission() -> Bool { CGRequestListenEventAccess() }

    private func startInteractionTap() -> Bool {
        interactionTap.start(returnEnabled: returnEnabled, scrollEnabled: scrollEnabled, onSignal: { [weak self] trigger in self?.interactionSignal(trigger) }, onFailure: { [weak self] status in
            self?.onEvent?(.init(kind: .system, detail: status == "permission_denied" ? L("输入监控权限已失效，回车与滚动触发已停止") : L("系统已停止回车与滚动触发，请在设置中重新启用"), trigger: "system"))
        })
    }
    private func interactionSignal(_ trigger: String) {
        guard interactionTriggersActive, let app = frontmostApplication,
              windowContentScope.contains(app.bundleID), app.bundleID != Bundle.main.bundleIdentifier else { return }
        onEvent?(.init(kind: .context, application: app, trigger: trigger))
        requestFocusRefresh(app, trigger: trigger)
    }

    // Internal scheduling requires actual power/session state even when the
    // user opts out of persisting that state. Session sanitizes stored output.
    public func currentContextState() -> DesktopContextSnapshot { resourceContext(.init(), includeSystemState: true) }

    public func setWindowContentScope(allowedBundleIDs: Set<String>) {
        guard windowContentScope != allowedBundleIDs else { return }
        contextRevision &+= 1
        windowContentScope = allowedBundleIDs
        removeAXObserver(); lastFocus = ""
        if isSessionActive, let application = frontmostApplication, windowContentScope.contains(application.bundleID) {
            requestFocusRefresh(application)
        }
    }

    public func setTextContentScope(allowedBundleIDs: Set<String>) {
        guard textContentScope != allowedBundleIDs else { return }
        contextRevision &+= 1
        textContentScope = allowedBundleIDs
        removeAXObserver(); lastFocus = ""
        if isSessionActive, let app = frontmostApplication, windowContentScope.contains(app.bundleID) {
            requestFocusRefresh(app, trigger: "focus")
        }
    }

    public func readContext(allowedBundleIDs: Set<String>, includeText: Bool) async throws -> DesktopContextReading {
        guard !contextReadInFlight else { throw DesktopCaptureError.busy }
        guard isSessionActive, DesktopSessionSafety.canCapture else { throw DesktopCaptureError.inactiveSession }
        guard let app = frontmostApplication else { throw DesktopCaptureError.noFocusedWindow }
        guard allowedBundleIDs.contains(app.bundleID), windowContentScope.contains(app.bundleID) else { throw DesktopCaptureError.excludedApplication }
        let generation = focusGeneration, owner = focusOwnerID
        let balanced = balancedSampling, options = contentOptions
        let textAllowed = includeText && textContentScope.contains(app.bundleID) && (options.readableTextEnabled || options.selectedTextEnabled || options.inputValueBundleIDs?.contains(app.bundleID) == true)
        contextReadInFlight = true
        if textAllowed { textReadsInFlight += 1 }
        defer { contextReadInFlight = false; if textAllowed { textReadsInFlight -= 1 } }
        let focus = try await DesktopAXWorker.shared.read(application: app, includeText: textAllowed, contentOptions: options, wakeDormantWeb: true)
        try Task.checkCancellation()
        // Balanced snapshots are allowed during ongoing edits. Recheck actual
        // source and protection rather than rejecting every cursor/content AX
        // revision; a real window change still invalidates the result.
        if balanced {
            let verified = try await DesktopAXWorker.shared.read(application: app, includeText: false, minimalMetadata: true, contentOptions: options)
            try Task.checkCancellation()
            guard DesktopContextSourcePolicy.accepts(expectedWindow: focus.windowIdentity, actualWindow: verified.windowIdentity,
                                                     expectedPID: app.pid, actualPID: frontmostApplication?.pid,
                                                     ownerUnchanged: owner == focusOwnerID, generationUnchanged: generation == focusGeneration,
                                                     balanced: true, allowed: windowContentScope.contains(app.bundleID) && (!textAllowed || textContentScope.contains(app.bundleID)),
                                                     active: isSessionActive && DesktopSessionSafety.canCapture) else { throw DesktopCaptureError.focusChanged }
            guard verified.context.isProtected != true else { throw DesktopCaptureError.protectedContent }
            if verified.context.isComposing == true {
                return DesktopContextReading(application: app, title: verified.title, context: resourceContext(verified.context), windowIdentity: verified.windowIdentity)
            }
        }
        guard (balanced || generation == focusGeneration), owner == focusOwnerID, isSessionActive,
              DesktopSessionSafety.canCapture, frontmostApplication?.pid == app.pid,
              windowContentScope.contains(app.bundleID), !textAllowed || textContentScope.contains(app.bundleID) else { throw DesktopCaptureError.cancelled }
        var context = resourceContext(focus.context)
        if includeText, !textContentScope.contains(app.bundleID), context.isProtected != true { context.textStatus = "not_allowed" }
        return DesktopContextReading(application: app, title: focus.title, context: context, windowIdentity: focus.windowIdentity)
    }

    private func resourceContext(_ value: DesktopContextSnapshot, includeSystemState: Bool? = nil) -> DesktopContextSnapshot {
        var result = value
        guard includeSystemState ?? contentOptions.systemStateEnabled else {
            result.sessionActive = nil; result.onBattery = nil; result.lowPower = nil
            result.thermalState = nil; result.idleSeconds = nil
            return result
        }
        let process = ProcessInfo.processInfo
        result.sessionActive = isSessionActive
        result.onBattery = Self.isOnBattery()
        result.lowPower = process.isLowPowerModeEnabled
        switch process.thermalState {
        case .nominal: result.thermalState = "nominal"
        case .fair: result.thermalState = "fair"
        case .serious: result.thermalState = "serious"
        case .critical: result.thermalState = "critical"
        @unknown default: result.thermalState = nil
        }
        // Aggregate elapsed idle time only; never inspect or subscribe to input.
        let idle = CGEventSource.secondsSinceLastEventType(.combinedSessionState, eventType: CGEventType(rawValue: UInt32.max)!)
        result.idleSeconds = idle.isFinite && idle >= 0 ? idle : nil
        return result
    }

    public func snapshot() -> DesktopSystemSnapshot {
        let process = ProcessInfo.processInfo
        let screen = CGPreflightScreenCaptureAccess()
        let accessibility = AXIsProcessTrusted()
        let thermal = process.thermalState
        let onBattery = Self.isOnBattery()
        let memoryGiB = Double(process.physicalMemory) / 1_073_741_824
        let displays = NSScreen.screens.enumerated().map { index, screen in
            let pixels = screen.convertRectToBacking(screen.frame)
            return L("%@：%@ × %@", index + 1, Int(pixels.width), Int(pixels.height))
        }.joined(separator: L("；"))
        let version = process.operatingSystemVersion
        let rows = [
            DesktopInfoRow(L("系统"), "macOS \(version.majorVersion).\(version.minorVersion).\(version.patchVersion)", symbol: "desktopcomputer"),
            DesktopInfoRow(L("芯片"), Self.sysctlString("machdep.cpu.brand_string") ?? L("未知"), symbol: "cpu"),
            DesktopInfoRow(L("内存"), String(format: "%.0f GiB", memoryGiB), symbol: "memorychip"),
            DesktopInfoRow(L("逻辑核心"), "\(process.activeProcessorCount)", symbol: "square.grid.3x3"),
            DesktopInfoRow(L("显示器"), displays.isEmpty ? L("不可用") : displays, symbol: "display"),
            DesktopInfoRow(L("屏幕权限"), screen ? L("已允许") : L("未允许"), symbol: "rectangle.dashed.badge.record"),
            DesktopInfoRow(L("辅助功能"), accessibility ? L("已允许") : L("未允许"), symbol: "accessibility"),
            DesktopInfoRow(L("低电量模式"), process.isLowPowerModeEnabled ? L("已开启") : L("未开启"), symbol: "battery.50percent"),
            DesktopInfoRow(L("供电"), onBattery.map { $0 ? L("使用电池") : L("外部电源") } ?? L("未知"), symbol: "powerplug"),
            DesktopInfoRow(L("温度状态"), Self.thermalDescription(thermal), symbol: "thermometer.medium")
        ]
        return DesktopSystemSnapshot(rows: rows, screenPermission: screen, accessibilityPermission: accessibility,
                                     thermalBlocked: thermal == .serious || thermal == .critical, lowPower: process.isLowPowerModeEnabled, onBattery: onBattery)
    }

    public func refreshApplicationCatalog() async {
        guard !catalogRefreshing else { return }
        catalogRefreshing = true; defer { catalogRefreshing = false }
        let paths = Array((UserDefaults.standard.stringArray(forKey: "desktop.applicationPaths") ?? []).prefix(256))
        let result = await Task.detached(priority: .utility) {
            (DesktopApplicationCatalog.scan(), paths.compactMap { DesktopApplicationCatalog.descriptor(url: URL(fileURLWithPath: $0)) })
        }.value
        installedApplications = result.0
        let latestPaths = UserDefaults.standard.stringArray(forKey: "desktop.applicationPaths") ?? []
        let inFlightAdds = manuallyAdded.filter { $0.bundlePath.map(latestPaths.contains) == true }
        manuallyAdded = Array(Dictionary((result.1 + inFlightAdds).map { ($0.bundleID, $0) }, uniquingKeysWith: { _, latest in latest }).values)
    }

    public func addApplication(url: URL) -> Bool {
        guard let app = DesktopApplicationCatalog.descriptor(url: url), app.bundleID != Bundle.main.bundleIdentifier else { return false }
        let paths = Array(Set((UserDefaults.standard.stringArray(forKey: "desktop.applicationPaths") ?? []) + [url.path])).sorted()
        guard paths.count <= 256 else { return false }
        manuallyAdded.removeAll { $0.bundleID == app.bundleID }; manuallyAdded.append(app)
        UserDefaults.standard.set(paths, forKey: "desktop.applicationPaths")
        return true
    }

    public func applications() -> [DesktopApplication] {
        var known = Dictionary((installedApplications + manuallyAdded).map { ($0.bundleID, $0) }, uniquingKeysWith: { first, _ in first })
        for app in NSWorkspace.shared.runningApplications {
            guard let url = app.bundleURL, let bundleID = app.bundleIdentifier,
                  app.activationPolicy == .regular || known[bundleID] != nil else { continue }
            let cached = known[bundleID].flatMap { $0.bundlePath == url.path ? $0 : nil }
            guard var application = cached ?? DesktopApplicationCatalog.descriptor(url: url) else { continue }
            application.pid = app.processIdentifier
            known[application.bundleID] = application
        }
        return known.values.filter { $0.bundleID != Bundle.main.bundleIdentifier }
            .sorted {
                let comparison = $0.name.localizedStandardCompare($1.name)
                return comparison == .orderedSame ? $0.bundleID < $1.bundleID : comparison == .orderedAscending
            }
    }

    public func start(onEvent: @escaping @MainActor (DesktopSystemEvent) -> Void) {
        stop()
        self.onEvent = onEvent
        DesktopSessionSafety.enabled = true
        blockedReasons = DesktopSessionSafety.currentConsoleIsUsable() ? [] : ["session_unknown"]
        updateSafety()
        let workspace = NSWorkspace.shared.notificationCenter
        observe(workspace, NSWorkspace.didActivateApplicationNotification) { [weak self] in self?.applicationChanged() }
        for (name, reason) in [(NSWorkspace.willSleepNotification, "sleep"),
                               (NSWorkspace.screensDidSleepNotification, "screen_off"),
                               (NSWorkspace.sessionDidResignActiveNotification, "session_inactive")] {
            observe(workspace, name) { [weak self] in self?.block(reason) }
        }
        for (name, reason) in [(NSWorkspace.didWakeNotification, "sleep"),
                               (NSWorkspace.screensDidWakeNotification, "screen_off"),
                               (NSWorkspace.sessionDidBecomeActiveNotification, "session_inactive")] {
            observe(workspace, name) { [weak self] in self?.unblock(reason) }
        }
        defaultObservers.append(NotificationCenter.default.addObserver(forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated {
                DesktopSessionSafety.epoch &+= 1
                self?.onEvent?(.init(kind: .system, detail: L("显示器配置已变化")))
            }
        })
        _ = ProcessInfo.processInfo.thermalState
        for name in [ProcessInfo.thermalStateDidChangeNotification, Notification.Name.NSProcessInfoPowerStateDidChange] {
            defaultObservers.append(NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.onEvent?(.init(kind: .system, detail: L("电源或温度状态已变化"))) }
            })
        }
        Self.activePowerMonitor = self
        if let source = IOPSNotificationCreateRunLoopSource({ _ in
            MainActor.assumeIsolated { DesktopSystemMonitor.activePowerMonitor?.onEvent?(.init(kind: .system, detail: L("供电来源已变化"))) }
        }, nil)?.takeRetainedValue() {
            powerSource = source
            CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
        }
        let distributed = DistributedNotificationCenter.default()
        // Compatibility signals: release validation must exercise actual lock,
        // sleep and fast-user-switch behavior on the supported OS.
        for (name, reason) in [("com.apple.screenIsLocked", "locked"), ("com.apple.screensaver.didstart", "screensaver")] {
            distributedObservers.append(distributed.addObserver(forName: Notification.Name(name), object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.block(reason) }
            })
        }
        for (name, reason) in [("com.apple.screenIsUnlocked", "locked"), ("com.apple.screensaver.didstop", "screensaver")] {
            distributedObservers.append(distributed.addObserver(forName: Notification.Name(name), object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.unblock(reason) }
            })
        }
        applicationChanged()
    }

    public func stop() {
        contextRevision &+= 1
        interactionTap.stop()
        for token in workspaceObservers { NSWorkspace.shared.notificationCenter.removeObserver(token) }
        for token in defaultObservers { NotificationCenter.default.removeObserver(token) }
        for token in distributedObservers { DistributedNotificationCenter.default().removeObserver(token) }
        workspaceObservers.removeAll(); defaultObservers.removeAll(); distributedObservers.removeAll()
        removeAXObserver()
        if let source = powerSource { CFRunLoopRemoveSource(CFRunLoopGetMain(), source, .commonModes) }
        powerSource = nil
        if Self.activePowerMonitor === self { Self.activePowerMonitor = nil }
        onEvent = nil; lastFocus = ""; isSessionActive = false
        DesktopSessionSafety.enabled = false
        DesktopSessionSafety.update(blocked: true)
    }

    private func observe(_ center: NotificationCenter, _ name: Notification.Name, action: @escaping @MainActor () -> Void) {
        workspaceObservers.append(center.addObserver(forName: name, object: nil, queue: .main) { _ in
            MainActor.assumeIsolated { action() }
        })
    }
    private func block(_ reason: String) {
        guard blockedReasons.insert(reason).inserted else { return }
        updateSafety(); removeAXObserver(); interactionTap.stop()
        onEvent?(.init(kind: .blocked, detail: L("采集暂时停止：%@", Self.reasonDescription(reason))))
    }
    private func unblock(_ reason: String) {
        blockedReasons.remove(reason)
        if DesktopSessionSafety.currentConsoleIsUsable() { blockedReasons.remove("session_unknown") }
        else { blockedReasons.insert("session_unknown") }
        updateSafety()
        if isSessionActive, interactionTriggersRequested { _ = startInteractionTap() }
        if isSessionActive { onEvent?(.init(kind: .resumed, detail: L("会话恢复；继续遵循原采集设置"))); applicationChanged() }
    }
    private func updateSafety() {
        isSessionActive = blockedReasons.isEmpty && DesktopSessionSafety.currentConsoleIsUsable()
        DesktopSessionSafety.update(blocked: !isSessionActive)
    }
    private func applicationChanged() {
        contextRevision &+= 1
        removeAXObserver(); lastFocus = ""
        interactionTap.discardPending()
        DesktopSessionSafety.epoch &+= 1
        guard let application = frontmostApplication else { removeAXObserver(); return }
        if blockedReasons == ["session_unknown"], DesktopSessionSafety.currentConsoleIsUsable() {
            blockedReasons.remove("session_unknown"); updateSafety()
        }
        guard isSessionActive else { return }
        if interactionTriggersRequested, !interactionTap.isActive { _ = startInteractionTap() }
        // Deliver lightweight app metadata immediately; AX title/geometry is
        // acquired on its own serial executor and emitted as a window event.
        onEvent?(.init(kind: .application, application: application, detail: L("前台应用切换"), trigger: "application"))
        if windowContentScope.contains(application.bundleID) { requestFocusRefresh(application) }
        else { removeAXObserver(); lastFocus = "" }
    }
    static func receivedAXSignal(trigger: String) { activeAXMonitor?.windowChanged(trigger: trigger) }
    private func windowChanged(trigger: String) {
        contextRevision &+= 1
        guard isSessionActive, let application = frontmostApplication, windowContentScope.contains(application.bundleID) else { return }
        if DesktopSamplingSignalPolicy.isActivityOnly(balanced: balancedSampling, trigger: trigger) {
            samplingActivity.noteText(at: ProcessInfo.processInfo.systemUptime)
            // No body, no title and no AX task: Session treats dirty as a hint,
            // never as an independently persistent observation.
            onEvent?(.init(kind: .context, application: application, trigger: "dirty"))
            requestFocusRefresh(application, trigger: trigger)
            return
        }
        if trigger == "window" { interactionTap.discardPending() }
        if trigger == "window" || trigger == "focus" { DesktopSessionSafety.epoch &+= 1 }
        requestFocusRefresh(application, trigger: trigger)
    }
    private func requestFocusRefresh(_ application: DesktopApplication, trigger: String = "window") {
        guard windowContentScope.contains(application.bundleID) else { return }
        focusGeneration &+= 1
        pendingFocus = application
        focusQuietPeriod.note(trigger: trigger, at: ProcessInfo.processInfo.systemUptime)
        Self.activeAXMonitor = self
        guard focusRefreshTask == nil else { return }
        runFocusRefresh()
    }
    private func runFocusRefresh() {
        focusRefreshTask = Task { [weak self] in
            guard let self else { return }
            while !Task.isCancelled, self.pendingFocus != nil, self.onEvent != nil, self.isSessionActive {
                // One task sleeps to the pending deadline. A new notification
                // extends that deadline; on waking we recheck before any AX IPC.
                // Continuous typing therefore never becomes a 300ms sampler.
                let now = ProcessInfo.processInfo.systemUptime
                guard let remaining = self.focusQuietPeriod.remaining(at: now) else { break }
                if remaining > 0 {
                    do { try await Task.sleep(for: .seconds(remaining)) } catch { break }
                    continue
                }
                guard let ready = self.focusQuietPeriod.takeReady(at: now), let application = self.pendingFocus else { continue }
                self.pendingFocus = nil
                let trigger = ready.trigger
                let generation = self.focusGeneration, owner = self.focusOwnerID
                let options = self.contentOptions
                let textAllowed = self.textContentScope.contains(application.bundleID) && (options.readableTextEnabled || options.selectedTextEnabled || options.inputValueBundleIDs?.contains(application.bundleID) == true)
                let includeText = textAllowed && (DesktopSamplingActivity.isTextSignal(trigger) || ["return", "mouse", "scroll"].contains(trigger) || options.inputValueBundleIDs?.contains(application.bundleID) == true || !self.balancedSampling)
                if includeText { self.textReadsInFlight += 1 }
                let focus = try? await DesktopAXWorker.shared.observeAndRead(application: application, ownerID: owner, includeText: includeText, observeText: textAllowed, minimalMetadata: self.balancedSampling, contentOptions: options)
                if includeText { self.textReadsInFlight -= 1 }
                guard !Task.isCancelled else { break }
                guard let focus else { continue }
                let checkpoint = includeText
                // A continuous text stream may advance its revision during IPC.
                // Revalidate the actual window instead of starving every checkpoint.
                let verified = checkpoint ? try? await DesktopAXWorker.shared.read(application: application, includeText: false, minimalMetadata: true, contentOptions: options) : nil
                guard DesktopContextSourcePolicy.accepts(expectedWindow: focus.windowIdentity,
                          actualWindow: checkpoint ? (verified?.windowIdentity ?? 0) : focus.windowIdentity,
                          expectedPID: application.pid, actualPID: self.frontmostApplication?.pid,
                          ownerUnchanged: owner == self.focusOwnerID,
                          generationUnchanged: self.focusQuietPeriod.isCurrent(ready) && generation == self.focusGeneration,
                          balanced: checkpoint, allowed: self.windowContentScope.contains(application.bundleID)
                            && (!textAllowed || self.textContentScope.contains(application.bundleID)),
                          active: self.isSessionActive && DesktopSessionSafety.canCapture),
                      !checkpoint || (verified?.context.isProtected != true && verified?.context.isComposing != true) else { continue }
                // Include focus identity, selection and bounded body in equality;
                // equal window titles must not swallow real text/focus events.
                let token = "\(focus.application.pid)|\(focus.windowIdentity)|\(focus.title)|\(focus.frame)|\(focus.focusIdentity)|\(focus.context)"
                if token != self.lastFocus {
                    self.lastFocus = token
                    let kind: DesktopSystemEvent.Kind = trigger == "window" ? .window : .context
                    self.onEvent?(.init(kind: kind, application: focus.application, title: focus.title,
                                        detail: ["return", "mouse"].contains(trigger) ? "输入操作已记录，发出未确认" : (kind == .window ? "当前窗口变化" : "焦点或可读文字变化"),
                                        trigger: trigger, context: self.resourceContext(focus.context), windowIdentity: focus.windowIdentity))
                } else {
                    // A harmless AX signal can settle without adding another
                    // observation, but must release a pending screenshot.
                    self.onEvent?(.init(kind: .context, application: focus.application, title: focus.title,
                                        trigger: "settled", context: self.resourceContext(focus.context), windowIdentity: focus.windowIdentity))
                }
            }
            self.focusRefreshTask = nil
            // A stop/start may have replaced the pending request while an old
            // AX call was still returning. Keep only the latest request.
            if self.pendingFocus != nil, self.onEvent != nil, self.isSessionActive { self.runFocusRefresh() }
        }
    }
    private func removeAXObserver() {
        focusGeneration &+= 1; pendingFocus = nil; focusQuietPeriod.cancel(); samplingActivity.cancel(); focusRefreshTask?.cancel()
        interactionTap.discardPending()
        let oldOwner = focusOwnerID
        focusOwnerID = UUID()
        Task { await DesktopAXWorker.shared.clear(ownerID: oldOwner) }
        if Self.activeAXMonitor === self { Self.activeAXMonitor = nil }
    }
    private static func reasonDescription(_ reason: String) -> String {
        switch reason { case "sleep": L("系统休眠"); case "screen_off": L("屏幕关闭"); case "session_inactive": L("用户会话不活跃"); case "locked": L("屏幕锁定"); case "screensaver": L("屏幕保护程序"); default: L("会话状态待确认") }
    }
    private static func thermalDescription(_ state: ProcessInfo.ThermalState) -> String {
        switch state { case .nominal: L("正常"); case .fair: L("轻微升温"); case .serious: L("高温，暂停截图"); case .critical: L("严重高温，暂停截图"); @unknown default: L("未知") }
    }
    private static func sysctlString(_ name: String) -> String? {
        var size = 0
        guard sysctlbyname(name, nil, &size, nil, 0) == 0, size > 0, size < 4096 else { return nil }
        var buffer = [CChar](repeating: 0, count: size)
        guard sysctlbyname(name, &buffer, &size, nil, 0) == 0 else { return nil }
        return String(decoding: buffer.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }, as: UTF8.self)
    }
    private static func isOnBattery() -> Bool? {
        guard let info = IOPSCopyPowerSourcesInfo()?.takeRetainedValue(),
              let value = IOPSGetProvidingPowerSourceType(info)?.takeUnretainedValue() as String? else { return nil }
        if value == kIOPSBatteryPowerValue { return true }
        if value == kIOPSACPowerValue { return false }
        return nil
    }
}
