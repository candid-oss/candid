import CoreGraphics
import DesktopCore
import DesktopPlatform
import Foundation
import ImageIO
import CandidClient
import Testing
import TimelineConnectors
import UniformTypeIdentifiers
@testable import Candid

@MainActor
private func bootstrapDetailed(_ session: DesktopSession) async {
    await session.bootstrap()
    session.setSamplingMode(.detailed)
    for _ in 0..<100 {
        if session.samplingMode == .detailed { break }
        try? await Task.sleep(for: .milliseconds(10))
    }
}

@MainActor
private final class SyntheticMonitor: DesktopMonitoring {
    let editor = DesktopApplication(bundleID: "test.synthetic.editor", name: "Synthetic Editor", pid: 42)
    var frontmostApplication: DesktopApplication?
    var isSessionActive = true
    var isContentSettling = false
    var contextRevision: UInt64 = 0
    var snapshots = 0
    var starts = 0
    var stops = 0
    var screenAllowed = true
    var accessibilityAllowed = true
    var onBattery = false
    var inputMonitoringAuthorized = true
    var interactionTriggersActive = false
    var selectedReturn = true
    var selectedScroll = true
    func setInteractionTriggerSelection(returnEnabled: Bool, scrollEnabled: Bool) { selectedReturn = returnEnabled; selectedScroll = scrollEnabled }
    func setInteractionTriggersEnabled(_ enabled: Bool) -> Bool { interactionTriggersActive = enabled; return true }
    var contextReads = 0
    var bodyText = "SYNTHETIC visible body"
    var contextTitle = "Synthetic context"
    var windowIdentity: UInt?
    var holdsContext = false
    var textScope: Set<String> = []
    var hideApplications = false
    private var contextWaiter: CheckedContinuation<DesktopContextReading, any Error>?
    private var callback: (@MainActor (DesktopSystemEvent) -> Void)?
    private var lateCallback: (@MainActor (DesktopSystemEvent) -> Void)?
    init() { frontmostApplication = editor }
    func snapshot() -> DesktopSystemSnapshot {
        snapshots += 1
        return DesktopSystemSnapshot(rows: [.init("测试环境", "明确标记的合成系统")], screenPermission: screenAllowed,
                                     accessibilityPermission: accessibilityAllowed, thermalBlocked: false, lowPower: false, onBattery: onBattery)
    }
    func applications() -> [DesktopApplication] { hideApplications ? [] : [editor] }
    func setTextContentScope(allowedBundleIDs: Set<String>) { textScope = allowedBundleIDs }
    func readContext(allowedBundleIDs: Set<String>, includeText: Bool) async throws -> DesktopContextReading {
        contextReads += 1
        guard allowedBundleIDs.contains(editor.bundleID) else { throw DesktopCaptureError.excludedApplication }
        if holdsContext { return try await withCheckedThrowingContinuation { contextWaiter = $0 } }
        return contextReading(includeText: includeText)
    }
    func completeContext() { contextWaiter?.resume(returning: contextReading(includeText: true)); contextWaiter = nil }
    private func contextReading(includeText: Bool) -> DesktopContextReading {
        .init(application: editor, title: contextTitle, context: .init(focusedRole: "AXTextArea", selectedText: includeText ? "SYNTHETIC selection" : nil, readableText: includeText ? bodyText : nil, textStatus: includeText ? "captured" : "disabled"), windowIdentity: windowIdentity)
    }
    func start(onEvent: @escaping @MainActor (DesktopSystemEvent) -> Void) {
        starts += 1; isSessionActive = true; callback = onEvent; lateCallback = onEvent
    }
    func stop() { stops += 1; isSessionActive = false; callback = nil }
    func send(_ event: DesktopSystemEvent) {
        contextRevision &+= 1
        if event.kind == .blocked { isSessionActive = false }
        if event.kind == .resumed { isSessionActive = true }
        callback?(event)
    }
    func sendAfterStop(_ event: DesktopSystemEvent) { lateCallback?(event) }
}

@MainActor private final class CaptureClock {
    var now: Double = 0
    var waiting: [(Double, CheckedContinuation<Void, any Error>)] = []
    func sleep(_ duration: Duration) async throws {
        let parts = duration.components
        let seconds = Double(parts.seconds) + Double(parts.attoseconds) / 1e18
        if seconds >= 1 {
            try await withCheckedThrowingContinuation { waiting.append((seconds, $0)) }
        } else {
            now += seconds
            try await Task.sleep(for: .milliseconds(10))
        }
    }
    func advanceNext() {
        guard !waiting.isEmpty else { return }
        let (seconds, continuation) = waiting.removeFirst(); now += seconds; continuation.resume()
    }
    func cancelAll() { let old = waiting; waiting.removeAll(); old.forEach { $0.1.resume(throwing: CancellationError()) } }
}

@MainActor
private final class SyntheticCapture: DesktopCapturing {
    var screenRequests = 0
    var accessibilityRequests = 0
    var cancellations = 0
    var calls = 0
    var capturedAllowedSets: [Set<String>] = []
    var capturedCompatibilitySets: [Set<String>] = []
    var encodingReuseHints: [Bool] = []
    func setImageEncodingReuseEnabled(_ enabled: Bool) { encodingReuseHints.append(enabled) }
    var holdsResult = false
    var captureError: DesktopCaptureError?
    var fixedImageTone: UInt8?
    var application = DesktopApplication(bundleID: "test.synthetic.editor", name: "Synthetic Editor", pid: 42)
    var frameTitle = ""
    var imageOptions = DesktopImageOptions()
    func setImageOptions(_ options: DesktopImageOptions) { imageOptions = options }
    private var waiter: CheckedContinuation<CapturedWindow, any Error>?
    @discardableResult func requestScreenPermission() -> Bool { screenRequests += 1; return true }
    @discardableResult func requestAccessibilityPermission() -> Bool { accessibilityRequests += 1; return true }
    func cancelPendingCapture() {
        cancellations += 1
        // Deliberately ignore cancellation like a framework callback already in
        // flight. The real Session must reject the eventual returned content.
    }
    func captureFrontmost(allowedBundleIDs: Set<String>, windowCompatibilityBundleIDs: Set<String>,
                          axTextBundleIDs: Set<String>, includeAXFocus: Bool) async throws -> CapturedWindow {
        calls += 1; capturedAllowedSets.append(allowedBundleIDs)
        capturedCompatibilitySets.append(windowCompatibilityBundleIDs)
        guard allowedBundleIDs.contains(application.bundleID) else { throw DesktopCaptureError.excludedApplication }
        if let captureError { throw captureError }
        if holdsResult {
            return try await withCheckedThrowingContinuation { continuation in waiter = continuation }
        }
        return try frame()
    }
    func completePending() {
        guard let continuation = waiter else { return }
        waiter = nil
        do { continuation.resume(returning: try frame()) }
        catch { continuation.resume(throwing: error) }
    }
    private func frame() throws -> CapturedWindow {
        // A generated 2x2 JPEG, not a screenshot, copied image or private fixture.
        let tone = fixedImageTone ?? UInt8(truncatingIfNeeded: calls * 16)
        let bytes = Data(Array(repeating: [tone, tone, tone, UInt8(255)], count: 4).flatMap { $0 })
        guard let provider = CGDataProvider(data: bytes as CFData),
              let image = CGImage(width: 2, height: 2, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: 8,
                                  space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipLast.rawValue),
                                  provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent) else { throw DesktopCaptureError.unavailable }
        let encoded = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(encoded, UTType.jpeg.identifier as CFString, 1, nil) else { throw DesktopCaptureError.unavailable }
        CGImageDestinationAddImage(destination, image, nil)
        guard CGImageDestinationFinalize(destination) else { throw DesktopCaptureError.unavailable }
        return CapturedWindow(data: encoded as Data, application: application, title: frameTitle, width: 2, height: 2, capturedAt: Date())
    }
}

private func syntheticSessionRoot() throws -> URL {
    let location = FileManager.default.temporaryDirectory.appendingPathComponent("desktop-session-synthetic-" + UUID().uuidString)
    try FileManager.default.createDirectory(at: location, withIntermediateDirectories: true)
    return location
}

@MainActor
// Generous deadline: the suite runs targets in parallel, so a busy machine must
// not turn a slow-but-correct path into a failure. Passing tests return early.
private func eventually(_ description: String, timeout: Duration = .seconds(10), _ predicate: @escaping @MainActor () -> Bool) async throws {
    let clock = ContinuousClock()
    let deadline = clock.now.advanced(by: timeout)
    while !predicate(), clock.now < deadline { try await Task.sleep(for: .milliseconds(10)) }
    try #require(predicate(), Comment(rawValue: description))
}

