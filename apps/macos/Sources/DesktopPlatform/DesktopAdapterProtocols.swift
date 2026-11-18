import DesktopCore
import Foundation

/// Narrow injectable boundary: orchestration tests need no OS observers or TCC.
@MainActor
public protocol DesktopMonitoring: AnyObject {
    func snapshot() -> DesktopSystemSnapshot
    func currentContextState() -> DesktopContextSnapshot
    func applications() -> [DesktopApplication]
    func start(onEvent: @escaping @MainActor (DesktopSystemEvent) -> Void)
    func stop()
    func setWindowContentScope(allowedBundleIDs: Set<String>)
    func setTextContentScope(allowedBundleIDs: Set<String>)
    func readContext(allowedBundleIDs: Set<String>, includeText: Bool) async throws -> DesktopContextReading
    var frontmostApplication: DesktopApplication? { get }
    var isSessionActive: Bool { get }
    var isContentSettling: Bool { get }
    var contextRevision: UInt64 { get }
    func setInteractionTriggerSelection(returnEnabled: Bool, scrollEnabled: Bool)
    func addApplication(url: URL) -> Bool
    func refreshApplicationCatalog() async
    func setContentOptions(_ options: DesktopContentOptions)
    func setSamplingPolicy(balanced: Bool)
    var inputMonitoringAuthorized: Bool { get }
    var interactionTriggersActive: Bool { get }
    var interactionTriggerStatus: String { get }
    @discardableResult func setInteractionTriggersEnabled(_ enabled: Bool) -> Bool
    @discardableResult func requestInputMonitoringPermission() -> Bool
}

extension DesktopMonitoring {
    public var isContentSettling: Bool { false }
    public var contextRevision: UInt64 { 0 }
    public func setInteractionTriggerSelection(returnEnabled: Bool, scrollEnabled: Bool) {}
    public func addApplication(url: URL) -> Bool { false }
    public func refreshApplicationCatalog() async {}
    public func setContentOptions(_ options: DesktopContentOptions) {}
    public func setSamplingPolicy(balanced: Bool) {}
    public var inputMonitoringAuthorized: Bool { false }
    public var interactionTriggersActive: Bool { false }
    public var interactionTriggerStatus: String { "disabled" }
    @discardableResult public func setInteractionTriggersEnabled(_ enabled: Bool) -> Bool { !enabled }
    @discardableResult public func requestInputMonitoringPermission() -> Bool { false }
    public func currentContextState() -> DesktopContextSnapshot {
        let state = snapshot()
        return .init(sessionActive: isSessionActive, onBattery: state.onBattery, lowPower: state.lowPower)
    }
    public func setWindowContentScope(allowedBundleIDs: Set<String>) {}
    public func setTextContentScope(allowedBundleIDs: Set<String>) {}
    public func readContext(allowedBundleIDs: Set<String>, includeText: Bool) async throws -> DesktopContextReading { throw DesktopCaptureError.unavailable }
}

@MainActor
public protocol DesktopCapturing: AnyObject {
    @discardableResult func requestScreenPermission() -> Bool
    @discardableResult func requestAccessibilityPermission() -> Bool
    func setImageOptions(_ options: DesktopImageOptions)
    func setImageEncodingReuseEnabled(_ enabled: Bool)
    func setRawFrameDelivery(_ enabled: Bool)
    func cancelPendingCapture()
    func captureFrontmost(allowedBundleIDs: Set<String>, windowCompatibilityBundleIDs: Set<String>,
                          axTextBundleIDs: Set<String>, includeAXFocus: Bool) async throws -> CapturedWindow
}

extension DesktopSystemMonitor: DesktopMonitoring {}
extension DesktopCaptureProvider: DesktopCapturing {}

extension DesktopCapturing {
    public func setImageOptions(_ options: DesktopImageOptions) {}
    public func setImageEncodingReuseEnabled(_ enabled: Bool) {}
    public func setRawFrameDelivery(_ enabled: Bool) {}
}
