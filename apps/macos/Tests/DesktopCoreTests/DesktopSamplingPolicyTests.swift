import Foundation
import Testing
@testable import DesktopCore

@Suite
struct DesktopSamplingPolicyTests {
    private func syntheticRecord(text: String? = "金额 ¥100.00\nif ready:\n    save(\"A\")", selection: String? = "ready") -> DesktopRecord {
        DesktopRecord(kind:.context,bundleID:"test.synthetic.editor",windowTitle:"Synthetic document",context:.init(focusedRole:"AXTextArea",focusedLabel:"Synthetic editor",selectedText:selection,readableText:text,textStatus:"available",isProtected:false,onBattery:false,lowPower:false,thermalState:"nominal",idleSeconds:0))
    }

    @Test func backgroundStateAndFocusLabelsDoNotInventDocumentChanges() throws {
        let initial = syntheticRecord()
        var changedState = initial
        var context = try #require(changedState.context)
        context.idleSeconds = 47
        context.onBattery = true; context.lowPower = true; context.thermalState = "serious"
        context.focusedLabel = "Another synthetic focus label"; context.focusedRole = "AXButton"
        changedState.context = context
        changedState.id = "another-observation"; changedState.timestamp = Date(timeIntervalSince1970:200)
        #expect(DesktopContentSignature.make(initial) == DesktopContentSignature.make(changedState))
    }

