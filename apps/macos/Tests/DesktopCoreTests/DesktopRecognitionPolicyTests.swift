import DesktopCore
import Testing

struct DesktopRecognitionPolicyTests {
    @Test func unplugAndSleepResetContinuousPluggedTime() {
        var policy = DesktopRecognitionPolicy()
        policy.requireExternalPower = true
        policy.pluggedMinutes = 2
        let ac = DesktopRecognitionPower(onExternalPower: true, hasInternalBattery: true,
            batteryPercent: 80, lowPower: false, thermalBlocked: false)
        let battery = DesktopRecognitionPower(onExternalPower: false, hasInternalBattery: true,
            batteryPercent: 80, lowPower: false, thermalBlocked: false)
        var gate = DesktopRecognitionPowerGate()
        #expect(gate.blockReason(policy: policy, power: ac, uptime: 0) != nil)
        #expect(gate.blockReason(policy: policy, power: ac, uptime: 120) == nil)
        #expect(gate.blockReason(policy: policy, power: battery, uptime: 121) != nil)
        #expect(gate.blockReason(policy: policy, power: ac, uptime: 125) != nil)
        #expect(gate.remainingPluggedWait(policy: policy, uptime: 130) == 115)
        #expect(gate.blockReason(policy: policy, power: ac, uptime: 245) == nil)
        gate.reset()
        #expect(gate.blockReason(policy: policy, power: ac, uptime: 800) != nil)
    }

    @Test func independentBatteryThresholdsAndDesktopBehavior() {
        let ocr = DesktopRecognitionPolicy()
        var vision = DesktopRecognitionPolicy()
        vision.minimumBatteryPercent = 60
        let battery = DesktopRecognitionPower(onExternalPower: false, hasInternalBattery: true,
            batteryPercent: 59, lowPower: false, thermalBlocked: false)
        let unknownCharge = DesktopRecognitionPower(onExternalPower: true, hasInternalBattery: nil,
            batteryPercent: nil, lowPower: false, thermalBlocked: false)
        let desktop = DesktopRecognitionPower(onExternalPower: true, hasInternalBattery: false,
            batteryPercent: nil, lowPower: false, thermalBlocked: false)
        var gate = DesktopRecognitionPowerGate()
        #expect(gate.blockReason(policy: ocr, power: battery, uptime: 0) == nil)
        #expect(gate.blockReason(policy: vision, power: battery, uptime: 0) != nil)
        #expect(gate.blockReason(policy: vision, power: unknownCharge, uptime: 0) != nil)
        #expect(gate.blockReason(policy: vision, power: desktop, uptime: 0) == nil)
    }
}
