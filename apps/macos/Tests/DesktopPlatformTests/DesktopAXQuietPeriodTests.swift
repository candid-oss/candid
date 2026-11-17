import Testing
@testable import DesktopPlatform

struct DesktopAXQuietPeriodTests {
    @Test func eightSecondInputBurstNeverBecomesPeriodicSampling() {
        var policy = DesktopAXQuietPeriod()
        for index in 0...20 {
            let now = Double(index) * 0.4
            policy.note(trigger: index.isMultiple(of: 2) ? "content" : "selection", at: now)
            #expect(policy.takeReady(at: now + 0.39) == nil)
        }
        #expect(policy.isTextSettling(at: 9.49))
        #expect(!policy.isTextSettling(at: 9.5))
        #expect(policy.takeReady(at: 9.49) == nil)
        #expect(policy.takeReady(at: 9.5)?.trigger == "content")
        #expect(policy.takeReady(at: 30) == nil)
    }

    @Test func lastTextNotificationRestartsQuietPeriodAndKeepsLatestTrigger() {
        var policy = DesktopAXQuietPeriod()
        policy.note(trigger: "content", at: 0)
        policy.note(trigger: "selection", at: 1.4)
        #expect(policy.takeReady(at: 1.5) == nil)
        #expect(policy.takeReady(at: 2.89) == nil)
        #expect(policy.takeReady(at: 2.9)?.trigger == "selection")
    }

    @Test func focusAndWindowKeepShortDelayButNeverReplacePendingTextTrigger() {
        var policy = DesktopAXQuietPeriod()
        policy.note(trigger: "window", at: 0)
        #expect(policy.takeReady(at: 0.299) == nil)
        #expect(policy.takeReady(at: 0.3)?.trigger == "window")
        policy.note(trigger: "content", at: 1)
        policy.note(trigger: "focus", at: 1.2)
        #expect(policy.takeReady(at: 1.5) == nil)
        policy.note(trigger: "window", at: 2.49)
        #expect(policy.takeReady(at: 2.5)?.trigger == "content")
    }

    @Test func pauseOrScopeRevocationDiscardsPendingAndLateRead() throws {
        var policy = DesktopAXQuietPeriod()
        policy.note(trigger: "content", at: 0)
        policy.cancel()
        #expect(!policy.isTextSettling(at: 10))
        #expect(policy.remaining(at: 10) == nil)
        #expect(policy.takeReady(at: 10) == nil)
        policy.note(trigger: "selection", at: 20)
        let ready = policy.takeReady(at: 21.5)
        let inFlight = try #require(ready)
        #expect(policy.isCurrent(inFlight))
        policy.cancel()
        #expect(!policy.isCurrent(inFlight))
        policy.note(trigger: "focus", at: 30)
        #expect(policy.takeReady(at: 30.3)?.trigger == "focus")
        #expect(!policy.isCurrent(inFlight))
    }

    @Test func notificationDuringReadInvalidatesItsResultAndSchedulesLatestOnly() throws {
        var policy = DesktopAXQuietPeriod()
        policy.note(trigger: "content", at: 0)
        let ready = policy.takeReady(at: 1.5)
        let reading = try #require(ready)
        policy.note(trigger: "content", at: 1.6)
        policy.note(trigger: "selection", at: 1.8)
        #expect(!policy.isCurrent(reading))
        #expect(policy.takeReady(at: 3.29) == nil)
        #expect(policy.takeReady(at: 3.3)?.trigger == "selection")
    }
}

extension DesktopAXQuietPeriodTests {
    @Test func continuousChangesSaveAtTenSecondsThenStartAnotherBurst() {
        var policy = DesktopAXQuietPeriod()
        for n in 0..<100 { policy.note(trigger: "content", at: Double(n) / 10) }
        #expect(policy.takeReady(at: 9.99) == nil)
        #expect(policy.takeReady(at: 10)?.trigger == "content")
        policy.note(trigger: "content", at: 10.1)
        #expect(policy.takeReady(at: 11.6)?.trigger == "content")
    }
}

extension DesktopAXQuietPeriodTests {
    @Test func configurableContinuousDeadlineStillSurvivesSelectionNoise() {
        var policy = DesktopAXQuietPeriod(); policy.maximumWait = 5
        for n in 0..<50 { policy.note(trigger: "content", at: Double(n) / 10) }
        #expect(policy.takeReady(at: 5)?.trigger == "content")
    }
}
