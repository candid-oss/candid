/// Lightweight activity state; recording text activity allocates no task and
/// requires no AX calls. Balanced sampling consumes this only as a hint.
struct DesktopSamplingActivity: Sendable {
    private var textQuietUntil: Double?
    mutating func noteText(at now: Double) { textQuietUntil = now + 1.5 }
    func isSettling(at now: Double) -> Bool { textQuietUntil.map { now < $0 } ?? false }
    mutating func cancel() { textQuietUntil = nil }
    static func isTextSignal(_ trigger: String) -> Bool { trigger == "content" || trigger == "selection" }
}

struct DesktopInteractionSignals: Sendable {
    private var scrollDeadline: Double?
    private var lastReturn: Double?
    mutating func noteReturn(keyCode: Int64, isRepeat: Bool, at now: Double) -> Bool {
        guard [36, 76].contains(keyCode), !isRepeat else { return false }
        guard lastReturn.map({ now - $0 >= 0.5 }) ?? true else { return false }
        lastReturn = now
        return true
    }
    mutating func noteScroll(at now: Double) { scrollDeadline = now + 1 }
    func scrollRemaining(at now: Double) -> Double? { scrollDeadline.map { max(0, $0 - now) } }
    mutating func takeScroll(at now: Double) -> Bool {
        guard let deadline = scrollDeadline, now >= deadline else { return false }
        scrollDeadline = nil
        return true
    }
    mutating func cancel() { scrollDeadline = nil; lastReturn = nil }
}

/// Content revisions and cursor focus are activity, not source identity. Only
/// balanced mode may ignore their generation; source and safety checks remain.
enum DesktopContextSourcePolicy {
    static func accepts(expectedWindow: UInt, actualWindow: UInt, expectedPID: Int32, actualPID: Int32?, ownerUnchanged: Bool, generationUnchanged: Bool, balanced: Bool, allowed: Bool, active: Bool) -> Bool {
        expectedWindow == actualWindow && expectedPID == actualPID && ownerUnchanged && allowed && active && (balanced || generationUnchanged)
    }
}


enum DesktopSamplingSignalPolicy {
    static func isActivityOnly(balanced: Bool, trigger: String) -> Bool {
        balanced && DesktopSamplingActivity.isTextSignal(trigger)
    }
    static func readsEventText(balanced: Bool, textAllowed: Bool) -> Bool { !balanced && textAllowed }
}