    @Test func realDocumentChangesPreserveCharactersAmountsIndentationAndSelection() {
        let initial = syntheticRecord()
        let original = DesktopContentSignature.make(initial)
        let variants = [
            syntheticRecord(text:"金额 ¥100.00\nif ready:\n    save(\"B\")"),
            syntheticRecord(text:"金额 ¥1000.00\nif ready:\n    save(\"A\")"),
            syntheticRecord(text:"金额 ¥100.00\nif ready:\n  save(\"A\")"),
            syntheticRecord(selection:"save")
        ]
        for record in variants { #expect(DesktopContentSignature.make(record) != original) }
        #expect(Set(variants.map(DesktopContentSignature.make)).count == variants.count)
    }

    @Test func missingAndExplicitlyEmptyTextOrSelectionAreDistinctObservations() {
        let records = [syntheticRecord(text:nil,selection:nil),syntheticRecord(text:"",selection:nil),syntheticRecord(text:nil,selection:""),syntheticRecord(text:"",selection:"")]
        #expect(Set(records.map(DesktopContentSignature.make)).count == 4)
    }

    @Test func sourceFingerprintsSeparateWindowProcessAndUnavailableIdentity() {
        let source = DesktopContentSignature.source(bundleID:"test.synthetic.editor",pid:501,windowIdentity:11,title:"Synthetic document")
        let alternatives = [
            DesktopContentSignature.source(bundleID:"test.synthetic.editor",pid:502,windowIdentity:11,title:"Synthetic document"),
            DesktopContentSignature.source(bundleID:"test.synthetic.editor",pid:501,windowIdentity:12,title:"Synthetic document"),
            DesktopContentSignature.source(bundleID:"test.synthetic.other",pid:501,windowIdentity:11,title:"Synthetic document"),
            DesktopContentSignature.source(bundleID:"test.synthetic.editor",pid:501,windowIdentity:11,title:"Other synthetic document"),
            DesktopContentSignature.source(bundleID:"test.synthetic.editor",pid:nil,windowIdentity:nil,title:"Synthetic document"),
            DesktopContentSignature.source(bundleID:"test.synthetic.editor",pid:0,windowIdentity:0,title:"Synthetic document")
        ]
        #expect(Set([source] + alternatives).count == 7)
        #expect(source == DesktopContentSignature.source(bundleID:"test.synthetic.editor",pid:501,windowIdentity:11,title:"Synthetic document"))
        for hash in [source,DesktopContentSignature.make(syntheticRecord())] + alternatives {
            #expect(hash.utf8.count == 64)
            #expect(hash.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) })
        }
    }

    @Test func legacyPreferencesStartBalancedWithoutInteractionTriggers() throws {
        let legacy = Data(#"{"screenshotEnabled":true,"minimumCaptureInterval":30,"retentionDays":14,"textEnabled":true}"#.utf8)
        let preferences = try JSONDecoder().decode(DesktopPreferences.self,from:legacy)
        #expect(preferences.samplingMode == .balanced && !preferences.interactionTriggersEnabled)
        #expect(preferences.screenshotEnabled && preferences.minimumCaptureInterval == 30)
        #expect(preferences.retentionDays == 14 && preferences.textEnabled)
        var explicit = preferences
        explicit.samplingMode = .detailed; explicit.interactionTriggersEnabled = true
        #expect(try JSONDecoder().decode(DesktopPreferences.self,from:JSONEncoder().encode(explicit)) == explicit)
    }

    @Test func explicitFastImagePolicyUsesIntervalAndCapacityWithoutHiddenTenSecondFloor() {
        var limiter = CaptureRateLimiter()
        expectSamplingAdmission(limiter.admit(now: 0, automatic: true, interval: 2, automaticCapacity: 3), expected: true)
        expectSamplingAdmission(limiter.admit(now: 1.99, automatic: true, interval: 2, automaticCapacity: 3), expected: false)
        expectSamplingAdmission(limiter.admit(now: 2, automatic: true, interval: 2, automaticCapacity: 3), expected: true)
        expectSamplingAdmission(limiter.admit(now: 4, automatic: true, interval: 2, automaticCapacity: 3), expected: true)
        expectSamplingAdmission(limiter.admit(now: 6, automatic: true, interval: 2, automaticCapacity: 3), expected: false)
        #expect(limiter.nextAvailableDelay(now: 6, automatic: true, interval: 2, automaticCapacity: 3) == 54)
    }

    @Test func longConfiguredIntervalSurvivesRollingMinuteExpiry() {
        var limiter = CaptureRateLimiter()
        expectSamplingAdmission(limiter.admit(now: 0, automatic: true, interval: 300), expected: true)
        for second in [60.0, 120, 299.999] {
            expectSamplingAdmission(limiter.admit(now: second, automatic: true, interval: 300), expected: false)
        }
        #expect(limiter.nextAvailableDelay(now: 120, automatic: true, interval: 300) == 180)
        expectSamplingAdmission(limiter.admit(now: 300, automatic: true, interval: 300), expected: true)
    }

    @Test func oldPreferencesRetainEffectiveBalancedLimitsAndNewCustomSettingsRoundTrip() throws {
        let old = try JSONDecoder().decode(DesktopPreferences.self, from: Data(#"{"minimumCaptureInterval":10,"periodicInterval":30,"interactionTriggersEnabled":true}"#.utf8))
        #expect(old.minimumCaptureInterval == 15 && old.periodicInterval == 60)
        #expect(old.samplingOptions == DesktopSamplingOptions() && old.interactionTriggersEnabled)
        var custom = old; custom.minimumCaptureInterval = 1; custom.periodicInterval = 5
        custom.samplingOptions.minimumContentInterval = 1; custom.samplingOptions.imageFallbackInterval = 5
        custom.samplingOptions.contentCapacity = 60; custom.samplingOptions.imageCapacity = 60
        custom.samplingOptions.conserveBattery = false
        #expect(try JSONDecoder().decode(DesktopPreferences.self, from: JSONEncoder().encode(custom)) == custom)
        var invalid = custom.samplingOptions; invalid.imageFallbackInterval = .infinity
        #expect(!invalid.isValid)
        invalid = custom.samplingOptions; invalid.imageCapacity = 61; #expect(!invalid.isValid)
    }

    @Test func fasterAutomaticSettingsPassOldCeilingAndRetainBatteryLimit() {
        var limiter = CaptureRateLimiter()
        for second in 0..<60 {
            expectSamplingAdmission(limiter.admit(now: Double(second), automatic: true, interval: 1, automaticCapacity: 60), expected: true)
        }
        #expect(limiter.nextAvailableDelay(now: 59.5, automatic: true, interval: 1, automaticCapacity: 60) == 0.5)
        expectSamplingAdmission(limiter.admit(now: 60, automatic: true, interval: 1, automaticCapacity: 60), expected: true)

        var battery = CaptureRateLimiter()
        expectSamplingAdmission(battery.admit(now: 0, automatic: true, interval: 1, lowPower: true, automaticCapacity: 60), expected: true)
        #expect(battery.nextAvailableDelay(now: 1, automatic: true, interval: 1, lowPower: true, automaticCapacity: 60) == 29)
        expectSamplingAdmission(battery.admit(now: 30, automatic: true, interval: 1, lowPower: true, automaticCapacity: 60), expected: true)
        expectSamplingAdmission(battery.admit(now: 59, automatic: true, interval: 1, lowPower: true, automaticCapacity: 60), expected: false)
    }

    @Test func balancedAutomaticAttemptsCannotBurstPastFourOrRetryFailedWorkForFree() {
        var limiter = CaptureRateLimiter()
        // Every admitted attempt may fail afterward. There is deliberately no
        // success callback here that could refund a failed attempt's budget.
        for second in [0.0,10,20,30] { expectSamplingAdmission(limiter.admit(now:second,automatic:true,automaticCapacity:4), expected:true) }
        for second in [31.0,40,50,59.999] { expectSamplingAdmission(limiter.admit(now:second,automatic:true,automaticCapacity:4), expected:false) }
        #expect(abs(limiter.nextAvailableDelay(now:59.999,automatic:true,automaticCapacity:4) - 0.001) < 0.000001)
        expectSamplingAdmission(limiter.admit(now:60,automatic:true,automaticCapacity:4), expected:true)
        expectSamplingAdmission(limiter.admit(now:60,automatic:true,automaticCapacity:4), expected:false)
        expectSamplingAdmission(limiter.admit(now:69.999,automatic:true,automaticCapacity:4), expected:false)
        expectSamplingAdmission(limiter.admit(now:70,automatic:true,automaticCapacity:4), expected:true)
    }

    @Test func balancedAutomaticCapacityPreservesManualBudgetAndSharedCeiling() {
        var mixed = CaptureRateLimiter()
        for second in [0.0,10,20,30] { expectSamplingAdmission(mixed.admit(now:second,automatic:true,automaticCapacity:4), expected:true) }
        expectSamplingAdmission(mixed.admit(now:31,automatic:true,automaticCapacity:4), expected:false)
        for second in stride(from:32.0,through:46,by:2) { expectSamplingAdmission(mixed.admit(now:second,automatic:false,automaticCapacity:4), expected:true) }
        expectSamplingAdmission(mixed.admit(now:48,automatic:false,automaticCapacity:4), expected:false)
        expectSamplingAdmission(mixed.admit(now:60,automatic:true,automaticCapacity:4), expected:true)
        var manual = CaptureRateLimiter()
        for second in stride(from:0.0,through:22,by:2) { expectSamplingAdmission(manual.admit(now:second,automatic:false,lowPower:true,automaticCapacity:4), expected:true) }
        expectSamplingAdmission(manual.admit(now:24,automatic:false,lowPower:true,automaticCapacity:4), expected:false)
        expectSamplingAdmission(manual.admit(now:60,automatic:false,lowPower:true,automaticCapacity:4), expected:true)
    }

    @Test func lowPowerStillAllowsOnlyTwoAutomaticAttemptsPerMinute() {
        var limiter = CaptureRateLimiter()
        expectSamplingAdmission(limiter.admit(now:0,automatic:true,lowPower:true,automaticCapacity:4), expected:true)
        expectSamplingAdmission(limiter.admit(now:29.999,automatic:true,lowPower:true,automaticCapacity:4), expected:false)
        expectSamplingAdmission(limiter.admit(now:30,automatic:true,lowPower:true,automaticCapacity:4), expected:true)
        expectSamplingAdmission(limiter.admit(now:59.999,automatic:true,lowPower:true,automaticCapacity:4), expected:false)
        expectSamplingAdmission(limiter.admit(now:60,automatic:true,lowPower:true,automaticCapacity:4), expected:true)
    }
}

private func expectSamplingAdmission(_ actual: Bool, expected: Bool) { #expect(actual == expected) }
