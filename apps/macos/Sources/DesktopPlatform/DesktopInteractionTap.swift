import CoreGraphics
import Foundation

/// Optional listen-only tap. Never asks for permission, changes an event,
/// extracts Unicode/characters, or retains CGEvents. Only Return keycodes and
/// scroll activity are converted into bounded candidate signals.
@MainActor
final class DesktopInteractionTap {
    private var tap: CFMachPort?
    private var source: CFRunLoopSource?
    private var scrollTask: Task<Void, Never>?
    private var scrollGeneration: UInt64 = 0
    private var signals = DesktopInteractionSignals()
    private var onSignal: (@MainActor (String) -> Void)?
    private var onFailure: (@MainActor (String) -> Void)?
    private static weak var active: DesktopInteractionTap?
    private(set) var status = "disabled"
    var isActive: Bool { tap.map { CGEvent.tapIsEnabled(tap: $0) } ?? false }

    func start(returnEnabled: Bool = true, scrollEnabled: Bool = true, onSignal: @escaping @MainActor (String) -> Void, onFailure: @escaping @MainActor (String) -> Void) -> Bool {
        stop()
        guard CGPreflightListenEventAccess() else { status = "permission_denied"; return false }
        self.onSignal = onSignal; self.onFailure = onFailure
        let mask = (returnEnabled ? CGEventMask(1) << CGEventType.keyDown.rawValue : 0) |
            (scrollEnabled ? CGEventMask(1) << CGEventType.scrollWheel.rawValue : 0) |
            (returnEnabled ? CGEventMask(1) << CGEventType.leftMouseDown.rawValue : 0)
        guard mask != 0 else { return false }
        guard let created = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .tailAppendEventTap, options: .listenOnly, eventsOfInterest: mask, callback: { _, type, event, _ in
            // Source is installed exclusively on the main run loop. Filter the
            // scalar keycode before consulting the app; never request its text.
            MainActor.assumeIsolated {
                guard let owner = DesktopInteractionTap.active else { return }
                if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput { owner.disabled(); return }
                if type == .keyDown {
                    let key = event.getIntegerValueField(.keyboardEventKeycode)
                    guard key == 36 || key == 76 else { return }
                    owner.returnPressed(key: key, isRepeat: event.getIntegerValueField(.keyboardEventAutorepeat) != 0)
                } else if type == .scrollWheel { owner.scrolled() }
                else if type == .leftMouseDown { owner.onSignal?("mouse") }
            }
            return Unmanaged.passUnretained(event)
        }, userInfo: nil), let runSource = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, created, 0) else {
            status = "unavailable"; self.onSignal = nil; self.onFailure = nil; return false
        }
        tap = created; source = runSource; Self.active = self
        CFRunLoopAddSource(CFRunLoopGetMain(), runSource, .commonModes)
        CGEvent.tapEnable(tap: created, enable: true)
        status = "active"
        return true
    }

    func stop() {
        discardPending()
        if let source { CFRunLoopRemoveSource(CFRunLoopGetMain(), source, .commonModes) }
        if let tap { CGEvent.tapEnable(tap: tap, enable: false); CFMachPortInvalidate(tap) }
        source = nil; tap = nil; onSignal = nil; onFailure = nil; status = "disabled"
        if Self.active === self { Self.active = nil }
    }

    /// Scope, app and session changes must discard a scroll tail from the old
    /// source, while retaining the already-authorized passive tap.
    func discardPending() { scrollGeneration &+= 1; scrollTask?.cancel(); scrollTask = nil; signals.cancel() }

    private func disabled() {
        let failure = CGPreflightListenEventAccess() ? "system_disabled" : "permission_denied"
        let callback = onFailure
        stop(); status = failure
        callback?(failure)
    }
    private func returnPressed(key: Int64, isRepeat: Bool) {
        guard CGPreflightListenEventAccess() else { disabled(); return }
        if signals.noteReturn(keyCode: key, isRepeat: isRepeat, at: ProcessInfo.processInfo.systemUptime) { onSignal?("return") }
    }
    private func scrolled() {
        // Check TCC once per scroll burst, not once per wheel tick.
        if scrollTask == nil, !CGPreflightListenEventAccess() { disabled(); return }
        signals.noteScroll(at: ProcessInfo.processInfo.systemUptime)
        guard scrollTask == nil else { return }
        let generation = scrollGeneration
        scrollTask = Task { [weak self] in
            guard let self else { return }
            while !Task.isCancelled, generation == self.scrollGeneration, let remaining = self.signals.scrollRemaining(at: ProcessInfo.processInfo.systemUptime) {
                if remaining > 0 {
                    do { try await Task.sleep(for: .seconds(remaining)) } catch { break }
                    continue
                }
                if self.signals.takeScroll(at: ProcessInfo.processInfo.systemUptime) {
                    guard self.isActive, CGPreflightListenEventAccess() else { self.disabled(); break }
                    self.onSignal?("scroll")
                }
                break
            }
            if generation == self.scrollGeneration { self.scrollTask = nil }
        }
    }
}
