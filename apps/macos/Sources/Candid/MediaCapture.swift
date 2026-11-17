import AppKit
import CandidLocalization
import AVFoundation
import CoreGraphics
import ScreenCaptureKit

enum MediaCaptureError: LocalizedError {
    case microphonePermission, screenPermission, recordingBusy, recordingUnavailable
    case systemAudioUnavailable, audioFormat
    case noRecording, emptyRecording, screenUnavailable, selectionBusy, screenshotEncoding

    var errorDescription: String? {
        switch self {
        case .microphonePermission: L("请在系统设置的麦克风权限中允许 Candid。")
        case .screenPermission: L("请在系统设置的屏幕录制权限中允许 Candid，再重试。")
        case .recordingBusy: L("已有录音正在开始或进行。")
        case .recordingUnavailable: L("无法开始录音，请检查麦克风是否可用。")
        case .systemAudioUnavailable: L("系统混音需要 macOS 15 或更新版本，可改用麦克风录音。")
        case .audioFormat: L("无法转换当前音频输入格式，录音已停止。")
        case .noRecording: L("当前没有录音。")
        case .emptyRecording: L("没有录到可播放的声音，请重新录制。")
        case .screenUnavailable: L("屏幕已不可用或布局已变化，请重新框选。")
        case .selectionBusy: L("已有截图框选正在进行。")
        case .screenshotEncoding: L("无法保存这张截图。")
        }
    }
}

/// Region screenshots remain drafts; microphone audio goes to a durable segmented outbox.
@MainActor
final class MediaCapture {
    private var engine: AVAudioEngine?
    private var sink: RecordingAudioSink?
    private var systemAudio: AnyObject?
    private var configurationObserver: NSObjectProtocol?
    var recordingID: UUID? { sink?.recordingID }
    var recordingStatus: RecordingAudioSink.Status? { sink?.status }
    private var startingRecording = false
    var isStartingRecording: Bool { startingRecording }
    private var selectingRegion = false
    private let regionSelector = MediaCaptureRegionSelector()

