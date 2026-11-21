import Foundation

/// Reconstructible, bounded view cache. Journal originals are opened read-only.
enum RecordingPlaybackCopy {
    static let byteLimit: Int64 = 512 * 1024 * 1024

    static func make(journal: RecordingJournal, parts: [RecordingSegment], to destination: URL, preferOriginalParts: Bool = true) throws -> URL {
        let parts = parts.sorted { $0.sampleStart < $1.sampleStart }
        guard let first = parts.first, let last = parts.last else { throw RecordingJournalError.invalid }
        if preferOriginalParts {
            var cursor: Int64 = 0
            let playlist: [[String: Any]] = try parts.map { part in
                guard part.sampleRate == first.sampleRate, part.sampleStart >= cursor else { throw RecordingJournalError.invalid }
                let file = journal.file(part, part.mediaSuffix)
                let checked = try RecordingJournal.validate(file, sampleRate: part.sampleRate)
                guard checked.hash == part.sha256 else { throw RecordingJournalError.invalid }
                cursor = part.sampleStart + part.sampleCount
                return ["path": file.path, "sample_start": part.sampleStart, "sample_count": part.sampleCount, "sample_rate": part.sampleRate]
            }
            try FileManager.default.createDirectory(at: destination.deletingLastPathComponent(), withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            let manifest = destination.appendingPathExtension("playlist.json")
            try JSONSerialization.data(withJSONObject: ["format": "candid-playlist/1", "parts": playlist]).write(to: manifest, options: .atomic)
            return manifest
        }
        let frames = last.sampleStart + last.sampleCount
        guard frames > 0, frames <= (byteLimit - 44) / 2 else { throw MediaWorkspaceError.tooLarge }
        try FileManager.default.createDirectory(at: destination.deletingLastPathComponent(), withIntermediateDirectories: true,
                                                attributes: [.posixPermissions: 0o700])
        let staging = destination.appendingPathExtension("part")
        FileManager.default.createFile(atPath: staging.path, contents: RecordingJournal.wavHeader(frames: frames, rate: first.sampleRate),
                                       attributes: [.posixPermissions: 0o600])
        let output = try FileHandle(forWritingTo: staging)
        defer { try? output.close(); try? FileManager.default.removeItem(at: staging) }
        try output.seekToEnd()
        var cursor: Int64 = 0
        for part in parts {
            guard part.sampleRate == first.sampleRate, part.sampleStart >= cursor else { throw RecordingJournalError.invalid }
            var gap = part.sampleStart - cursor
            while gap > 0 {
                try Task.checkCancellation()
                let count = min(gap, 32768)
                try output.write(contentsOf: Data(repeating: 0, count: Int(count * 2)))
                gap -= count
            }
            // A flat PCM copy is built only from WAV parts; AAC parts always play through the playlist.
            guard part.codec != .aac else { throw RecordingJournalError.invalid }
            let input = try FileHandle(forReadingFrom: journal.file(part, ".wav"))
            defer { try? input.close() }
            try input.seek(toOffset: 44)
            var remaining = part.sampleCount * 2
            while remaining > 0 {
                try Task.checkCancellation()
                let bytes = try input.read(upToCount: Int(min(remaining, 65536))) ?? Data()
                guard !bytes.isEmpty else { throw RecordingJournalError.invalid }
                try output.write(contentsOf: bytes)
                remaining -= Int64(bytes.count)
            }
            cursor = part.sampleStart + part.sampleCount
        }
        try output.close()
        try? FileManager.default.removeItem(at: destination)
        try FileManager.default.moveItem(at: staging, to: destination)
        return destination
    }
}
