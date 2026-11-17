import AVFoundation
import CoreMedia
import Testing
@testable import Candid

@Suite struct RecordingPCMDecoderTests {
    @Test func threeChannelMicrophoneReproducesTheOldStartupFailure() throws {
        let sample = try packet(format: .pcmFormatFloat32, interleaved: true, channels: 3)
        let description = try #require(CMSampleBufferGetFormatDescription(sample))
        let asbd = try #require(CMAudioFormatDescriptionGetStreamBasicDescription(description))
        // The old callback treated this nil as a corrupt recording, before writing any audio.
        #expect(AVAudioFormat(streamDescription: asbd) == nil)
        #expect(try RecordingPCMDecoder().decode(sample).samples.count == 480)
    }

    @Test func nativeIntegerFloatAndInterleavedPacketsHaveTheSameMonoSamples() throws {
        let decoder = RecordingPCMDecoder()
        // Reuse the decoder across a route/format change as SCK does during a call.
        for format in [AVAudioCommonFormat.pcmFormatInt16, .pcmFormatFloat32, .pcmFormatInt32, .pcmFormatFloat64, .pcmFormatInt16] {
            for interleaved in [true, false] {
              for channelCount: AVAudioChannelCount in [1, 2, 3] {
                let sample = try packet(format: format, interleaved: interleaved, channels: channelCount)
                let decoded = try decoder.decode(sample)
                #expect(decoded.rate == 48_000 && decoded.timestamp == 100)
                #expect(decoded.samples.count == 480)
                #expect(decoded.samples.allSatisfy { abs($0 - (0.125 + 0.25 / Float(channelCount))) < 0.0001 })
              }
            }
        }
    }

    @Test func integerMicrophoneAndFloatSystemAudioReachTheDurableWriter() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("SYNTHETIC-native-pcm-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let journal = RecordingJournal(profile: root)
        let sink = try RecordingAudioSink(journal: journal, sampleRate: 48_000, seconds: 30, startedAt: Date(), input: .systemAndMicrophone)
        let mixer = RecordingMixer { pcm, level in sink.enqueue(pcm, level: level) }
        let mic = try RecordingPCMDecoder().decode(packet(format: .pcmFormatInt16, interleaved: true))
        let system = try RecordingPCMDecoder().decode(packet(format: .pcmFormatFloat32, interleaved: false))
        try mixer.append(mic.samples, rate: mic.rate, timestamp: mic.timestamp, input: .microphone)
        try mixer.append(system.samples, rate: system.rate, timestamp: system.timestamp, input: .system)
        try mixer.finish(); try await sink.finish(reason: "stopped")
        let saved = try #require(journal.recover().pending.first)
        #expect(saved.sampleCount == 480)
        let bytes = try Data(contentsOf: journal.file(saved, ".wav")).dropFirst(44)
        #expect(bytes.withUnsafeBytes { $0.bindMemory(to: Int16.self).allSatisfy { abs(Int($0) - 8192) <= 1 } })
    }

    private func packet(format common: AVAudioCommonFormat, interleaved: Bool, channels: AVAudioChannelCount = 2) throws -> CMSampleBuffer {
        let layout = try #require(AVAudioChannelLayout(layoutTag: kAudioChannelLayoutTag_DiscreteInOrder | channels))
        let format = AVAudioFormat(commonFormat: common, sampleRate: 48_000, interleaved: interleaved, channelLayout: layout)
        let buffer = try #require(AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 480))
        buffer.frameLength = 480
        for channel in 0..<Int(channels) {
            for frame in 0..<480 {
                let index = interleaved ? frame * Int(channels) + channel : frame
                let plane = interleaved ? 0 : channel
                let value = channel == 0 ? 0.375 : 0.125
                switch common {
                case .pcmFormatInt16: buffer.int16ChannelData![plane][index] = Int16(value * 32768)
                case .pcmFormatInt32: buffer.int32ChannelData![plane][index] = Int32(value * 2147483648)
                case .pcmFormatFloat32: buffer.floatChannelData![plane][index] = Float(value)
                case .pcmFormatFloat64:
                    let data = UnsafeMutableAudioBufferListPointer(buffer.mutableAudioBufferList)[plane].mData!
                    data.assumingMemoryBound(to: Double.self)[index] = value
                default: Issue.record("Unexpected synthetic format")
                }
            }
        }
        var sample: CMSampleBuffer?
        var timing = CMSampleTimingInfo(duration: CMTime(value: 1, timescale: 48_000), presentationTimeStamp: CMTime(seconds: 100, preferredTimescale: 48_000), decodeTimeStamp: .invalid)
        #expect(CMSampleBufferCreate(allocator: nil, dataBuffer: nil, dataReady: false, makeDataReadyCallback: nil, refcon: nil,
                                    formatDescription: format.formatDescription, sampleCount: 480, sampleTimingEntryCount: 1,
                                    sampleTimingArray: &timing, sampleSizeEntryCount: 0, sampleSizeArray: nil, sampleBufferOut: &sample) == noErr)
        let ready = try #require(sample)
        #expect(CMSampleBufferSetDataBufferFromAudioBufferList(ready, blockBufferAllocator: nil, blockBufferMemoryAllocator: nil,
                                                            flags: 0, bufferList: buffer.audioBufferList) == noErr)
        #expect(CMSampleBufferSetDataReady(ready) == noErr)
        return ready
    }
}