@MainActor
private func enableImages(_ session: DesktopSession, bundle: String) async throws {
    session.setAllowedApp(bundle, true)
    try await eventually("allowed application persisted") { session.allowedBundleIDs.contains(bundle) }
    session.setScreenshotsEnabled(true)
    try await eventually("screenshot preference persisted") { session.screenshotsEnabled }
}

@Suite(.serialized) @MainActor
struct DesktopSessionTests {
    @Test func windowCompatibilityPersistsAndIsRemovedOnExclusion() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture()
        let bundle = monitor.editor.bundleID
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture, automaticCaptureAvailable: true)
        defer { session.prepareToQuit() }
        await bootstrapDetailed(session)
        session.setWindowCompatibility(bundle, true)
        try await enableImages(session, bundle: bundle)
        #expect(session.windowCompatibilityBundleIDs.isEmpty)
        session.setWindowCompatibility(bundle, true)
        try await eventually("compatibility saved") { session.windowCompatibilityBundleIDs == [bundle] }
        #expect(try DesktopPreferencesFile(directory: root).load().windowCompatibilityBundleIDs == [bundle])
        session.startRecording()
        try await eventually("recording started") { session.isRecording }
        monitor.send(.init(kind: .window, application: monitor.editor, title: "Synthetic compatible window"))
        try await eventually("compatibility forwarded to capture") { capture.capturedCompatibilitySets == [[bundle]] }
        session.setExcludedApp(bundle, true)
        try await eventually("exclusion revokes compatibility") { session.excludedBundleIDs.contains(bundle) && session.windowCompatibilityBundleIDs.isEmpty }
        #expect(try DesktopPreferencesFile(directory: root).load().windowCompatibilityBundleIDs.isEmpty)
    }

    @Test func contentTogglesRedactBothObservationsAndImagesAndImageOptionsReachCapture() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture(), clock = CaptureClock()
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture, automaticCaptureAvailable: true, captureSleep: { try await clock.sleep($0) }, captureClock: { clock.now })
        defer { session.prepareToQuit(); clock.cancelAll() }
        await session.bootstrap(); try await enableImages(session, bundle: monitor.editor.bundleID)
        session.setTextAllowedApp(monitor.editor.bundleID, true)
        try await eventually("text application scope saved") { session.textAllowedBundleIDs.contains(monitor.editor.bundleID) }
        session.setTextEnabled(true)
        try await eventually("text ready") { session.textEnabled }
        session.updateContentOption(\.windowTitleEnabled, value: false)
        session.updateContentOption(\.selectedTextEnabled, value: false)
        session.updateContentOption(\.focusEnabled, value: false)
        session.updateContentOption(\.systemStateEnabled, value: false)
        session.updateImageOption(\.maximumEdge, value: 2560)
        session.updateImageOption(\.jpegQuality, value: 0.9)
        session.setPeriodicEnabled(true)
        try await eventually("independent options composed") {
            !session.contentOptions.windowTitleEnabled && !session.contentOptions.selectedTextEnabled && !session.contentOptions.focusEnabled && !session.contentOptions.systemStateEnabled && session.imageOptions.maximumEdge == 2560 && session.imageOptions.jpegQuality == 0.9 && session.periodicEnabled
        }
        #expect(capture.imageOptions == session.imageOptions)
        session.startRecording(); try await eventually("started") { session.isRecording }
        await session.samplePeriodicContext()
        try await eventually("paired content and image") { session.samplingDiagnostics.imageSaved == 1 }
        let records = try await storedRecords(root).filter { $0.kind == .context || $0.kind == .screenshot }
        #expect(records.count == 2)
        for record in records {
            #expect(record.windowTitle.isEmpty && record.context?.readableText == monitor.bodyText)
            #expect(record.context?.selectedText == nil && record.context?.focusedRole == nil && record.context?.focusedLabel == nil)
            #expect(record.context?.onBattery == nil && record.context?.idleSeconds == nil && record.context?.sessionActive == nil)
        }
        let image = try #require(records.first { $0.kind == .screenshot })
        #expect(records.contains { $0.id == image.parentID && $0.activityID == image.activityID })
        #expect(session.error == nil)
    }

    @Test func customSamplingSettingsPersistWithoutBalancedClampingAndRejectInvalidEdits() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: SyntheticMonitor(), capture: SyntheticCapture())
        defer { session.prepareToQuit() }
        await session.bootstrap()
        var options = DesktopSamplingOptions(); options.minimumContentInterval = 2; options.contentCapacity = 8
        options.imageCapacity = 10; options.imageFallbackInterval = 30
        session.setSamplingOptions(options); session.setMinimumCaptureInterval(2); session.setPeriodicInterval(30)
        // Two UI edits before the asynchronous save completes must compose on
        // desired state, rather than each copying the last displayed policy.
        session.updateSamplingOption(\.minimumContentInterval, value: 5)
        session.updateSamplingOption(\.contentCapacity, value: 9)
        options.minimumContentInterval = 5; options.contentCapacity = 9
        try await eventually("custom policy saved") { session.samplingOptions == options && session.periodicInterval == 30 && session.minimumCaptureInterval == 2 }
        var invalid = options; invalid.contentCapacity = 0
        session.setSamplingOptions(invalid); session.setMinimumCaptureInterval(.nan)
        let restored = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: SyntheticMonitor(), capture: SyntheticCapture())
        defer { restored.prepareToQuit() }
        await restored.bootstrap()
        #expect(restored.samplingOptions == options && restored.minimumCaptureInterval == 2 && restored.periodicInterval == 30)
        #expect(!restored.isRecording && restored.error == nil)
        session.applyFastSamplingFrequency()
        try await eventually("fast preset saved") {
            session.minimumCaptureInterval == 3 && session.periodicInterval == 10 &&
                session.samplingOptions.minimumContentInterval == 3 && session.samplingOptions.imageFallbackInterval == 10 &&
                session.samplingOptions.contentCapacity == 20 && session.samplingOptions.imageCapacity == 20
        }
        #expect(!session.periodicEnabled)
        #expect(!session.isRecording && !session.automaticCaptureAvailable)
        session.restoreSamplingDefaults()
        try await eventually("defaults restored") { session.samplingOptions == DesktopSamplingOptions() && session.periodicInterval == 60 && session.minimumCaptureInterval == 15 }
    }

    @Test func repeatedUnknownFocusDoesNotDelayTheNextCapturableWindow() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture(), clock = CaptureClock()
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture, automaticCaptureAvailable: true,
                                     captureSleep: { try await clock.sleep($0) }, captureClock: { clock.now })
        defer { session.prepareToQuit(); clock.cancelAll() }
        await bootstrapDetailed(session); try await enableImages(session, bundle: monitor.editor.bundleID)
        session.startRecording(); try await eventually("started") { session.isRecording && monitor.starts == 1 }
        capture.captureError = .noVerifiedFocus
        for failure in 1...3 {
            clock.now += 31
            monitor.send(.init(kind: .window, application: monitor.editor, title: "Synthetic window \(failure)", detail: "unknown focus"))
            try await eventually("failed capture \(failure)") { session.captureFailures == failure }
        }
        capture.captureError = nil
        clock.now += 31
        monitor.send(.init(kind: .window, application: monitor.editor, title: "Synthetic verified window", detail: "known focus"))
        try await eventually("capture resumes without a shared cooldown") { session.samplingDiagnostics.imageSaved == 1 }
    }

    @Test func disabledWindowTriggerRetainsActivityAndTimerStillCaptures() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture(), clock = CaptureClock()
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture, automaticCaptureAvailable: true, captureSleep: { try await clock.sleep($0) }, captureClock: { clock.now })
        defer { session.prepareToQuit(); clock.cancelAll() }
        await session.bootstrap(); try await enableImages(session, bundle: monitor.editor.bundleID)
        var options = session.samplingOptions; options.windowTriggersEnabled = false
        session.setSamplingOptions(options); session.setPeriodicEnabled(true)
        try await eventually("window trigger disabled") { !session.samplingOptions.windowTriggersEnabled && session.periodicEnabled }
        session.startRecording(); try await eventually("started") { session.isRecording }
        monitor.send(.init(kind: .application, application: monitor.editor, trigger: "application"))
        monitor.send(.init(kind: .window, application: monitor.editor, title: monitor.contextTitle, trigger: "window", windowIdentity: 7))
        try await eventually("activity retained") { session.records.contains { $0.kind == .window } }
        #expect(monitor.contextReads == 0 && capture.calls == 0)
        await session.samplePeriodicContext()
        try await eventually("timer independently captures") { session.samplingDiagnostics.imageSaved == 1 }
    }

    @Test func independentReturnAndScrollPreferencesFilterLateSignals() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), clock = CaptureClock()
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: SyntheticCapture(), captureSleep: { try await clock.sleep($0) }, captureClock: { clock.now })
        defer { session.prepareToQuit(); clock.cancelAll() }
        await session.bootstrap(); try await enableImages(session, bundle: monitor.editor.bundleID)
        session.setInteractionTrigger("return", enabled: true)
        try await eventually("return only saved") { session.interactionTriggersEnabled && session.samplingOptions.returnEnabled && !session.samplingOptions.scrollEnabled }
        #expect(monitor.selectedReturn && !monitor.selectedScroll)
        session.startRecording(); try await eventually("started") { session.isRecording }
        monitor.send(.init(kind: .context, application: monitor.editor, trigger: "scroll"))
        try await Task.sleep(for: .milliseconds(50)); #expect(monitor.contextReads == 0)
        monitor.send(.init(kind: .context, application: monitor.editor, trigger: "return"))
        try await eventually("return samples") { monitor.contextReads == 1 }
        session.setInteractionTrigger("return", enabled: false)
        try await eventually("last trigger stops native monitoring") { !session.interactionTriggersEnabled && !monitor.interactionTriggersActive }
        monitor.send(.init(kind: .context, application: monitor.editor, trigger: "return"))
        try await Task.sleep(for: .milliseconds(50)); #expect(monitor.contextReads == 1)
    }

    @Test func customImageFallbackCanBeShorterThanOldTwoMinuteFloor() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture(), clock = CaptureClock()
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture, automaticCaptureAvailable: true, captureSleep: { try await clock.sleep($0) }, captureClock: { clock.now })
        defer { session.prepareToQuit(); clock.cancelAll() }
        await session.bootstrap(); try await enableImages(session, bundle: monitor.editor.bundleID)
        var options = session.samplingOptions; options.minimumContentInterval = 2; options.imageFallbackInterval = 30
        session.setSamplingOptions(options); session.setMinimumCaptureInterval(2); session.setPeriodicEnabled(true)
        try await eventually("policy saved") { session.samplingOptions == options && session.minimumCaptureInterval == 2 && session.periodicEnabled }
        session.startRecording(); try await eventually("started") { session.isRecording }
        await session.samplePeriodicContext(); try await eventually("initial image") { capture.calls == 1 && session.samplingDiagnostics.imageSaved == 1 }
        clock.now = 10; await session.samplePeriodicContext()
        try await eventually("unchanged text check") { session.samplingDiagnostics.unchangedChecks == 1 }
        #expect(capture.calls == 1)
        clock.now = 31; await session.samplePeriodicContext()
        try await eventually("configured 30 second image") { session.samplingDiagnostics.imageSaved == 2 }
    }

    @Test func customContentBudgetAndBatterySwitchAffectRealScheduler() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), clock = CaptureClock(); monitor.onBattery = true
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: SyntheticCapture(), captureSleep: { try await clock.sleep($0) }, captureClock: { clock.now })
        defer { session.prepareToQuit(); clock.cancelAll() }
        await session.bootstrap(); try await enableImages(session, bundle: monitor.editor.bundleID)
        // 后台处理 随时: capture is not throttled on battery.
        session.capturePace = { .anytime }
        var options = session.samplingOptions; options.minimumContentInterval = 2; options.contentCapacity = 2
        session.setSamplingOptions(options); session.setPeriodicEnabled(true)
        try await eventually("custom text policy saved") { session.samplingOptions == options && session.periodicEnabled }
        session.startRecording(); try await eventually("started") { session.isRecording }
        await session.samplePeriodicContext(); try await eventually("first read") { monitor.contextReads == 1 }
        clock.now = 3; await session.samplePeriodicContext()
        try await eventually("battery switch allows configured fast interval") { monitor.contextReads == 2 }
        clock.now = 6; await session.samplePeriodicContext()
        try await eventually("capacity wait registered") { !clock.waiting.isEmpty }
        #expect(monitor.contextReads == 2)
        clock.advanceNext()
        try await eventually("rolling minute restores text budget") { monitor.contextReads == 3 }
    }

    @Test func balancedDirtyDoesNotReadOrSaveUntilCheckpointAndUnchangedSavesNothing() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), clock = CaptureClock()
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: SyntheticCapture(), captureSleep: { try await clock.sleep($0) }, captureClock: { clock.now })
        defer { session.prepareToQuit(); clock.cancelAll() }
        await session.bootstrap()
        session.setTextAllowedApp(monitor.editor.bundleID, true)
        try await eventually("text scope") { !session.textAllowedBundleIDs.isEmpty }
        session.setTextEnabled(true); session.setPeriodicEnabled(true)
        try await eventually("sampling enabled") { session.textEnabled && session.periodicEnabled }
        session.startRecording(); try await eventually("started") { session.isRecording }
        for _ in 0..<80 { monitor.send(.init(kind: .context, application: monitor.editor, trigger: "dirty")) }
        try await Task.sleep(for: .milliseconds(50))
        #expect(monitor.contextReads == 0 && !session.records.contains { $0.kind == .context })
        // No quiet interval is required for a periodic text checkpoint.
        monitor.isContentSettling = true
        await session.samplePeriodicContext()
        try await eventually("first checkpoint persisted") { session.records.contains { $0.trigger == "timer" } }
        let first = try #require(session.records.first { $0.trigger == "timer" })
        clock.now = 61
        await session.samplePeriodicContext()
        try await eventually("unchanged check") { session.samplingDiagnostics.unchangedChecks == 1 }
        #expect(session.records.filter { $0.kind == .context }.count == 1 && monitor.contextReads == 2)
        // An unchanged check saves nothing new: the core holds only the first sampled observation.
        #expect(try await storedRecords(root).filter { $0.kind == .context }.map(\.id) == [first.id])
        clock.now = 122; monitor.bodyText += "!"
        await session.samplePeriodicContext()
        try await eventually("small actual change preserved") { session.records.filter { $0.kind == .context }.count == 2 }
        #expect(session.records.contains { $0.context?.readableText == monitor.bodyText })
    }

    @Test func balancedVisualFallbackSurvivesEqualTextWithoutThirtySecondPictures() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture(), clock = CaptureClock()
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture, automaticCaptureAvailable: true, captureSleep: { try await clock.sleep($0) }, captureClock: { clock.now })
        defer { session.prepareToQuit(); clock.cancelAll() }
        await session.bootstrap(); try await enableImages(session, bundle: monitor.editor.bundleID)
        session.setTextAllowedApp(monitor.editor.bundleID, true)
        try await eventually("text scope") { !session.textAllowedBundleIDs.isEmpty }
        session.setTextEnabled(true); session.setPeriodicEnabled(true)
        try await eventually("enabled") { session.textEnabled && session.periodicEnabled }
        session.startRecording(); try await eventually("started") { session.isRecording }
        await session.samplePeriodicContext()
        try await eventually("first image") { session.samplingDiagnostics.imageSaved == 1 }
        clock.now = 61; await session.samplePeriodicContext()
        try await eventually("text-only check") { session.samplingDiagnostics.unchangedChecks == 1 }
        #expect(capture.calls == 1)
        clock.now = 123; await session.samplePeriodicContext()
        try await eventually("visual fallback despite equal body") { session.samplingDiagnostics.imageSaved == 2 }
        #expect(capture.calls == 2)
        // The legacy synthetic provider emits equal text without R2 evidence.
        // Its repeated observations may share a canonical parent after delivery;
        // both distinct frames must survive with valid persisted source links.
        let persisted = try await storedRecords(root)
        let snapshots = persisted.filter { $0.kind == .screenshot }
        #expect(snapshots.count == 2 && Set(snapshots.map(\.id)).count == 2)
        #expect(snapshots.allSatisfy { shot in shot.parentID.map { parent in persisted.contains { $0.id == parent } } == true })
    }

    @Test func balancedUntitledFirstWindowGetsSnapshotAndOrdinaryFocusDoesNot() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture(), clock = CaptureClock()
        monitor.contextTitle = ""; monitor.windowIdentity = 7
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture, automaticCaptureAvailable: true, captureSleep: { try await clock.sleep($0) }, captureClock: { clock.now })
        defer { session.prepareToQuit(); clock.cancelAll() }
        await session.bootstrap(); try await enableImages(session, bundle: monitor.editor.bundleID)
        session.startRecording(); try await eventually("started") { session.isRecording }
        monitor.send(.init(kind: .application, application: monitor.editor, trigger: "application"))
        monitor.send(.init(kind: .window, application: monitor.editor, title: "", trigger: "window", windowIdentity: 7))
        try await eventually("untitled source captured") { session.samplingDiagnostics.imageSaved == 1 }
        let reads = monitor.contextReads
        for _ in 0..<20 { monitor.send(.init(kind: .context, application: monitor.editor, title: "", trigger: "focus", windowIdentity: 7)) }
        try await Task.sleep(for: .milliseconds(80))
        #expect(monitor.contextReads == reads && capture.calls == 1)
    }

    @Test func balancedLateCheckpointCannotWriteAfterPause() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), clock = CaptureClock(); monitor.holdsContext = true
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: SyntheticCapture(), captureSleep: { try await clock.sleep($0) }, captureClock: { clock.now })
        defer { session.prepareToQuit(); monitor.completeContext(); clock.cancelAll() }
        await session.bootstrap(); session.setTextAllowedApp(monitor.editor.bundleID, true)
        try await eventually("text scope") { !session.textAllowedBundleIDs.isEmpty }
        session.setTextEnabled(true); session.setPeriodicEnabled(true)
        try await eventually("enabled") { session.textEnabled && session.periodicEnabled }
        session.startRecording(); try await eventually("started") { session.isRecording }
        await session.samplePeriodicContext(); try await eventually("read in flight") { monitor.contextReads == 1 }
        session.pauseRecording(); monitor.completeContext()
        try await Task.sleep(for: .milliseconds(100))
        #expect(try await storedRecords(root).filter { $0.kind == .context }.isEmpty)
        #expect(session.samplingDiagnostics.imageAttempts == 0)
    }

    @Test func hiddenTimelineWritesWithoutReloadingAndRefreshesOnReturn() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: SyntheticMonitor(), capture: SyntheticCapture())
        defer { session.prepareToQuit() }
        await session.bootstrap(); session.setTimelineVisible(false)
        let reloads = session.samplingDiagnostics.listReloads
        session.addMarker()
        // Hidden, deliveries refresh the counts at most once a minute (each refresh counts the whole library); a
        // settings page on screen asks for them, as this does.
        for _ in 0..<500 where session.statistics.recordCount != 1 {
            await session.refreshStatistics(maxAge: 0)
            try await Task.sleep(for: .milliseconds(20))
        }
        #expect(session.statistics.recordCount == 1, "hidden write committed")
        #expect(session.samplingDiagnostics.listReloads == reloads)
        #expect(session.records.isEmpty)
        session.setTimelineVisible(true)
        try await eventually("return loads saved marker") { session.timelineEvents.count == 1 && session.samplingDiagnostics.listReloads > reloads }
        #expect(session.records.contains { $0.kind == .marker })
    }

    @Test func differentWindowsWithEqualTitlesNeverShareAnActivity() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor()
        let active = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: SyntheticCapture())
        defer { active.prepareToQuit() }
        await bootstrapDetailed(active); active.setAllowedApp(monitor.editor.bundleID, true)
        try await eventually("window scope enabled") { active.allowedBundleIDs.contains(monitor.editor.bundleID) }
        active.startRecording(); try await eventually("started") { active.isRecording && monitor.starts == 1 }
        for identity: UInt in [11,22] {
            monitor.send(.init(kind: .window, application: monitor.editor, title: "Untitled", trigger: "window", windowIdentity: identity))
            monitor.send(.init(kind: .context, application: monitor.editor, title: "Untitled", trigger: "content", windowIdentity: identity))
        }
        try await eventually("both windows saved") { active.records.filter { $0.bundleID == monitor.editor.bundleID }.count == 4 }
        try await eventually("timeline shows both windows") { active.timelineEvents.filter { $0.latestRecord.bundleID == monitor.editor.bundleID }.count == 2 }
        let activities = active.timelineEvents.filter { $0.latestRecord.bundleID == monitor.editor.bundleID }
        #expect(activities.count == 2 && activities.allSatisfy { $0.sampleCount == 2 })
    }

    @Test func earlierObservationSelectionSurvivesRefreshAndNewSamples() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let now = Date()
        try await writeDirectly(root, (0..<400).map { index in
            DesktopRecord(id: "sample-\(index)", timestamp: now.addingTimeInterval(Double(index - 400)), kind: .context, bundleID: "test.synthetic.editor", windowTitle: "Synthetic long activity", runID: "synthetic-paging", trigger: "timer", activityID: "synthetic-activity")
        })
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: SyntheticMonitor(), capture: SyntheticCapture())
        defer { session.prepareToQuit() }
        await bootstrapDetailed(session)
        #expect(session.selectedEvent?.sampleCount == 400 && session.selectedEvent?.observations.count == 120)
        session.loadEarlierObservations()
        try await eventually("earlier bounded page selected") { !session.observationPageLoading && session.selectedEvent?.observationOffset == 160 }
        let selected = try #require(session.selectedRecord?.id)
        #expect(selected == "sample-279")
        try await writeDirectly(root, [DesktopRecord(id: "sample-400", timestamp: now, kind: .context, bundleID: "test.synthetic.editor", windowTitle: "Synthetic long activity", runID: "synthetic-paging", trigger: "timer", activityID: "synthetic-activity")])
        session.refresh()
        try await eventually("refresh includes new observation") { session.selectedEvent?.sampleCount == 401 }
        #expect(session.selectedRecord?.id == selected && session.selectedEvent?.observations.count == 120)
    }

    @Test func typingBlocksTimerAndStableSamplesFormOneActivity() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture(); monitor.isContentSettling = true
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture, periodicSleep: { _ in try await Task.sleep(for: .milliseconds(40)) })
        defer { session.prepareToQuit() }
        await bootstrapDetailed(session); session.setPeriodicEnabled(true)
        try await eventually("periodic enabled") { session.periodicEnabled }
        session.startRecording(); try await eventually("started") { session.isRecording }
        try await Task.sleep(for: .milliseconds(180))
        #expect(monitor.contextReads == 0 && !session.records.contains { $0.trigger == "timer" })
        monitor.isContentSettling = false
        try await eventually("three periodic observations") { session.records.filter { $0.trigger == "timer" }.count >= 3 }
        // The list reloads on a debounce after records change; wait for it rather than racing it.
        try await eventually("timeline reflects the samples") {
            session.timelineEvents.filter { $0.latestRecord.bundleID == monitor.editor.bundleID }.contains { $0.endDate > $0.startDate }
        }
        // Stable samples of the same text fold into one observation whose end moves with each sample.
        let activities = session.timelineEvents.filter { $0.latestRecord.bundleID == monitor.editor.bundleID }
        #expect(activities.count == 1 && activities[0].sampleCount >= 1)
        #expect(activities[0].startDate < activities[0].endDate)
    }

    @Test(arguments: [DesktopSamplingMode.balanced, .detailed])
    func distinctWindowsCaptureBeforeTheSameWindowInterval(mode: DesktopSamplingMode) async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture(), clock = CaptureClock()
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture, automaticCaptureAvailable: true,
            captureSleep: { try await clock.sleep($0) }, captureClock: { clock.now })
        defer { session.prepareToQuit(); clock.cancelAll() }
        if mode == .detailed { await bootstrapDetailed(session) } else { await session.bootstrap() }
        try await enableImages(session, bundle: monitor.editor.bundleID)
        session.startRecording()
        try await eventually("started") { session.isRecording }
        for window in 1...2 {
            monitor.windowIdentity = UInt(window)
            monitor.contextTitle = "Window \(window)"
            monitor.send(.init(kind: .window, application: monitor.editor, title: monitor.contextTitle, windowIdentity: monitor.windowIdentity))
            try await eventually("window \(window) captured without advancing clock") { session.samplingDiagnostics.imageSaved == window }
        }
        #expect(clock.now <= 1.6 && capture.calls == 2)
    }

    @Test func unchangedTimedImageSkipsStorageAndChecksLessOften() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture(), clock = CaptureClock()
        capture.fixedImageTone = 100
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture, automaticCaptureAvailable: true,
            captureSleep: { try await clock.sleep($0) }, captureClock: { clock.now })
        defer { session.prepareToQuit(); clock.cancelAll() }
        await bootstrapDetailed(session)
        try await enableImages(session, bundle: monitor.editor.bundleID)
        session.setPeriodicEnabled(true)
        session.setPeriodicInterval(30)
        try await eventually("timer configured") { session.periodicEnabled && session.periodicInterval == 30 }
        session.startRecording()
        try await eventually("started") { session.isRecording }
        await session.samplePeriodicContext()
        try await eventually("first frame saved") { session.samplingDiagnostics.imageSaved == 1 }
        clock.now += 30
        await session.samplePeriodicContext()
        try await eventually("duplicate checked") { capture.calls == 2 && !session.captureBusy }
        #expect(session.samplingDiagnostics.imageSaved == 1)
        clock.now += 30
        await session.samplePeriodicContext()
        #expect(capture.calls == 2 && !session.captureBusy)
        capture.fixedImageTone = 200
        clock.now += 30
        await session.samplePeriodicContext()
        try await eventually("changed frame saved") { session.samplingDiagnostics.imageSaved == 2 }
        #expect(capture.calls == 3)
        #expect(capture.encodingReuseHints == [false, true, true])
    }

    @Test func rateLimitedCaptureKeepsLatestSnapshotAndEventuallyAttachesItsImage() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture(), clock = CaptureClock()
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture, automaticCaptureAvailable: true,
                                     captureSleep: { try await clock.sleep($0) }, captureClock: { clock.now })
        defer { session.prepareToQuit(); clock.cancelAll() }
        await bootstrapDetailed(session); try await enableImages(session, bundle: monitor.editor.bundleID)
        session.startRecording(); try await eventually("started") { session.isRecording && monitor.starts == 1 }
        monitor.send(.init(kind: .window, application: monitor.editor, title: "Synthetic stable window", detail: "first"))
        try await eventually("first screenshot saved") { session.records.contains { $0.kind == .screenshot } }
        monitor.send(.init(kind: .context, application: monitor.editor, title: "Synthetic stable window", detail: "second", trigger: "content"))
        try await eventually("waiting for rate budget") { !clock.waiting.isEmpty }
        #expect(capture.calls == 1)
        monitor.send(.init(kind: .context, application: monitor.editor, title: "Synthetic stable window", detail: "final", trigger: "content"))
        try await eventually("latest stable observation saved") { session.records.contains { $0.detail == "final" } }
        let last = try #require(session.records.first { $0.detail == "final" })
        #expect(session.captureFeedback(for: last) != nil)
        clock.advanceNext()
        try await eventually("deferred latest screenshot saved") { session.records.filter { $0.kind == .screenshot }.count == 2 }
        session.selectedID = session.timelineEvents.first { $0.latestRecord.bundleID == monitor.editor.bundleID }?.id
        try await eventually("selected activity loads its exact attachments") { session.selectedEvent?.screenshots.count == 2 }
        let activity = try #require(session.selectedEvent)
        // "final" reads the same (empty) text as "second" in the same window, so the core folds it into "second":
        // the latest screenshot attaches to that observation.
        let second = try #require(session.records.first { $0.detail == "second" })
        #expect(activity.sampleCount == 2 && activity.screenshots.count == 2)
        #expect(activity.screenshots.last?.parentID == second.id && last.id != second.id)
        #expect(capture.calls == 2)
        #expect(!session.records.contains { $0.kind == .gap })
    }

    @Test func continuousChangesKeepVerifiedCheckpointWithoutInheritingStaleText() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture(), clock = CaptureClock(); capture.holdsResult = true
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture, automaticCaptureAvailable: true,
                                     captureSleep: { try await clock.sleep($0) }, captureClock: { clock.now })
        defer { session.prepareToQuit(); capture.completePending(); clock.cancelAll() }
        await bootstrapDetailed(session); try await enableImages(session, bundle: monitor.editor.bundleID)
        session.startRecording(); try await eventually("started") { session.isRecording && monitor.starts == 1 }
        monitor.send(.init(kind: .window, application: monitor.editor, title: "Synthetic stable window", detail: "old", context: .init(readableText: "old text")))
        try await eventually("frame in flight") { capture.calls == 1 }
        monitor.isContentSettling = true; monitor.contextRevision &+= 1
        capture.completePending(); capture.holdsResult = false
        try await eventually("source-verified checkpoint saved during continuous changes") { session.records.contains { $0.kind == .screenshot } }
        let shot = try #require(session.records.first { $0.kind == .screenshot })
        #expect(shot.context?.readableText == nil)
        #expect(shot.context?.recordEvidence == nil)
        #expect(session.captureFailures == 0)
    }

    @Test func bulkContentScopeUsesCurrentAppsRespectsExclusionsAndCanBeRevoked() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture()
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture)
        defer { session.prepareToQuit() }
        await bootstrapDetailed(session)
        session.setExcludedApp(monitor.editor.bundleID, true)
        try await eventually("excluded app saved") { session.excludedBundleIDs.contains(monitor.editor.bundleID) }
        session.setContentAllowedForApps([monitor.editor.bundleID, "not.running.synthetic"], allowed: true)
        try await Task.sleep(for: .milliseconds(50))
        #expect(session.allowedBundleIDs.isEmpty && session.textAllowedBundleIDs.isEmpty)
        session.setExcludedApp(monitor.editor.bundleID, false)
        try await eventually("exclusion removed") { session.excludedBundleIDs.isEmpty }
        session.setContentAllowedForApps([monitor.editor.bundleID, "not.running.synthetic"], allowed: true)
        try await eventually("both scopes saved") { session.allowedBundleIDs == [monitor.editor.bundleID] && session.textAllowedBundleIDs == [monitor.editor.bundleID] }
        #expect(!session.screenshotsEnabled && !session.textEnabled && !session.isRecording)
        monitor.hideApplications = true; session.refresh()
        #expect(session.applications.first { $0.bundleID == monitor.editor.bundleID }?.pid == 0)
        session.setContentAllowedForApps([monitor.editor.bundleID], allowed: false)
        try await eventually("both scopes revoked") { session.allowedBundleIDs.isEmpty && session.textAllowedBundleIDs.isEmpty }
    }

    @Test func arrivingScreenshotEnrichesSelectedEventWithoutAddingTimelineRow() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture(); capture.holdsResult = true
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture, automaticCaptureAvailable: true)
        defer { session.prepareToQuit(); capture.completePending() }
        await bootstrapDetailed(session)
        try await enableImages(session, bundle: monitor.editor.bundleID)
        session.startRecording()
        try await eventually("recording active") { session.isRecording && monitor.starts == 1 }
        monitor.send(.init(kind: .context, application: monitor.editor, title: "Synthetic unified event", detail: "synthetic-unified-event", trigger: "selection", context: .init(focusedRole: "AXTextArea")))
        try await eventually("event visible before image arrives") { session.timelineEvents.contains { $0.record.detail == "synthetic-unified-event" } && capture.calls == 1 }
        let event = try #require(session.timelineEvents.first { $0.record.detail == "synthetic-unified-event" })
        let count = session.timelineEvents.count
        session.selectedID = event.id
        #expect(session.selectedImageRecord == nil)
        capture.completePending()
        try await eventually("image added to original event") { session.selectedEvent?.hasScreenshot == true && session.selectedImageRecord.flatMap { session.imageURL(for: $0) } != nil }
        #expect(session.timelineEvents.count == count && session.selectedID == event.id)
        #expect(session.selectedRecord?.trigger == "selection")
        #expect(session.selectedImageRecord?.parentID == event.id)
        #expect(session.timelineEvents.allSatisfy { $0.record.kind != .screenshot })
        let withoutImage = try #require(session.timelineEvents.first { $0.id != event.id })
        session.selectedID = withoutImage.id
        #expect(session.selectedImageRecord == nil && session.imageLocations.isEmpty)
    }

    @Test func focusAndSelectionEventsSaveTextWithoutScreenshotsAndScopeRevocationIsImmediate() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture()
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture)
        defer { session.prepareToQuit() }
        await bootstrapDetailed(session)
        session.setTextAllowedApp(monitor.editor.bundleID, true)
        try await eventually("text allow saved") { session.textAllowedBundleIDs.contains(monitor.editor.bundleID) }
        session.setTextEnabled(true)
        try await eventually("text reading enabled independently of pictures") { session.textEnabled }
        session.startRecording()
        try await eventually("recording active") { session.isRecording && monitor.starts == 1 }
        let context = DesktopContextSnapshot(focusedRole: "AXTextArea", focusedLabel: "Synthetic body", selectedText: "SYNTHETIC unique selection", readableText: "SYNTHETIC body", textStatus: "captured")
        monitor.send(.init(kind: .context, application: monitor.editor, title: "Synthetic text only", detail: "synthetic-selection", trigger: "selection", context: context))
        try await eventually("selection event saved") { session.records.contains { $0.trigger == "selection" } }
        let saved = try #require(session.records.first { $0.trigger == "selection" })
        #expect(saved.kind == .context && saved.context?.selectedText == context.selectedText)
        #expect(saved.windowTitle == "Synthetic text only" && !session.screenshotsEnabled && capture.calls == 0)
        #expect(try await storedRecords(root, query: "unique selection").contains { $0.id == saved.id })
        session.setTextAllowedApp(monitor.editor.bundleID, false)
        #expect(monitor.textScope.isEmpty)
        monitor.send(.init(kind: .context, application: monitor.editor, title: "Must disappear", detail: "synthetic-revoked", trigger: "focus", context: context))
        try await eventually("revoked event saved without content") { session.records.contains { $0.detail == "synthetic-revoked" } }
        let stripped = try #require(session.records.first { $0.detail == "synthetic-revoked" })
        #expect(stripped.windowTitle.isEmpty && stripped.context?.selectedText == nil && stripped.context?.readableText == nil && stripped.context?.focusedLabel == nil)
    }

    @Test func periodicReadsAreIndependentAndPauseRejectsLateText() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture(); monitor.holdsContext = true
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture, periodicSleep: { _ in try await Task.sleep(for: .milliseconds(40)) })
        defer { session.prepareToQuit(); monitor.completeContext() }
        await bootstrapDetailed(session)
        session.setTextAllowedApp(monitor.editor.bundleID, true)
        try await eventually("scope persisted") { !session.textAllowedBundleIDs.isEmpty }
        session.setTextEnabled(true)
        try await eventually("text enabled") { session.textEnabled }
        session.setPeriodicEnabled(true)
        try await eventually("timer enabled") { session.periodicEnabled }
        #expect(monitor.contextReads == 0)
        session.startRecording()
        try await eventually("timer reading in flight") { monitor.contextReads == 1 }
        try await Task.sleep(for: .milliseconds(140))
        #expect(monitor.contextReads == 1)
        session.pauseRecording()
        try await eventually("paused") { session.statusText == "已暂停" }
        monitor.completeContext()
        try await Task.sleep(for: .milliseconds(140))
        #expect(try await storedRecords(root).allSatisfy { $0.trigger != "timer" })
        #expect(monitor.contextReads == 1 && capture.calls == 0)
    }

    @Test func periodicSamplingStoresContextAndStopsOnSessionBlock() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture()
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture, periodicSleep: { _ in try await Task.sleep(for: .milliseconds(40)) })
        defer { session.prepareToQuit() }
        await bootstrapDetailed(session)
        session.setTextAllowedApp(monitor.editor.bundleID, true)
        try await eventually("scope persisted") { !session.textAllowedBundleIDs.isEmpty }
        session.setTextEnabled(true)
        try await eventually("text enabled") { session.textEnabled }
        session.setPeriodicEnabled(true)
        try await eventually("timer enabled") { session.periodicEnabled }
        session.startRecording()
        try await eventually("timer record saved") { session.records.contains { $0.trigger == "timer" } }
        let record = try #require(session.records.first { $0.trigger == "timer" })
        #expect(record.context?.readableText == "SYNTHETIC visible body" && record.context?.sessionActive == true)
        #expect(record.context?.onBattery == false && capture.calls == 0)
        monitor.send(.init(kind: .blocked, detail: "synthetic lock"))
        let reads = monitor.contextReads
        try await Task.sleep(for: .milliseconds(140))
        #expect(monitor.contextReads == reads)
        monitor.send(.init(kind: .resumed, detail: "synthetic unlock"))
        try await eventually("sampling resumes after unlock") { monitor.contextReads > reads }
    }

    @Test func periodicSamplingOutsideContentScopeSavesOnlyStateWithoutAXReads() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture()
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture, periodicSleep: { _ in try await Task.sleep(for: .milliseconds(40)) })
        defer { session.prepareToQuit() }
        await bootstrapDetailed(session); session.setPeriodicEnabled(true)
        try await eventually("timer enabled") { session.periodicEnabled }
        session.startRecording()
        try await eventually("state sample saved") { session.records.contains { $0.trigger == "timer" } }
        let record = try #require(session.records.first { $0.trigger == "timer" })
        #expect(record.context?.sessionActive == true && record.windowTitle.isEmpty)
        #expect(record.context?.readableText == nil && monitor.contextReads == 0 && capture.calls == 0)
    }

    @Test func bootstrapAlwaysPausesAndDoesNotObserveOrRequestPermission() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture()
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture)
        await bootstrapDetailed(session)
        #expect(!session.isRecording)
        #expect(session.statusText == "已暂停")
        #expect(monitor.starts == 0)
        #expect(capture.screenRequests == 0 && capture.accessibilityRequests == 0 && capture.calls == 0)
        #expect(session.records.isEmpty)
        session.prepareToQuit()
    }

    @Test func pauseRejectsLateSystemEventsAndLateScreenshotCallback() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture(); capture.holdsResult = true
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture, automaticCaptureAvailable: true)
        defer { session.prepareToQuit(); capture.completePending() }
        await bootstrapDetailed(session)
        try await enableImages(session, bundle: monitor.editor.bundleID)
        session.startRecording()
        try await eventually("recording started") { session.isRecording && monitor.starts == 1 }
        monitor.send(.init(kind: .application, application: monitor.editor, title: "Synthetic before pause", detail: "synthetic-before"))
        try await eventually("fake screenshot is in flight") { capture.calls == 1 && session.captureBusy }
        session.captureNow()
        #expect(session.notice?.contains("3 秒") == true)
        session.pauseRecording()
        try await eventually("pause is durable") { session.statusText == "已暂停" }
        #expect(session.notice == nil)
        let before = try await storedRecords(root)
        monitor.sendAfterStop(.init(kind: .resumed, detail: "synthetic-late-unlock"))
        monitor.sendAfterStop(.init(kind: .application, application: monitor.editor, title: "Must never persist", detail: "synthetic-late-event"))
        capture.completePending()
        try await eventually("late callback has returned") { !session.captureBusy }
        let after = try await storedRecords(root)
        #expect(Set(before.map(\.id)) == Set(after.map(\.id)))
        #expect(after.allSatisfy { $0.kind != .screenshot && $0.detail != "synthetic-late-event" })
        #expect(!session.isRecording)
        #expect(session.notice == nil)
        #expect(capture.cancellations > 0)
    }

    @Test func demoCannotStartNativeObserversPermissionsOrCapture() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture()
        let session = DesktopSession(isDemo: true, dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture)
        defer { session.prepareToQuit() }
        await bootstrapDetailed(session)
        // The synthetic day is generated relative to launch time and reaches the timeline through the outbox;
        // assert its shape, not a count.
        try await eventually("synthetic day delivered") { !session.records.isEmpty && session.statistics.screenshotCount > 0 }
        #expect(session.records.allSatisfy { $0.runID == "synthetic-demo" })
        session.requestScreenPermission(); session.requestAccessibilityPermission()
        session.startRecording()
        try await eventually("demo state started") { session.isRecording }
        session.captureNow()
        try await Task.sleep(for: .milliseconds(100))
        #expect(monitor.starts == 0)
        #expect(capture.screenRequests == 0 && capture.accessibilityRequests == 0 && capture.calls == 0)
        #expect(session.records.allSatisfy { $0.runID == "synthetic-demo" || $0.kind == .system })
    }

    @Test func preferencesBecomeEffectiveAfterSaveAndLatestEditWins() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture()
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture)
        defer { session.prepareToQuit() }
        await bootstrapDetailed(session)
        let first = monitor.editor.bundleID, second = "test.synthetic.second"
        session.setAllowedApp(first, true)
        #expect(session.allowedBundleIDs.isEmpty)
        session.setAllowedApp(first, false)
        session.setAllowedApp(second, true)
        try await eventually("latest preference is effective") { session.allowedBundleIDs == [second] }
        #expect(try DesktopPreferencesFile(directory: root).load().allowedBundleIDs == [second])
        #expect(!session.screenshotsEnabled)
    }

    @Test func narrowingScopeImmediatelyStopsNewTitleContent() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture()
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture)
        defer { session.prepareToQuit(); capture.completePending() }
        await bootstrapDetailed(session)
        session.setAllowedApp(monitor.editor.bundleID, true)
        try await eventually("initial allow saved") { session.allowedBundleIDs.contains(monitor.editor.bundleID) }
        session.startRecording()
        try await eventually("recording active") { session.isRecording && monitor.starts == 1 }
        session.setAllowedApp(monitor.editor.bundleID, false)
        // The preference Task has not had an opportunity to finish; desired
        // scope already has to protect the content of this synchronous event.
        monitor.send(.init(kind: .window, application: monitor.editor, title: "Must be stripped immediately", detail: "synthetic-scope-tightening"))
        try await eventually("event arrived") { session.records.contains { $0.detail == "synthetic-scope-tightening" } }
        let record = try #require(session.records.first { $0.detail == "synthetic-scope-tightening" })
        #expect(record.windowTitle.isEmpty)
        #expect(capture.calls == 0)
    }

    @Test func manualCountdownSurvivesSwitchingToTargetApplication() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture()
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture)
        defer { session.prepareToQuit(); capture.completePending() }
        await bootstrapDetailed(session)
        try await enableImages(session, bundle: monitor.editor.bundleID)
        session.startRecording()
        try await eventually("recording active") { session.isRecording && monitor.starts == 1 }
        session.captureNow()
        monitor.send(.init(kind: .application, application: monitor.editor, title: "Synthetic target", detail: "synthetic-manual-switch"))
        try await eventually("manual screenshot committed", timeout: .seconds(5)) { session.records.contains { $0.kind == .screenshot } }
        let screenshot = try #require(session.records.first { $0.kind == .screenshot })
        #expect(screenshot.detail.contains("用户手动触发"))
        #expect(capture.calls == 1)
        #expect(capture.capturedAllowedSets == [[monitor.editor.bundleID]])
        #expect(screenshot.imageHash != nil)
        #expect(session.notice == nil)
    }

    @Test func idleOrPausedSessionDoesNotPollMonitorOrCapture() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture()
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture)
        defer { session.prepareToQuit() }
        await bootstrapDetailed(session)
        let pausedSnapshots = monitor.snapshots
        try await Task.sleep(for: .milliseconds(300))
        #expect(monitor.snapshots == pausedSnapshots && capture.calls == 0)
        session.startRecording()
        try await eventually("recording active") { session.isRecording }
        let recordingSnapshots = monitor.snapshots
        try await Task.sleep(for: .milliseconds(300))
        #expect(monitor.snapshots == recordingSnapshots && capture.calls == 0)
        #expect(monitor.starts == 1)
    }

    @Test func deniedScreenCapabilitiesKeepMetadataUsableWithoutCapture() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture()
        monitor.screenAllowed = false; monitor.accessibilityAllowed = false
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture)
        defer { session.prepareToQuit() }
        await bootstrapDetailed(session)
        session.setAllowedApp(monitor.editor.bundleID, true)
        try await eventually("allow saved without OS privilege") { session.allowedBundleIDs.contains(monitor.editor.bundleID) }
        session.setScreenshotsEnabled(true)
        #expect(!session.screenshotsEnabled && session.error != nil)
        session.startRecording()
        try await eventually("metadata is independently available") { session.isRecording && monitor.starts == 1 }
        monitor.send(.init(kind: .application, application: monitor.editor, detail: "synthetic-metadata-without-permission"))
        try await eventually("application event saved") { session.records.contains { $0.detail == "synthetic-metadata-without-permission" } }
        #expect(capture.calls == 0 && capture.screenRequests == 0 && capture.accessibilityRequests == 0)
        #expect(try DesktopPreferencesFile(directory: root).load().screenshotEnabled == false)
    }

    @Test(arguments: [false, true])
    func disablingScreenshotsImmediatelyRejectsNewAutomaticAndManualRequests(_ manual: Bool) async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture()
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture, automaticCaptureAvailable: true)
        defer { session.prepareToQuit(); capture.completePending() }
        await bootstrapDetailed(session)
        try await enableImages(session, bundle: monitor.editor.bundleID)
        session.startRecording()
        try await eventually("recording active") { session.isRecording && monitor.starts == 1 }

        session.setScreenshotsEnabled(false)
        // No suspension has occurred since the change. Effective state has not
        // observed the asynchronous save, but desired=false must already deny.
        #expect(session.screenshotsEnabled)
        if manual {
            session.captureNow()
            #expect(session.error != nil)
        }
        monitor.send(.init(kind: .application, application: monitor.editor, title: "Synthetic after screenshot disable", detail: "synthetic-disabled-request"))
        try await eventually("disabled preference is durable") { !session.screenshotsEnabled }
        // Exercise the actual settle/countdown windows rather than merely
        // checking a setter or the count before scheduled work could run.
        try await Task.sleep(for: manual ? .milliseconds(3300) : .milliseconds(1100))
        #expect(capture.calls == 0 && session.captureAttempts == 0)
        #expect(session.isRecording)
        #expect(try await storedStatistics(root).screenshotCount == 0)
        #expect(try DesktopPreferencesFile(directory: root).load().screenshotEnabled == false)
        #expect(try await storedRecords(root).contains { $0.detail == "synthetic-disabled-request" })
    }

    @Test func disablingScreenshotsRejectsAlreadyInFlightLateResult() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture(); capture.holdsResult = true
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture, automaticCaptureAvailable: true)
        defer { session.prepareToQuit(); capture.completePending() }
        await bootstrapDetailed(session)
        try await enableImages(session, bundle: monitor.editor.bundleID)
        session.startRecording()
        try await eventually("recording active") { session.isRecording && monitor.starts == 1 }
        monitor.send(.init(kind: .application, application: monitor.editor, title: "Synthetic in flight", detail: "synthetic-before-image-disable"))
        try await eventually("capture is in flight") { capture.calls == 1 && session.captureBusy }
        session.setScreenshotsEnabled(false)
        capture.completePending()
        try await eventually("late native result returned without publication") { !session.captureBusy && !session.screenshotsEnabled }
        #expect(try await storedStatistics(root).screenshotCount == 0)
        #expect(session.records.allSatisfy { $0.kind != .screenshot })
        #expect(session.isRecording && monitor.starts == 1)
    }

    @Test func automaticCapturePreferencePersistsWhileRecordingStartsPaused() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor(), capture = SyntheticCapture()
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: capture)
        defer { session.prepareToQuit() }
        await bootstrapDetailed(session)
        try await enableImages(session, bundle: monitor.editor.bundleID)
        session.startRecording()
        try await eventually("recording active") { session.isRecording && monitor.starts == 1 }
        monitor.send(.init(kind: .application, application: monitor.editor, title: "Synthetic metadata only", detail: "synthetic-auto-not-verified"))
        try await eventually("application metadata saved") { session.records.contains { $0.detail == "synthetic-auto-not-verified" } }
        try await Task.sleep(for: .milliseconds(1100))
        #expect(capture.calls == 0 && session.captureAttempts == 0)
        #expect(session.screenshotsEnabled && session.isRecording)
        session.setAutomaticCaptureEnabled(true)
        try await eventually("enabled preference saved") { session.automaticCaptureAvailable }
        monitor.send(.init(kind: .window, application: monitor.editor, title: "Synthetic explicit opt-in", detail: "synthetic-auto-opt-in"))
        try await eventually("explicitly enabled event screenshot saved") { session.records.contains { $0.kind == .screenshot } }
        #expect(capture.calls == 1)
        session.prepareToQuit()
        let reopenedMonitor = SyntheticMonitor(), reopenedCapture = SyntheticCapture()
        let reopened = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: reopenedMonitor, capture: reopenedCapture)
        defer { reopened.prepareToQuit() }
        await reopened.bootstrap()
        #expect(!reopened.isRecording && reopened.automaticCaptureAvailable && reopenedMonitor.starts == 0)
        reopened.startRecording()
        try await eventually("reopened session started") { reopened.isRecording && reopenedMonitor.starts == 1 }
        reopenedMonitor.send(.init(kind: .window, application: reopenedMonitor.editor, title: "Synthetic resumed window"))
        try await eventually("reopened session captures without toggling") { reopened.samplingDiagnostics.imageSaved == 1 }
        reopened.setAutomaticCaptureEnabled(false)
        for _ in 0..<100 {
            if try !DesktopPreferencesFile(directory: root).load().automaticScreenshotEnabled { break }
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(try !DesktopPreferencesFile(directory: root).load().automaticScreenshotEnabled)
        reopened.prepareToQuit()
        let disabled = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: SyntheticMonitor(), capture: SyntheticCapture())
        defer { disabled.prepareToQuit() }
        await disabled.bootstrap()
        #expect(!disabled.isRecording && !disabled.automaticCaptureAvailable)
    }
}

