import Foundation
import CandidLocalization
import CandidClient

/// Hands outbox entries to the core: consecutive record entries go as one ingest (up to 256 events) with their files
/// linked into the core's staging directory; deletions go alone. Shared by the App and the offline diagnostics.
public enum DesktopDelivery {
    public static let sourceName = L("Mac 桌面")

    /// The desktop source (registered once by kind and name; the same one the ledger import used).
    public static func registerSource(_ channel: LocalChannel) async throws -> String {
        try await channel.registerSource(kind: "mac.desktop", name: sourceName, policy: SourcePolicy(store_text: true, store_media: true))
    }

    /// Sends the leading entries; returns the sequence numbers the core confirmed (or refused for good, with a note).
    public static func deliver(_ entries: [DesktopOutbox.Entry], channel: LocalChannel, sourceID: String) async throws -> ([UInt64], String?) {
        guard let first = entries.first else { return ([], nil) }
        switch first.command {
        case .deleteActivity(let id):
            _ = try await channel.deleteDesktopActivity(id)
            return ([first.seq], nil)
        case .retention(let before):
            _ = try await channel.deleteDesktopBefore(before)
            return ([first.seq], nil)
        case .ingest:
            var events: [IngestEvent] = []
            var seqs: [UInt64] = []
            var staged: [URL] = []
            let staging = URL(fileURLWithPath: try await channel.health().staging_dir, isDirectory: true)
            defer { staged.forEach { try? FileManager.default.removeItem(at: $0) } }
            for entry in entries {
                guard case .ingest(var batch) = entry.command, events.count + batch.count <= 256 else { break }
                for i in batch.indices {
                    for b in batch[i].blobs.indices {
                        guard let file = entry.files[batch[i].blobs[b].sha256] else { throw DesktopStoreError.missingImage }
                        let target = staging.appendingPathComponent("app-\(UUID().uuidString).\(file.pathExtension)")
                        try stage(file, to: target)
                        staged.append(target)
                        batch[i].blobs[b].staging_path = target.path
                    }
                }
                events += batch; seqs.append(entry.seq)
            }
            do {
                _ = try await channel.ingest(IngestBatch(source_id: sourceID, events: events, on_missing_parent: "detach", merge_text: true))
            } catch CoreError.service(400, let code, let message) {
                // A batch the core can never take (it fails validation): keep it out of the way and say so.
                return (seqs, L("有 %@ 组记录无法写入时间线库（%@）：%@", seqs.count, code, message))
            }
            return (seqs, nil)
        }
    }

    /// Hard-links an outbox file into the core's staging directory (same volume), else copies it.
    static func stage(_ file: URL, to target: URL) throws {
        do { try FileManager.default.linkItem(at: file, to: target) }
        catch {
            let part = target.appendingPathExtension("part")
            try FileManager.default.copyItem(at: file, to: part)
            try FileManager.default.moveItem(at: part, to: target)
        }
    }
}
