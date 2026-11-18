import Foundation
import Testing
@testable import DesktopCore

@Test func staleEpochNeverExecutesPublication() throws {
    let gate = CaptureCommitGate(), epoch = gate.enable()
    gate.invalidate()
    var published = false
    #expect(throws: CaptureAdmissionError.self) { try gate.withAdmission(epoch: epoch) { published = true } }
    #expect(!published)
    let resumed = gate.enable()
    #expect(resumed != epoch)
    #expect(!gate.permits(epoch)); #expect(gate.permits(resumed))
}

@Test func pauseWaitsForPublicationBoundaryAndBlocksLaterWork() async throws {
    let gate = CaptureCommitGate(), epoch = gate.enable()
    let entered = DispatchSemaphore(value: 0), release = DispatchSemaphore(value: 0)
    let publication = Task.detached {
        try gate.withAdmission(epoch: epoch) { entered.signal(); release.wait() }
    }
    #expect(waitForSignal(entered))
    let paused = Task.detached { gate.invalidate(); return true }
    release.signal()
    try await publication.value
    #expect(await paused.value)
    #expect(!gate.permits(epoch))
    #expect(throws: CaptureAdmissionError.self) { try gate.beginAdmission(epoch: epoch) }
}

@Test func automaticRateLimitIncludesFailedAttemptsAndHasRollingWindow() {
    var limiter = CaptureRateLimiter()
    expectAdmission(limiter.admit(now: 0, automatic: true), true)
    expectAdmission(limiter.admit(now: 0.75, automatic: true), false)
    for second in stride(from: 10, through: 50, by: 10) { expectAdmission(limiter.admit(now: Double(second), automatic: true), true) }
    expectAdmission(limiter.admit(now: 59, automatic: true), false)
    expectAdmission(limiter.admit(now: 60, automatic: true), true)
}

@Test func batteryAndManualLimitsCannotBypassBudgets() {
    var battery = CaptureRateLimiter()
    expectAdmission(battery.admit(now: 0, automatic: true, lowPower: true), true)
    expectAdmission(battery.admit(now: 10, automatic: true, lowPower: true), false)
    expectAdmission(battery.admit(now: 30, automatic: true, lowPower: true), true)
    expectAdmission(battery.admit(now: 50, automatic: true, lowPower: true), false)
    var manual = CaptureRateLimiter()
    for second in stride(from: 0, through: 22, by: 2) { expectAdmission(manual.admit(now: Double(second), automatic: false), true) }
    expectAdmission(manual.admit(now: 24, automatic: false), false)
    expectAdmission(manual.admit(now: 61, automatic: false), true)
}

@Test func deferredCaptureCanWaitForCapacityWithoutConsumingAnotherAttempt() {
    var limiter = CaptureRateLimiter()
    expectAdmission(limiter.admit(now: 0, automatic: true), true)
    #expect(limiter.nextAvailableDelay(now: 1, automatic: true) == 9)
    #expect(limiter.nextAvailableDelay(now: 1, automatic: true) == 9)
    expectAdmission(limiter.admit(now: 10, automatic: true), true)
    for second in stride(from: 20, through: 50, by: 10) { expectAdmission(limiter.admit(now: Double(second), automatic: true), true) }
    #expect(limiter.nextAvailableDelay(now: 55, automatic: true) == 5)
    #expect(limiter.nextAvailableDelay(now: 55, automatic: true, lowPower: true) == 45)
    expectAdmission(limiter.admit(now: 60, automatic: true), true)
}

@Test func perWindowAdmissionKeepsTheSharedCeilingAndBatteryLimit() {
    var limiter = CaptureRateLimiter()
    for window in 0..<12 {
        expectAdmission(limiter.admit(now: Double(window), automatic: true, interval: 15, automaticCapacity: 4, source: "window-\(window)"), true)
    }
    expectAdmission(limiter.admit(now: 12, automatic: true, interval: 15, automaticCapacity: 4, source: "window-12"), false)
    expectAdmission(limiter.admit(now: 60, automatic: true, interval: 15, automaticCapacity: 4, source: "window-12"), true)

    var battery = CaptureRateLimiter()
    expectAdmission(battery.admit(now: 0, automatic: true, interval: 15, lowPower: true, automaticCapacity: 4, source: "first"), true)
    expectAdmission(battery.admit(now: 15, automatic: true, interval: 15, lowPower: true, automaticCapacity: 4, source: "second"), false)
    expectAdmission(battery.admit(now: 30, automatic: true, interval: 15, lowPower: true, automaticCapacity: 4, source: "second"), true)
}

private func expectAdmission(_ actual: Bool, _ expected: Bool) { #expect(actual == expected) }
/// Returns as soon as the signal arrives; the long limit only absorbs a slow, busy CI runner.
private func waitForSignal(_ semaphore: DispatchSemaphore) -> Bool { semaphore.wait(timeout: .now()+10) == .success }
