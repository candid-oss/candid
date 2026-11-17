import Darwin
import CandidLocalization
import Foundation
import CandidClient

/// What the capture asks the timeline core to do, in order.
public enum DesktopCommand: Codable, Sendable, Equatable {
    /// Timeline events; a blob's `staging_path` names a file kept next to the entry.
    case ingest([IngestEvent])
    /// Delete a whole activity by its anchor's record ID.
    case deleteActivity(String)
    /// Delete desktop records before a time (retention).
    case retention(beforeMs: Int64)
}

/// The capture's durable, bounded queue in front of the core (ADR-024: the core is a separate process and may be
/// restarting). A record counts as saved once its entry is on disk here; a sender hands entries to the core in order
/// and removes them only after the core confirmed them, so nothing is lost while the core is away and a resend after
/// a lost answer is a duplicate the core ignores (`source_key`).
///
/// Layout: `<seq>.json` per entry (16 digits), `<seq>-<sha256>.<ext>` for its files. Files are written and synced
/// first, then the entry is renamed into place under the capture gate; a crash leaves only `.part` files or files
/// without an entry, which the next start removes.
public actor DesktopOutbox {
    public struct Limits: Sendable {
        public var items: Int
        public var bytes: Int64
        public init(items: Int = 10_000, bytes: Int64 = 1024 * 1024 * 1024) { self.items = items; self.bytes = bytes }
    }

    public struct Entry: Sendable, Equatable {
        public var seq: UInt64
        public var command: DesktopCommand
        /// Absolute paths of the entry's files by blob hash.
        public var files: [String: URL]
    }

    private struct Stored: Codable { var command: DesktopCommand; var files: [String: String] }

    public nonisolated let directory: URL
    public let limits: Limits
    private var index: [(seq: UInt64, bytes: Int64)] = []
    private var nextSeq: UInt64 = 1
    private var waiters: [CheckedContinuation<Void, Never>] = []

    public init(directory: URL, limits: Limits = Limits()) throws {
        self.directory = directory.standardizedFileURL
        self.limits = limits
        let manager = FileManager.default
        try manager.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        var entries: [UInt64: Int64] = [:]
        var files: [(UInt64, URL)] = []
        for url in try manager.contentsOfDirectory(at: directory, includingPropertiesForKeys: [.fileSizeKey]) {
            let name = url.lastPathComponent
            if name.hasSuffix(".part") { try? manager.removeItem(at: url); continue }
            let size = Int64((try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0)
            if name.hasSuffix(".json"), let seq = UInt64(name.dropLast(5)) { entries[seq, default: 0] += size }
            else if let dash = name.firstIndex(of: "-"), let seq = UInt64(name[..<dash]) { files.append((seq, url)) }
        }
        for (seq, url) in files {
            if entries[seq] == nil { try? manager.removeItem(at: url) }   // written before a crash, never committed
            else { entries[seq, default: 0] += Int64((try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0) }
        }
        index = entries.sorted { $0.key < $1.key }.map { ($0.key, $0.value) }
        nextSeq = (index.last?.seq ?? 0) + 1
    }

    public var count: Int { index.count }
    public var bytes: Int64 { index.reduce(0) { $0 + $1.bytes } }

    private func name(_ seq: UInt64) -> String { String(format: "%016llu", seq) }

    /// Writes `command` with `files` (hash → bytes, with the extension each gets) durably. With a gate, the entry is
    /// committed only while `epoch` is still current; a stale write leaves nothing behind.
    @discardableResult
    public func append(_ command: DesktopCommand, files: [String: (data: Data, ext: String)] = [:], gate: CaptureCommitGate? = nil, epoch: UInt64 = 0) throws -> UInt64 {
        let size = Int64(files.values.reduce(0) { $0 + $1.data.count })
        guard index.count < limits.items, bytes + size <= limits.bytes else { throw DesktopStoreError.outboxFull(index.count) }
        let seq = nextSeq; nextSeq += 1
        let manager = FileManager.default
        var written: [URL] = []
        var names: [String: String] = [:]
        do {
            for (sha, file) in files {
                try DesktopEventMapping.validateHash(sha)
                let target = directory.appendingPathComponent("\(name(seq))-\(sha).\(file.ext)")
                try Self.writeDurably(file.data, to: target)
                written.append(target)
                names[sha] = target.lastPathComponent
            }
            let encoder = JSONEncoder(); encoder.outputFormatting = [.sortedKeys]
            let entry = try encoder.encode(Stored(command: command, files: names))
            let target = directory.appendingPathComponent(name(seq) + ".json")
            let part = target.appendingPathExtension("part")
            try Self.writeDurably(entry, to: part, rename: false)
            written.append(part)
            if let gate { try gate.beginAdmission(epoch: epoch) }
            defer { gate?.endAdmission() }
            try manager.moveItem(at: part, to: target)
            Self.syncDirectory(directory)
            index.append((seq, size + Int64(entry.count)))
        } catch {
            for url in written { try? manager.removeItem(at: url) }
            throw error
        }
        let ready = waiters; waiters.removeAll()
        ready.forEach { $0.resume() }
        return seq
    }

    /// Up to `limit` entries from the front, oldest first.
    public func peek(_ limit: Int) -> [Entry] {
        index.prefix(limit).compactMap { item in
            let url = directory.appendingPathComponent(name(item.seq) + ".json")
            guard let data = try? Data(contentsOf: url), let stored = try? JSONDecoder().decode(Stored.self, from: data) else { return nil }
            return Entry(seq: item.seq, command: stored.command, files: stored.files.mapValues { directory.appendingPathComponent($0) })
        }
    }

    /// Removes confirmed entries and their files.
    public func remove(_ seqs: [UInt64]) {
        let drop = Set(seqs)
        let manager = FileManager.default
        for seq in drop {
            if let names = try? manager.contentsOfDirectory(atPath: directory.path) {
                for file in names where file.hasPrefix(name(seq) + "-") { try? manager.removeItem(at: directory.appendingPathComponent(file)) }
            }
            try? manager.removeItem(at: directory.appendingPathComponent(name(seq) + ".json"))
        }
        index.removeAll { drop.contains($0.seq) }
    }

    /// Drops queued records of an activity the user deleted, so they cannot reach the core afterwards.
    public func dropActivity(runID: String, activityID: String) {
        var drop: [UInt64] = []
        for entry in peek(index.count) {
            guard case .ingest(let events) = entry.command else { continue }
            let all = events.allSatisfy { event in
                guard case .object(let legacy)? = event.payload["legacy"] else { return false }
                return legacy["run_id"] == .string(runID) && legacy["activity_id"] == .string(activityID)
            }
            if all && !events.isEmpty { drop.append(entry.seq) }
        }
        remove(drop)
    }

    /// Suspends until an entry is appended (or returns at once when entries are waiting).
    public func waitForEntries() async {
        if !index.isEmpty { return }
        await withCheckedContinuation { waiters.append($0) }
    }

    // MARK: Files

    public static func writeDurably(_ data: Data, to url: URL, rename: Bool = true) throws {
        let part = rename ? url.appendingPathExtension("part") : url
        guard FileManager.default.createFile(atPath: part.path, contents: data, attributes: [.posixPermissions: 0o600]) else {
            throw DesktopStoreError.unavailable(L("无法写入待发队列"))
        }
        let handle = try FileHandle(forWritingTo: part)
        try handle.synchronize(); try handle.close()
        // rename(2) replaces an existing file atomically (a settings file is rewritten in place).
        if rename, Darwin.rename(part.path, url.path) != 0 {
            try? FileManager.default.removeItem(at: part)
            throw DesktopStoreError.unavailable(L("无法写入 %@", url.lastPathComponent))
        }
    }

    static func syncDirectory(_ directory: URL) {
        let descriptor = open(directory.path, O_RDONLY)
        guard descriptor >= 0 else { return }
        _ = fsync(descriptor); close(descriptor)
    }
}