extension DesktopSessionTests {
    @Test func r2BalancedEvidenceWritesInputChangesAndRevokesValues() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let monitor = SyntheticMonitor()
        let session = DesktopSession(dataRoot: root, core: try await TestCore.ready(root), monitor: monitor, capture: SyntheticCapture())
        defer { session.prepareToQuit() }
        await session.bootstrap()
        let bundle = monitor.editor.bundleID
        session.setTextAllowedApp(bundle, true)
        try await eventually("text scope") { session.textAllowedBundleIDs.contains(bundle) }
        session.setTextEnabled(true)
        try await eventually("text enabled") { session.textEnabled }
        session.setInputValueAllowedApp(bundle, true)
        try await eventually("explicit input scope") { (try? DesktopPreferencesFile(directory: root).load().contentOptions.inputValueBundleIDs?.contains(bundle)) == true }
        session.startRecording(); try await eventually("started") { session.isRecording }
        var context = DesktopContextSnapshot(textStatus: "captured")
        context.recordEvidence = .init(object: .init(key: "synthetic-doc", basis: "path"), field: "composer", inputValue: "SYNTHETIC first draft", inputStatus: "captured")
        monitor.send(.init(kind: .context, application: monitor.editor, title: "Synthetic doc", trigger: "content", context: context))
        try await eventually("first input persists") { session.records.contains { $0.context?.recordEvidence?.input_value == "SYNTHETIC first draft" } }
        context.recordEvidence?.input_value = "SYNTHETIC revised draft"
        let searchStarted = Date()
        monitor.send(.init(kind: .context, application: monitor.editor, title: "Synthetic doc", trigger: "content", context: context))
        let searchChannel = try await TestCore.ready(root).channel!
        var firstSearchHit: Date?
        for _ in 0..<100 {
            let found: AppActivities = try await searchChannel.app("app_activities", ["start_ms": .int(0), "end_ms": .int(Int64(Date().timeIntervalSince1970 * 1000) + 86_400_000), "q": .string("revised"), "limit": .int(100)])
            if !found.activities.isEmpty { firstSearchHit = Date(); break }
            try await Task.sleep(for: .milliseconds(20))
        }
        let hitAt = try #require(firstSearchHit)
        let ms: (Date) -> Int64 = { Int64($0.timeIntervalSince1970 * 1000) }
        let measurement: [String: Any] = ["format": "candid-measurement/1", "synthetic": true,
            "observed": [["start_ms": ms(searchStarted), "end_ms": ms(hitAt) + 1]], "context_items": [],
            "visits": NSNull(), "effective": NSNull(), "covered": NSNull(), "inputs": NSNull(), "input_records": [], "text_records": [],
            "shots": NSNull(), "resources": NSNull(), "searchable": [["occurred_ms": ms(searchStarted), "searchable_ms": ms(hitAt), "poll_interval_ms": 20]], "counts": [:]]
        try JSONSerialization.data(withJSONObject: measurement, options: [.sortedKeys, .prettyPrinted]).write(to: URL(fileURLWithPath: "/tmp/candid-capture-search-measurement.json"))
        try await eventually("second input persists despite same body") { session.records.contains { $0.context?.recordEvidence?.input_value == "SYNTHETIC revised draft" } }
        try await drained(root)
        let channel = try await TestCore.ready(root).channel!
        let hits: AppActivities = try await channel.app("app_activities", ["start_ms": .int(0), "end_ms": .int(Int64(Date().timeIntervalSince1970 * 1000) + 86_400_000), "q": .string("revised"), "limit": .int(100)])
        #expect(!hits.activities.isEmpty)
        session.setInputValueAllowedApp(bundle, false)
        try await eventually("input scope revoked") { (try? DesktopPreferencesFile(directory: root).load().contentOptions.inputValueBundleIDs?.contains(bundle)) == false }
        context.recordEvidence?.input_value = "SYNTHETIC forbidden late value"
        monitor.send(.init(kind: .context, application: monitor.editor, title: "Synthetic doc", trigger: "content", context: context))
        try await drained(root)
        let forbidden: AppActivities = try await channel.app("app_activities", ["start_ms": .int(0), "end_ms": .int(Int64(Date().timeIntervalSince1970 * 1000) + 86_400_000), "q": .string("forbidden"), "limit": .int(100)])
        #expect(forbidden.activities.isEmpty)
    }
}

