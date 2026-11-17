import AVFoundation
import CoreMedia
import OSLog

/// Queue-confined decoder for the native format of each SCK audio output. The
/// microphone route need not use the Float32 format of the system output.
final class RecordingPCMDecoder {
    private var converter: AVAudioConverter?
    private static let logger = Logger(subsystem: "io.github.candid-oss.candid", category: "RecordingAudio")

    func decode(_ sample: CMSampleBuffer) throws -> (samples: [Float], rate: Double, timestamp: Double) {
        let count = CMSampleBufferGetNumSamples(sample)
        guard let description = CMSampleBufferGetFormatDescription(sample),
              let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(description),
              (1...32).contains(asbd.pointee.mChannelsPerFrame),
              let layout = AVAudioChannelLayout(layoutTag: kAudioChannelLayoutTag_DiscreteInOrder | asbd.pointee.mChannelsPerFrame),
              let format = AVAudioFormat(streamDescription: asbd, channelLayout: layout),
              (8_000...192_000).contains(format.sampleRate),
              count > 0, count <= Int(format.sampleRate),
              let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(count))
        else { throw MediaCaptureError.audioFormat }
        buffer.frameLength = AVAudioFrameCount(count)
        let status = CMSampleBufferCopyPCMDataIntoAudioBufferList(sample, at: 0, frameCount: Int32(count), into: buffer.mutableAudioBufferList)
        guard status == noErr else {
            Self.logger.error("PCM packet copy failed code=\(status)")
            throw MediaCaptureError.audioFormat
        }
        let pcm = try floatingPoint(buffer)
        guard let channels = pcm.floatChannelData, pcm.frameLength == count else { throw MediaCaptureError.audioFormat }
        let channelCount = Int(pcm.format.channelCount)
        var mono = [Float](repeating: 0, count: count)
        for frame in 0..<count {
            for channel in 0..<channelCount {
                let value = channels[channel][frame]
                mono[frame] += (value.isFinite ? value : 0) / Float(channelCount)
            }
        }
        return (mono, format.sampleRate, CMTimeGetSeconds(CMSampleBufferGetPresentationTimeStamp(sample)))
    }

    private func floatingPoint(_ buffer: AVAudioPCMBuffer) throws -> AVAudioPCMBuffer {
        let format = buffer.format
        if format.commonFormat == .pcmFormatFloat32 && !format.isInterleaved { return buffer }
        if converter?.inputFormat != format {
            guard let layout = format.channelLayout else { throw MediaCaptureError.audioFormat }
            let target = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: format.sampleRate,
                                       interleaved: false, channelLayout: layout)
            guard
                  let conversion = AVAudioConverter(from: format, to: target) else { throw MediaCaptureError.audioFormat }
            converter = conversion
        }
        guard let converter, let output = AVAudioPCMBuffer(pcmFormat: converter.outputFormat, frameCapacity: buffer.frameLength)
        else { throw MediaCaptureError.audioFormat }
        do { try converter.convert(to: output, from: buffer) }
        catch {
            Self.logger.error("PCM conversion failed: \((error as NSError).domain, privacy: .public) code=\((error as NSError).code)")
            throw MediaCaptureError.audioFormat
        }
        return output
    }
}
