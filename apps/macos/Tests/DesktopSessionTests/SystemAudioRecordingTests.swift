import AVFoundation
import CoreMedia
import Foundation
import SwiftUI
import Testing
@testable import Candid

@Suite struct SystemAudioRecordingTests {
    @Test func mixedTimestampsResampleAndAlignBothInputs() throws {
        var result = Data()
        let mixer = RecordingMixer { data, _ in result.append(data) }
        // Deliver system first, with 44.1 kHz mic arriving 10 ms later; preserve offsets.
        for index in 0..<100 {
            let time = 100 + Double(index) / 100
            try mixer.append([Float](repeating: 0.6, count: 480), rate: 48_000, timestamp: time, input: .system)
            if index > 0 {
                try mixer.append([Float](repeating: 0.2, count: 441), rate: 44_100, timestamp: time, input: .microphone)
            }
        }
        try mixer.finish()
        #expect(result.count == 48_000 * 2)
        let values = result.withUnsafeBytes { Array($0.bindMemory(to: Int16.self)) }
        #expect(values.prefix(480).allSatisfy { abs(Int($0) - 9830) <= 1 })
        #expect(values.dropFirst(480).allSatisfy { abs(Int($0) - 13107) <= 1 })
    }

    @Test func silentOutputAndLateOrMissingPacketsAreHandledExplicitly() throws {
        var frames = 0
        let mixer = RecordingMixer { data, _ in frames += data.count / 2 }
        try mixer.append([Float](repeating: 0.2, count: 48_000), rate: 48_000, timestamp: 1, input: .microphone)
        #expect(frames == 24_000)
        #expect(throws: (any Error).self) {
            try mixer.append([0.4], rate: 48_000, timestamp: 1, input: .system)
        }
        #expect(throws: (any Error).self) {
            try mixer.append([0.2], rate: 48_000, timestamp: 3, input: .microphone)
        }
        try mixer.finish()
        #expect(frames == 48_000)
        try mixer.append([0.9], rate: 48_000, timestamp: 2, input: .microphone)
        #expect(frames == 48_000)
    }

    @Test func clippedAndNonfiniteSamplesRemainValidPCM() throws {
        var pcm = Data()
        let mixer = RecordingMixer { data, _ in pcm.append(data) }
        try mixer.append([2, -.infinity, .nan, -3], rate: 48_000, timestamp: 0, input: .system)
        try mixer.append([2, .infinity, .nan, -3], rate: 48_000, timestamp: 0, input: .microphone)
        try mixer.finish()
        #expect(pcm.withUnsafeBytes { Array($0.bindMemory(to: Int16.self)) } == [32767, 0, 0, -32767])
    }

    @Test func promptRequiresSustainedInputAndDoesNotRepeatDuringCall() {
        let app = CallAudioProcess(bundleID: "com.tencent.xinWeChat", pid: 42)
        var gate = CallReminderGate()
        #expect(gate.update([app], now: 0, canOffer: true) == nil)
        #expect(gate.update([app], now: 2, canOffer: false) == nil)
        #expect(gate.update([app], now: 4, canOffer: true) == app)
        #expect(gate.update([app], now: 6, canOffer: true) == nil)
        #expect(gate.update([], now: 10, canOffer: true) == nil)
        #expect(gate.update([app], now: 12, canOffer: true) == nil)
        #expect(gate.update([], now: 34, canOffer: true) == nil)
        #expect(gate.update([app], now: 36, canOffer: true) == nil)
        #expect(gate.update([app], now: 38, canOffer: true) == app)
        #expect(CallAudioProcess.supported("com.tencent.meeting"))
        #expect(CallAudioProcess.supported("com.google.Chrome.helper"))
        #expect(!CallAudioProcess.supported("com.tencent.meetingFake"))
        #expect(!CallAudioProcess.supported("io.github.candid-oss.candid"))
        var brief = CallReminderGate()
        #expect(brief.update([app], now: 0, canOffer: true) == nil)
        #expect(brief.update([], now: 2, canOffer: true) == nil)
        #expect(brief.update([app], now: 4, canOffer: true) == nil)
        #expect(brief.update([app], now: 6, canOffer: true) == app)
    }

    @Test func callEndStopsOnlyAssociatedRecordingAfterGrace() {
        let call = CallAudioProcess(bundleID: "com.tencent.meeting", pid: 42)
        let helper = CallAudioProcess(bundleID: "com.tencent.meeting.audio", pid: 43)
        let other = CallAudioProcess(bundleID: "com.google.Chrome", pid: 44)
        var gate = CallRecordingStopGate()
        let outcomes = [
            gate.shouldStop(target: call, active: [call], now: 0),
            gate.shouldStop(target: call, active: [], now: 2),
            gate.shouldStop(target: call, active: [helper], now: 10),
            gate.shouldStop(target: call, active: [other], now: 20),
            gate.shouldStop(target: call, active: nil, now: 30),
            gate.shouldStop(target: call, active: [], now: 40),
            gate.shouldStop(target: call, active: [other], now: 50),
            gate.shouldStop(target: call, active: [], now: 52),
            gate.shouldStop(target: nil, active: [], now: 90)
        ]
        #expect(outcomes == [false, false, false, false, false, false, false, true, false])
    }

    @Test func aKnownCallWindowKeepsMutedCallsRecordingUntilTheWindowCloses() {
        let call = CallAudioProcess(bundleID: "com.tencent.xinWeChat", pid: 42)
        var gate = CallRecordingStopGate()
        let outcomes = [
            gate.shouldStop(target: call, active: [], now: 0, callWindowPresent: true),
            gate.shouldStop(target: call, active: [], now: 600, callWindowPresent: true),
            gate.shouldStop(target: call, active: [call], now: 602, callWindowPresent: false),
            gate.shouldStop(target: call, active: [call], now: 614, callWindowPresent: false)
        ]
        #expect(outcomes == [false, false, false, true])
    }

    @Test @MainActor func preferencesAndLegacyManifestsKeepTheirSources() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("SYNTHETIC-mixture-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let store = MediaWorkspaceStore(profile: root)
        store.recordingInput = .systemAndMicrophone; store.callRecordingReminders = false; store.autoStopCallRecording = false
        let reopened = MediaWorkspaceStore(profile: root)
        #expect(reopened.recordingInput == .systemAndMicrophone)
        #expect(!reopened.callRecordingReminders)
        #expect(!reopened.autoStopCallRecording)
        let journal = RecordingJournal(profile: root)
        let writer = try SegmentedRecordingWriter(journal: journal, sampleRate: 48_000, seconds: 30, input: .systemAndMicrophone)
        try writer.append(Data(repeating: 0, count: 480 * 2)); try writer.finish()
        let segment = try #require(journal.recover().pending.first)
        #expect(segment.draft.source == "mac_system_audio")
        var object = try #require(JSONSerialization.jsonObject(with: JSONEncoder().encode(segment)) as? [String: Any])
        object.removeValue(forKey: "input")
        let legacy = try JSONDecoder().decode(RecordingSegment.self, from: JSONSerialization.data(withJSONObject: object))
        #expect(legacy.draft.source == "mac_microphone")
    }

    @Test @MainActor func renderPromptIfRequested() throws {
        guard let directory = ProcessInfo.processInfo.environment["CANDID_AUDIO_RENDER_DIR"] else { return }
        let root = URL(fileURLWithPath: directory)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let media = MediaWorkspaceStore(profile: root)
        for dark in [false, true] {
            let view = CallRecordingPrompt(appName: "腾讯会议", media: media, start: {}, dismiss: {})
                .environment(\.colorScheme, dark ? .dark : .light)
            let renderer = ImageRenderer(content: view)
            renderer.scale = 2
            let cg = try #require(renderer.cgImage)
            let image = NSBitmapImageRep(cgImage: cg)
            try #require(image.representation(using: .png, properties: [:])).write(to: root.appendingPathComponent(dark ? "prompt-dark.png" : "prompt-light.png"))
        }
    }
}
