import Foundation

public enum CaptureAdmissionError: Error, LocalizedError {
    case staleEpoch
    public var errorDescription: String? { "采集范围已变化，已丢弃旧截图结果。" }
}

/// The publication boundary is serialized with pause/scope changes, not just
/// checked before an asynchronous write. A pause cannot acknowledge too early.
public final class CaptureCommitGate: @unchecked Sendable {
    private let lock = NSLock()
    private var epoch: UInt64 = 0
    private var enabled = false
    public init() {}
    @discardableResult public func enable() -> UInt64 {
        lock.lock(); defer { lock.unlock() }
        epoch &+= 1; enabled = true; return epoch
    }
    public func invalidate() {
        lock.lock(); defer { lock.unlock() }
        epoch &+= 1; enabled = false
    }
    public func currentEpoch() -> UInt64 {
        lock.lock(); defer { lock.unlock() }; return epoch
    }
    public func permits(_ expected: UInt64) -> Bool {
        lock.lock(); defer { lock.unlock() }; return enabled && epoch == expected
    }
    public func beginAdmission(epoch expected: UInt64) throws {
        lock.lock()
        guard enabled && epoch == expected else { lock.unlock(); throw CaptureAdmissionError.staleEpoch }
    }
    public func endAdmission() { lock.unlock() }
    public func withAdmission<T>(epoch expected: UInt64, _ operation: () throws -> T) throws -> T {
        lock.lock(); defer { lock.unlock() }
        guard enabled && epoch == expected else { throw CaptureAdmissionError.staleEpoch }
        return try operation()
    }
}

public struct CaptureRateLimiter: Sendable {
    private var attempts: [(time: Double, automatic: Bool, source: String?)] = []
    private var lastAttempt: Double?
    private var lastBySource: [String: Double] = [:]
    public init() {}
    public func nextAvailableDelay(now: Double, automatic: Bool, interval: Double = 10, lowPower: Bool = false, automaticCapacity: Int = 6, source: String? = nil) -> Double {
        let active = attempts.filter { now >= $0.time && now - $0.time < 60 }
        var deadline = now
        let configuredCapacity = min(60, max(1, automaticCapacity))
        let sharedCapacity = max(12, configuredCapacity)
        if active.count >= sharedCapacity { deadline = max(deadline, active[active.count - sharedCapacity].time + 60) }
        let scopedSource = automatic && !lowPower ? source : nil
        let latest = if let scopedSource { lastBySource[scopedSource] } else { lastAttempt }
        if let latest, latest <= now {
            deadline = max(deadline, latest + (automatic ? (lowPower ? max(30, interval) : max(1, interval)) : 2))
        }
        let same = active.filter { $0.automatic == automatic && (scopedSource == nil || $0.source == scopedSource) }
        let capacity = automatic ? (lowPower ? min(2, configuredCapacity) : configuredCapacity) : 12
        if same.count >= capacity { deadline = max(deadline, same[same.count - capacity].time + 60) }
        return max(0, deadline - now)
    }
    public mutating func admit(now: Double, automatic: Bool, interval: Double = 10, lowPower: Bool = false, automaticCapacity: Int = 6, source: String? = nil) -> Bool {
        if let lastAttempt, now < lastAttempt { self.lastAttempt = nil }
        attempts.removeAll { now - $0.time >= 60 || now < $0.time }
        lastBySource = lastBySource.filter { now - $0.value <= 300 && now >= $0.value }
        guard nextAvailableDelay(now: now, automatic: automatic, interval: interval, lowPower: lowPower, automaticCapacity: automaticCapacity, source: source) == 0 else { return false }
        attempts.append((now, automatic, source)); lastAttempt = now
        if let source { lastBySource[source] = now }
        return true
    }
}