    func startRecording(journal: RecordingJournal, seconds: Int, input recordingInput: RecordingInput = .microphone) async throws -> Date {
        guard sink == nil, !startingRecording else { throw MediaCaptureError.recordingBusy }
        startingRecording = true
        defer { startingRecording = false }

        let authorized: Bool
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized: authorized = true
        case .notDetermined: authorized = await AVCaptureDevice.requestAccess(for: .audio)
        default: authorized = false
        }
        guard authorized else { throw MediaCaptureError.microphonePermission }
        try Task.checkCancellation()
        if recordingInput == .systemAndMicrophone {
            guard #available(macOS 15, *) else { throw MediaCaptureError.systemAudioUnavailable }
            guard CGPreflightScreenCaptureAccess() || CGRequestScreenCaptureAccess() else { throw MediaCaptureError.screenPermission }
            let startedAt = Date()
            let sink = try await Task.detached(priority: .userInitiated) {
                try RecordingAudioSink(journal: journal, sampleRate: 48_000, seconds: seconds, startedAt: startedAt, input: recordingInput)
            }.value
            let system = SystemAudioRecording(sink: sink)
            do {
                try await system.start()
                try Task.checkCancellation()
                self.sink = sink; self.systemAudio = system
                return startedAt
            } catch {
                try? await system.stop()
                try? await sink.finish(reason: "start_failed")
                throw error
            }
        }
        let engine = AVAudioEngine()
        let input = engine.inputNode
        let format = input.outputFormat(forBus: 0)
        guard format.sampleRate > 0, format.channelCount > 0, format.commonFormat == .pcmFormatFloat32,
              !format.isInterleaved else { throw MediaCaptureError.recordingUnavailable }
        let startedAt = Date()
        let sampleRate = Int(format.sampleRate)
        let sink = try await Task.detached(priority: .userInitiated) {
            try RecordingAudioSink(journal: journal, sampleRate: sampleRate, seconds: seconds, startedAt: startedAt)
        }.value
        input.installTap(onBus: 0, bufferSize: 2048, format: format) { buffer, time in
            sink.receive(buffer, sampleTime: time.isSampleTimeValid ? time.sampleTime : nil)
        }
        do {
            engine.prepare()
            try engine.start()
            self.engine = engine; self.sink = sink
            // A route/rate change is a real boundary, not an implied continuous recording.
            configurationObserver = NotificationCenter.default.addObserver(forName: .AVAudioEngineConfigurationChange,
                                                                           object: engine, queue: nil) { _ in
                sink.fail(RecordingJournalError.inputStopped)
            }
            return startedAt
        } catch {
            input.removeTap(onBus: 0); engine.stop()
            try? await sink.finish(reason: "start_failed")
            throw error
        }
    }

    /// Current input loudness, 0 (silence) to 1, or nil when not recording.
    func level() -> Float? { sink?.status.level }

    func recordingFailure() -> String? {
        guard let sink else { return nil }
        if AVCaptureDevice.authorizationStatus(for: .audio) != .authorized { return MediaCaptureError.microphonePermission.localizedDescription }
        if let failure = sink.status.failure { return failure }
        if #available(macOS 15, *), let system = systemAudio as? SystemAudioRecording {
            return system.microphoneStalled ? RecordingJournalError.inputStopped.localizedDescription : nil
        }
        if engine?.isRunning != true || Date().timeIntervalSince(sink.status.lastInput) > 4 { return RecordingJournalError.inputStopped.localizedDescription }
        return nil
    }

    /// Maps -50…0 dBFS onto 0…1 with a gentle curve so speech reads clearly
    /// and room noise stays near the floor.
    nonisolated static func normalized(decibels: Float) -> Float {
        guard decibels.isFinite else { return 0 }
        let linear = min(1, max(0, (decibels + 50) / 50))
        return linear * linear
    }

    func stopRecording(reason: String = "stopped") async throws {
        guard let sink else { throw MediaCaptureError.noRecording }
        if #available(macOS 15, *), let system = systemAudio as? SystemAudioRecording {
            self.systemAudio = nil
            defer { self.sink = nil }
            var stopError: Error?
            do { try await system.stop() } catch { stopError = error }
            try await sink.finish(reason: reason)
            if let stopError { throw stopError }
            return
        }
        guard let engine else { throw MediaCaptureError.noRecording }
        // Stop input before waiting for IO. Privacy never waits behind a save.
        engine.stop(); engine.inputNode.removeTap(onBus: 0)
        if let configurationObserver { NotificationCenter.default.removeObserver(configurationObserver) }
        configurationObserver = nil; self.engine = nil
        defer { self.sink = nil }
        try await sink.finish(reason: reason)
    }

    func captureRegion() async throws -> URL? {
        guard !selectingRegion else { throw MediaCaptureError.selectionBusy }
        selectingRegion = true
        defer { selectingRegion = false }

        guard CGPreflightScreenCaptureAccess() || CGRequestScreenCaptureAccess() else {
            throw MediaCaptureError.screenPermission
        }
        let pointer = NSEvent.mouseLocation
        guard let screen = NSScreen.screens.first(where: { $0.frame.contains(pointer) }) ?? NSScreen.main,
              let displayID = Self.displayID(for: screen) else {
            throw MediaCaptureError.screenUnavailable
        }
        let frame = screen.frame
        let backingScale = screen.backingScaleFactor
        guard let selection = await regionSelector.select(on: screen) else { return nil }
        try Task.checkCancellation()
        guard NSScreen.screens.contains(where: {
            Self.displayID(for: $0) == displayID && $0.frame == frame && $0.backingScaleFactor == backingScale
        }) else { throw MediaCaptureError.screenUnavailable }

        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
        guard let display = content.displays.first(where: { $0.displayID == displayID }) else {
            throw MediaCaptureError.screenUnavailable
        }
        let filter = SCContentFilter(display: display, excludingWindows: [])
        let config = SCStreamConfiguration()
        config.sourceRect = selection
        config.width = Int((selection.width * backingScale).rounded())
        config.height = Int((selection.height * backingScale).rounded())
        config.showsCursor = false
        let image = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: config)
        try Task.checkCancellation()

        guard let data = NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:]) else {
            throw MediaCaptureError.screenshotEncoding
        }
        let url = temporaryURL(extension: "png")
        try data.write(to: url, options: .atomic)
        return url
    }

    private static func displayID(for screen: NSScreen) -> CGDirectDisplayID? {
        (screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value
    }

    private func temporaryURL(extension fileExtension: String) -> URL {
        FileManager.default.temporaryDirectory
            .appendingPathComponent("candid-\(UUID().uuidString)")
            .appendingPathExtension(fileExtension)
    }
}