extension DesktopSessionTests {
    @Test func explicitWindowAssociationStaysWithinRuntimeWindowAndCanBeCleared() async throws {
        let root = try syntheticSessionRoot(); defer { TestCore.finish(root) }
        let core = try await TestCore.ready(root)
        let channel = try #require(core.channel)
        let folder = root.appendingPathComponent("synthetic-session-source")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let stamp = Date().ISO8601Format()
        let rows: [[String: Any]] = [
            ["timestamp":stamp,"type":"session_meta","payload":["id":"synthetic-bound-window","cwd":"/SYNTHETIC"]],
            ["timestamp":stamp,"type":"response_item","payload":["type":"message","id":"u1","role":"user","source":"user","content":[["type":"input_text","text":"SYNTHETIC source message"]]]]
        ]
        let bytes = try rows.map { String(decoding: try JSONSerialization.data(withJSONObject: $0), as: UTF8.self) }.joined(separator: "\n") + "\n"
        try bytes.write(to: folder.appendingPathComponent("synthetic.jsonl"), atomically: true, encoding: .utf8)
        var consent = TimelineConnectors.AgentConsent(agent: "codex"); consent.read = true; consent.activity = true; consent.paths = [folder.path]
        let agents = TimelineConnectors.AgentCoreClient(channel: channel); try await agents.save(consent); try await channel.syncNow()
        let monitor = SyntheticMonitor()
        let session = DesktopSession(dataRoot: root, core: core, monitor: monitor, capture: SyntheticCapture())
        defer { session.prepareToQuit() }
        await bootstrapDetailed(session)
        session.setTextAllowedApp(monitor.editor.bundleID, true)
        try await eventually("text app saved") { session.textAllowedBundleIDs.contains(monitor.editor.bundleID) }
        session.setTextEnabled(true)
        try await eventually("text enabled") { session.textEnabled }
        session.setInputValueAllowedApp(monitor.editor.bundleID, true)
        try await eventually("input consent saved") { session.contentOptions.inputValueBundleIDs?.contains(monitor.editor.bundleID) == true }
        session.startRecording(); try await eventually("capture started") { session.isRecording }
        func send(_ field: String, _ text: String) {
            var context = DesktopContextSnapshot(textStatus: "captured")
            context.recordEvidence = .init(object: .init(key: "https://synthetic.invalid/chat", basis: "url"), field: field, inputValue: text, inputStatus: "captured")
            monitor.send(.init(kind: .context, application: monitor.editor, title: "SYNTHETIC chat", trigger: "mouse", context: context, windowIdentity: field.contains("100") ? 100 : 200))
        }
        send("42:100:composer", "SYNTHETIC draft one")
        try await eventually("window choice") { !session.inputWindows.isEmpty }
        let first = try #require(session.inputWindows.first?.id)
        #expect(await session.associateWindow(first, with: "agent:codex:synthetic-bound-window"))
        send("42:100:composer", "SYNTHETIC draft two")
        var bound = false
        for _ in 0..<100 {
            let records = try await storedRecords(root)
            bound = records.contains { $0.context?.recordEvidence?.input_value == "SYNTHETIC draft two" && $0.context?.recordEvidence?.object.key == "agent:codex:synthetic-bound-window" }
            if bound { break }; try await Task.sleep(for: .milliseconds(50))
        }
        #expect(bound)
        send("42:200:composer", "SYNTHETIC draft two")
        try await eventually("different window stays unbound") { session.inputWindows.contains { $0.record.context?.recordEvidence?.field == "42:200:composer" } }
        var otherWindow = false
        for _ in 0..<100 {
            let records = try await storedRecords(root)
            otherWindow = records.contains { $0.context?.recordEvidence?.field == "42:200:composer" && $0.context?.recordEvidence?.input_value == "SYNTHETIC draft two" && $0.context?.recordEvidence?.object.key == "https://synthetic.invalid/chat" }
            if otherWindow { break }; try await Task.sleep(for: .milliseconds(50))
        }
        #expect(otherWindow)
        session.clearWindowLinks()
        send("42:100:composer", "SYNTHETIC after clear")
        var cleared = false
        for _ in 0..<100 {
            let records = try await storedRecords(root)
            cleared = records.contains { $0.context?.recordEvidence?.input_value == "SYNTHETIC after clear" && $0.context?.recordEvidence?.object.key == "https://synthetic.invalid/chat" }
            if cleared { break }; try await Task.sleep(for: .milliseconds(50))
        }
        #expect(cleared)
        session.pauseRecording()
        #expect(session.inputWindows.isEmpty)
        #expect(!(await session.associateWindow(first, with: "agent:codex:synthetic-bound-window")))
    }
}
