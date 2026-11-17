// Compiled with the production journal and writer by test_recording_recovery.py.
// No App, capture device, service port or real library is opened.
import Foundation
import Darwin

@main struct RecordingRecoveryProbe {
    static func main() throws {
        let args = CommandLine.arguments
        let root = URL(fileURLWithPath: args[2], isDirectory: true)
        let journal = RecordingJournal(profile: root)
        if args[1] == "recover" {
            let inventory = try journal.recover()
            print(String(data: try JSONEncoder().encode(inventory.pending), encoding: .utf8)!)
            return
        }
        let writer = try SegmentedRecordingWriter(journal: journal, sampleRate: 8_000, seconds: 30,
                                                  startedAt: Date(timeIntervalSince1970: 1_800_000_000))
        func pcm(_ count: Int, start: Int = 0) -> Data {
            let samples = (start..<(start + count)).map { Int16(truncatingIfNeeded: $0).littleEndian }
            return samples.withUnsafeBytes { Data($0) }
        }
        func waitForKill() throws {
            try Data("ready".utf8).write(to: root.appendingPathComponent("ready"))
            while true { Thread.sleep(forTimeInterval: 0.1) }
        }
        try writer.append(pcm(240_000))
        if args[1] == "disk-full" {
            let filler = root.appendingPathComponent("synthetic-filler")
            FileManager.default.createFile(atPath: filler.path, contents: nil)
            let fd = Darwin.open(filler.path, O_WRONLY)
            guard fd >= 0 else { throw POSIXError(.EIO) }
            let block = [UInt8](repeating: 0, count: 65536)
            var diskError: Int32 = 0
            block.withUnsafeBytes { bytes in
                while Darwin.write(fd, bytes.baseAddress, bytes.count) >= 0 { }
                diskError = errno
            }
            Darwin.close(fd)
            guard diskError == ENOSPC else { throw POSIXError(POSIXErrorCode(rawValue: diskError) ?? .EIO) }
            var writeRejected = false
            do { try writer.append(pcm(240_000, start: 240_000)) } catch { writeRejected = true }
            writer.abandon()
            try FileManager.default.removeItem(at: filler)
            let result = try journal.recover()
            guard writeRejected, result.pending.count == 1, result.pending[0].sampleCount == 240_000 else {
                throw NSError(domain: "APP008.full-volume", code: 1)
            }
            print("APP-008: isolated HFS+ volume returned ENOSPC; writer stopped and the sealed segment recovered.")
            return
        }
        if args[1] == "mid-write" {
            try writer.append(pcm(60_000, start: 240_000))
            try waitForKill()
        } else {
            writer.checkpoint = { point in
                if (args[1] == "media-renamed" && point == .mediaRenamed)
                    || (args[1] == "committed" && point == .manifestCommitted) { try waitForKill() }
            }
            try writer.append(pcm(240_000, start: 240_000))
        }
    }
}
