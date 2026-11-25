import DesktopCore
import Foundation
import Testing

struct DesktopRecognitionPolicyTests {
    /// Saved policies from before the power gate keep working: the batch mode becomes automatic processing (the
    /// core's 后台处理 setting paces it now) and the old power fields are ignored.
    @Test func oldPoliciesDecodeWithoutTheirPowerFields() throws {
        let old = #"{"mode":"batch","requireExternalPower":true,"minimumBatteryPercent":60,"pluggedMinutes":5,"pauseInLowPower":true,"batchIntervalMinutes":30,"batchLimit":10,"includeHistory":true,"automaticSince":0}"#
        let policy = try JSONDecoder().decode(DesktopRecognitionPolicy.self, from: Data(old.utf8))
        #expect(policy.mode == .immediate && policy.includeHistory)
        let manual = try JSONDecoder().decode(DesktopRecognitionPolicy.self, from: Data(#"{"mode":"manual","includeHistory":false,"automaticSince":0}"#.utf8))
        #expect(manual.mode == .manual)
    }

    @Test func aMacWithoutABatteryReportsExternalPower() {
        let desktop = DesktopRecognitionPower(onExternalPower: nil, hasInternalBattery: false, batteryPercent: nil, lowPower: false, thermal: "nominal")
        #expect(desktop.report(idleSeconds: 3)["external_power"] as? Bool == true)
        let unknown = DesktopRecognitionPower(onExternalPower: nil, hasInternalBattery: nil, batteryPercent: nil, lowPower: false, thermal: "fair")
        #expect(unknown.report(idleSeconds: nil)["external_power"] is NSNull)
    }
}
