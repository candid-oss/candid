import AppKit
import DesktopCore
import Foundation
import IOKit.ps

public enum DesktopRecognitionPowerSource {
    public static func read() -> DesktopRecognitionPower {
        let process = ProcessInfo.processInfo
        var external: Bool?
        var internalBattery: Bool?
        var percent: Int?
        if let info = IOPSCopyPowerSourcesInfo()?.takeRetainedValue() {
            let source = IOPSGetProvidingPowerSourceType(info)?.takeUnretainedValue() as String?
            if source == kIOPSACPowerValue { external = true }
            if source == kIOPSBatteryPowerValue { external = false }
            if let list = IOPSCopyPowerSourcesList(info)?.takeRetainedValue() as? [AnyObject] {
                internalBattery = false
                for item in list {
                    guard let fields = IOPSGetPowerSourceDescription(info, item)?.takeUnretainedValue() as? [String: Any]
                    else { internalBattery = nil; continue }
                    guard fields[kIOPSTypeKey] as? String == kIOPSInternalBatteryType else { continue }
                    internalBattery = true
                    if let current = fields[kIOPSCurrentCapacityKey] as? NSNumber,
                       let maximum = fields[kIOPSMaxCapacityKey] as? NSNumber, maximum.doubleValue > 0 {
                        percent = Int((100 * current.doubleValue / maximum.doubleValue).rounded(.down))
                    }
                    break
                }
            }
        }
        return DesktopRecognitionPower(onExternalPower: external, hasInternalBattery: internalBattery, batteryPercent: percent,
            lowPower: process.isLowPowerModeEnabled, thermalBlocked: [.serious, .critical].contains(process.thermalState))
    }
}

/// IOKit delivers power-source changes on the main run loop.
public final class DesktopRecognitionPowerObservation {
    private let source: CFRunLoopSource?
    private let handler: Handler
    private let observers: [(NotificationCenter, NSObjectProtocol)]

    private final class Handler {
        let callback: @MainActor @Sendable (Bool) -> Void
        init(_ callback: @escaping @MainActor @Sendable (Bool) -> Void) { self.callback = callback }
    }

    @MainActor public init(_ callback: @escaping @MainActor @Sendable (Bool) -> Void) {
        let handler = Handler(callback)
        self.handler = handler
        source = IOPSNotificationCreateRunLoopSource({ context in
            guard let context else { return }
            let handler = Unmanaged<Handler>.fromOpaque(context).takeUnretainedValue()
            MainActor.assumeIsolated { handler.callback(false) }
        }, Unmanaged.passUnretained(handler).toOpaque())?.takeRetainedValue()
        let workspace = NSWorkspace.shared.notificationCenter
        observers = [
            (NotificationCenter.default, NotificationCenter.default.addObserver(
                forName: ProcessInfo.thermalStateDidChangeNotification, object: nil, queue: .main
            ) { _ in Task { @MainActor in callback(false) } }),
            (NotificationCenter.default, NotificationCenter.default.addObserver(
                forName: Notification.Name.NSProcessInfoPowerStateDidChange, object: nil, queue: .main
            ) { _ in Task { @MainActor in callback(false) } }),
            (workspace, workspace.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { _ in
                Task { @MainActor in callback(true) }
            })
        ]
        if let source { CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes) }
    }

    deinit {
        for (center, observer) in observers { center.removeObserver(observer) }
        if let source { CFRunLoopRemoveSource(CFRunLoopGetMain(), source, .commonModes) }
    }
}
