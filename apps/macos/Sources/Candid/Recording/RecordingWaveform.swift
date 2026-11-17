import AVFoundation
import Foundation

/// Bounded memory envelope of decoded source samples, including explicit silence gaps.
enum RecordingWaveform {
    nonisolated static func peaks(_ url: URL, buckets: Int = 256) throws -> [Float] {
        guard buckets > 0, buckets <= 4096 else { return [] }
        if url.lastPathComponent.hasSuffix(".playlist.json") {
            struct Part: Decodable { let path: String; let sampleStart: Int64; let sampleCount: Int64; let sampleRate: Int }
            struct Playlist: Decodable { let format: String; let parts: [Part] }
            let decoder = JSONDecoder(); decoder.keyDecodingStrategy = .convertFromSnakeCase
            let playlist = try decoder.decode(Playlist.self, from: Data(contentsOf: url))
            guard playlist.format == "candid-playlist/1", let total = playlist.parts.last.map({ $0.sampleStart + $0.sampleCount }), total > 0 else { return [] }
            var values = [Float](repeating: 0, count: buckets)
            for part in playlist.parts {
                let file = try AVAudioFile(forReading: URL(fileURLWithPath: part.path))
                guard let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: 8192) else { continue }
                var offset: Int64 = 0
                while offset < file.length {
                    try Task.checkCancellation()
                    try file.read(into: buffer, frameCount: AVAudioFrameCount(min(8192, file.length - offset)))
                    guard buffer.frameLength > 0, let samples = buffer.floatChannelData else { break }
                    for frame in 0..<Int(buffer.frameLength) {
                        let global = part.sampleStart + offset + Int64(frame)
                        let bucket = min(buckets - 1, Int(global * Int64(buckets) / total))
                        for channel in 0..<Int(buffer.format.channelCount) { values[bucket] = max(values[bucket], min(1, abs(samples[channel][frame]))) }
                    }
                    offset += Int64(buffer.frameLength)
                }
            }
            return values
        }
        let file = try AVAudioFile(forReading: url)
        guard file.length > 0,
              let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: 8192)
        else { return [] }
        var values = [Float](repeating: 0, count: buckets)
        var offset: Int64 = 0
        while offset < file.length {
            try Task.checkCancellation()
            try file.read(into: buffer, frameCount: AVAudioFrameCount(min(8192, file.length - offset)))
            guard buffer.frameLength > 0, let samples = buffer.floatChannelData else { break }
            for frame in 0..<Int(buffer.frameLength) {
                let bucket = min(buckets - 1, Int((offset + Int64(frame)) * Int64(buckets) / file.length))
                for channel in 0..<Int(buffer.format.channelCount) {
                    values[bucket] = max(values[bucket], min(1, abs(samples[channel][frame])))
                }
            }
            offset += Int64(buffer.frameLength)
        }
        return values
    }
}
