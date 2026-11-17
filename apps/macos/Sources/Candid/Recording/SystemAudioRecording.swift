import AVFoundation
import CoreMedia
import OSLog
import ScreenCaptureKit

/// Only audio outputs are registered; no screen frames are read or saved.
@available(macOS 15, *)
final class SystemAudioRecording: NSObject, SCStreamOutput, SCStreamDelegate, @unchecked Sendable {
    private let queue = DispatchQueue(label: "local.candid.audio-mixture", qos: .userInitiated)
    private let sink: RecordingAudioSink
    private let mixer: RecordingMixer
    private let systemDecoder = RecordingPCMDecoder()
    private let microphoneDecoder = RecordingPCMDecoder()
    private var loggedFormats = Set<Int>()
    private static let logger = Logger(subsystem: "io.github.candid-oss.candid", category: "RecordingAudio")
    private var stream: SCStream?
    private var accepting = true // queue confined
    private let lock = NSLock()
    private var lastMicrophone = Date()

    init(sink: RecordingAudioSink) {
        self.sink = sink
        mixer = RecordingMixer { pcm, level in sink.enqueue(pcm, level: level) }
    }

    @MainActor func start() async throws {
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
        guard let display = content.displays.first else { throw MediaCaptureError.screenUnavailable }
        let filter = SCContentFilter(display: display, excludingWindows: [])
        let config = SCStreamConfiguration()
        config.width = 2; config.height = 2
        config.minimumFrameInterval = CMTime(value: 1, timescale: 1)
        config.showsCursor = false
        config.capturesAudio = true
        config.excludesCurrentProcessAudio = true
        config.sampleRate = 48_000; config.channelCount = 1
        config.captureMicrophone = true
        let stream = SCStream(filter: filter, configuration: config, delegate: self)
        try stream.addStreamOutput(self, type: .audio, sampleHandlerQueue: queue)
        try stream.addStreamOutput(self, type: .microphone, sampleHandlerQueue: queue)
        self.stream = stream
        do { try await stream.startCapture() }
        catch { self.stream = nil; throw error }
    }

    var microphoneStalled: Bool { lock.withLock { Date().timeIntervalSince(lastMicrophone) > 4 } }

    @MainActor func stop() async throws {
        let stream = self.stream; self.stream = nil
        // Close admission before asking the OS to stop; in-flight audio cannot pass this gate.
        await withCheckedContinuation { continuation in
            queue.async { self.accepting = false; continuation.resume() }
        }
        var stopError: Error?
        do { try await stream?.stopCapture() } catch { stopError = error }
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            queue.async {
                do { try self.mixer.finish(); continuation.resume() }
                catch { self.sink.fail(error); continuation.resume(throwing: error) }
            }
        }
        if let stopError { throw stopError }
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        Self.logger.error("Audio stream stopped: \((error as NSError).domain, privacy: .public) code=\((error as NSError).code)")
        sink.fail(error)
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard accepting, type == .audio || type == .microphone else { return }
        guard sampleBuffer.isValid, CMSampleBufferDataIsReady(sampleBuffer) else { return }
        let count = CMSampleBufferGetNumSamples(sampleBuffer)
        guard count > 0 else { return }
        do {
            if loggedFormats.insert(type.rawValue).inserted,
               let description = CMSampleBufferGetFormatDescription(sampleBuffer),
               let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(description) {
                let f = asbd.pointee
                Self.logger.info("Audio input type=\(type.rawValue) rate=\(f.mSampleRate) channels=\(f.mChannelsPerFrame) bits=\(f.mBitsPerChannel) flags=\(f.mFormatFlags) frames=\(count)")
            }
            let decoder = type == .microphone ? microphoneDecoder : systemDecoder
            let decoded = try decoder.decode(sampleBuffer)
            try mixer.append(decoded.samples, rate: decoded.rate, timestamp: decoded.timestamp, input: type == .microphone ? .microphone : .system)
            if type == .microphone { lock.withLock { lastMicrophone = Date() } }
        } catch {
            accepting = false
            Self.logger.error("Audio packet rejected type=\(type.rawValue): \(error.localizedDescription, privacy: .public)")
            sink.fail(error)
        }
    }
}
