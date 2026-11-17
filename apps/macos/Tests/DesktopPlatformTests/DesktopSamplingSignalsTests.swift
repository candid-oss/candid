import Testing
@testable import DesktopPlatform

struct DesktopSamplingSignalsTests {
    @Test func balancedTextBurstsNeverRequestEventBodyReads() {
        var activity = DesktopSamplingActivity()
        for index in 0..<2000 {
            let trigger = index.isMultiple(of: 2) ? "content" : "selection"
            #expect(DesktopSamplingSignalPolicy.isActivityOnly(balanced: true, trigger: trigger))
            #expect(!DesktopSamplingSignalPolicy.readsEventText(balanced: true, textAllowed: true))
            activity.noteText(at: Double(index) * 0.1)
        }
        #expect(activity.isSettling(at: 200))
        #expect(!activity.isSettling(at: 201.5))
        #expect(!DesktopSamplingSignalPolicy.isActivityOnly(balanced: true, trigger: "window"))
        #expect(!DesktopSamplingSignalPolicy.isActivityOnly(balanced: true, trigger: "focus"))
    }

    @Test func detailedComparisonAndTextScopeArePreserved() {
        #expect(!DesktopSamplingSignalPolicy.isActivityOnly(balanced: false, trigger: "content"))
        #expect(DesktopSamplingSignalPolicy.readsEventText(balanced: false, textAllowed: true))
        #expect(!DesktopSamplingSignalPolicy.readsEventText(balanced: false, textAllowed: false))
        #expect(!DesktopSamplingSignalPolicy.readsEventText(balanced: true, textAllowed: false))
    }

    @Test func balancedSnapshotIsNotStarvedByContentOrCursorRevisions() {
        #expect(DesktopContextSourcePolicy.accepts(expectedWindow: 7, actualWindow: 7, expectedPID: 42, actualPID: 42, ownerUnchanged: true, generationUnchanged: false, balanced: true, allowed: true, active: true))
        #expect(!DesktopContextSourcePolicy.accepts(expectedWindow: 7, actualWindow: 7, expectedPID: 42, actualPID: 42, ownerUnchanged: true, generationUnchanged: false, balanced: false, allowed: true, active: true))
    }

    @Test func balancedSnapshotStillRejectsEverySourceAndSafetyBoundary() {
        #expect(!DesktopContextSourcePolicy.accepts(expectedWindow: 7, actualWindow: 8, expectedPID: 42, actualPID: 42, ownerUnchanged: true, generationUnchanged: false, balanced: true, allowed: true, active: true))
        #expect(!DesktopContextSourcePolicy.accepts(expectedWindow: 7, actualWindow: 7, expectedPID: 42, actualPID: 43, ownerUnchanged: true, generationUnchanged: false, balanced: true, allowed: true, active: true))
        #expect(!DesktopContextSourcePolicy.accepts(expectedWindow: 7, actualWindow: 7, expectedPID: 42, actualPID: nil, ownerUnchanged: true, generationUnchanged: false, balanced: true, allowed: true, active: true))
        #expect(!DesktopContextSourcePolicy.accepts(expectedWindow: 7, actualWindow: 7, expectedPID: 42, actualPID: 42, ownerUnchanged: false, generationUnchanged: false, balanced: true, allowed: true, active: true))
        #expect(!DesktopContextSourcePolicy.accepts(expectedWindow: 7, actualWindow: 7, expectedPID: 42, actualPID: 42, ownerUnchanged: true, generationUnchanged: false, balanced: true, allowed: false, active: true))
        #expect(!DesktopContextSourcePolicy.accepts(expectedWindow: 7, actualWindow: 7, expectedPID: 42, actualPID: 42, ownerUnchanged: true, generationUnchanged: false, balanced: true, allowed: true, active: false))
    }

    @Test func activityCancellationDiscardsPreviousSourceQuietState() {
        var activity = DesktopSamplingActivity()
        activity.noteText(at: 20)
        #expect(activity.isSettling(at: 21))
        activity.cancel()
        #expect(!activity.isSettling(at: 21))
    }

    @Test func optionalReturnPolicyReadsOnlyConfiguredCodesAndMergesRepeats() {
        var signals = DesktopInteractionSignals()
        let signalResult1 = signals.noteReturn(keyCode: 0, isRepeat: false, at: 0)
        #expect(!signalResult1)
        let signalResult2 = signals.noteReturn(keyCode: 9, isRepeat: false, at: 0)
        #expect(!signalResult2)
        let signalResult3 = signals.noteReturn(keyCode: 36, isRepeat: false, at: 0)
        #expect(signalResult3)
        let signalResult4 = signals.noteReturn(keyCode: 76, isRepeat: false, at: 0.2)
        #expect(!signalResult4)
        let signalResult5 = signals.noteReturn(keyCode: 36, isRepeat: true, at: 1)
        #expect(!signalResult5)
        let signalResult6 = signals.noteReturn(keyCode: 76, isRepeat: false, at: 1.1)
        #expect(signalResult6)
        signals.cancel()
        let signalResult7 = signals.noteReturn(keyCode: 36, isRepeat: false, at: 1.2)
        #expect(signalResult7)
    }

    @Test func scrollBurstEmitsOnlyAfterQuietAndCancellationDropsTail() {
        var signals = DesktopInteractionSignals()
        for index in 0...100 {
            let now = Double(index) / 20
            signals.noteScroll(at: now)
            let signalResult8 = signals.takeScroll(at: now + 0.049)
            #expect(!signalResult8)
        }
        let signalResult9 = signals.takeScroll(at: 5.99)
        #expect(!signalResult9)
        let signalResult10 = signals.takeScroll(at: 6)
        #expect(signalResult10)
        let signalResult11 = signals.takeScroll(at: 7)
        #expect(!signalResult11)
        signals.noteScroll(at: 10)
        signals.cancel()
        #expect(signals.scrollRemaining(at: 20) == nil)
        let signalResult12 = signals.takeScroll(at: 20)
        #expect(!signalResult12)
    }
}
