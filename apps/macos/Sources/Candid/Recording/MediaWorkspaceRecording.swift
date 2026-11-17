import Foundation

extension MediaWorkspaceStore {
    var pendingRecordingRows: [RecordingSegment] {
        Dictionary(grouping: recordingInventory.pending, by: \.recordingID).values
            .compactMap { $0.min { $0.index < $1.index } }
            .filter { part in !records.contains { $0.id == part.recordingID } }
            .sorted { $0.occurredAt < $1.occurredAt }
    }
    var pendingRecordingCount: Int { Set(recordingInventory.pending.map(\.recordingID)).count }
    var recoveredRecordingCount: Int { recordingInventory.segments.filter(\.recovered).count }

    func recordingLabel(for id: UUID) -> String? {
        let parts = recordingInventory.segments.filter { $0.recordingID == id || $0.id == id }
        guard let first = parts.first else { return nil }
        let end = parts.map { $0.sampleStart + $0.sampleCount }.max() ?? 0
        return "\(String(format: "%.1f", Double(end) / Double(first.sampleRate))) 秒"
            + (parts.contains(where: \.recovered) ? " · 中断后恢复" : " · 已保存")
    }

    /// Startup recovery and boundary uploads use the same stable IDs and outbox.
    /// Receipts survive successful uploads and intentional media-library deletion.
    func syncRecordings() async {
        guard !isDemo, !capture.isStartingRecording else { return }
        if recordingSyncing { recordingSyncAgain = true; return }
        recordingSyncing = true; recordingScanning = true
        defer {
            recordingSyncing = false; recordingScanning = false
            if recordingSyncAgain {
                recordingSyncAgain = false
                Task { await self.syncRecordings() }
            }
        }
        let journal = recordingJournal
        let active = capture.recordingID
        do {
            let inventory = try await Task.detached(priority: .utility) { try journal.recover(excluding: active) }.value
            recordingScanning = false
            recordingInventory = inventory
            recordingWarning = inventory.warnings.first
            recordingRecoveryLoaded = true
            let client = try MediaLibraryClient(profile: profile)
            for segment in inventory.pending {
                // Revalidate immediately before upload; changed bytes never reuse an ID.
                try await Task.detached(priority: .utility) {
                    let verified = try RecordingJournal.validate(journal.file(segment, ".wav"), sampleRate: segment.sampleRate)
                    guard verified.hash == segment.sha256, verified.frames == segment.sampleCount else { throw RecordingJournalError.invalid }
                }.value
                let item: MediaLibraryRecord
                do { item = try await client.upload(segment.draft, file: journal.file(segment, ".wav")) }
                catch MediaWorkspaceError.rejected(409, _) {
                    guard try await client.recordingWasDeleted(segment.recordingID) else { throw MediaWorkspaceError.malformed }
                    // An authenticated, matching tombstone is a terminal acknowledgement.
                    try await Task.detached(priority: .utility) { try journal.acknowledge(segment) }.value
                    recordingInventory.pending.removeAll { $0.id == segment.id }
                    continue
                }
                guard item.id == segment.recordingID,
                      item.recordingParts?.contains(where: { $0.id == segment.id && $0.sampleStart == segment.sampleStart && $0.sampleCount == segment.sampleCount }) == true
                else { throw MediaWorkspaceError.malformed }
                try await Task.detached(priority: .utility) { try journal.acknowledge(segment) }.value
                recordingInventory.pending.removeAll { $0.id == segment.id }
                replace(item)
                if selectedRecordingID == segment.id { selectedRecordingID = nil; selectedID = item.id }
            }
            let pendingIDs = Set(inventory.pending.map(\.id))
            for segment in inventory.segments where !pendingIDs.contains(segment.id) {
                do { replace(try await client.linkRecording(segment.draft)) }
                catch MediaWorkspaceError.rejected(404, _) { continue } // Deleted original stays deleted.
                catch MediaWorkspaceError.rejected(409, _) { continue } // Recording tombstone wins.
            }
            for id in Set(inventory.segments.map(\.recordingID)) {
                let session = try JSONDecoder().decode(RecordingSession.self, from: Data(contentsOf: journal.directory(id).appendingPathComponent("session.json")))
                // Re-send state after lost receipts, including stops exactly at a boundary.
                do { replace(try await client.recordingState(id, state: session.state)) }
                catch MediaWorkspaceError.rejected(404, _) { continue }
                catch MediaWorkspaceError.rejected(409, _) { continue }
            }
        } catch {
            recordingWarning = (recordingInventory.pending.isEmpty ? "资料库暂时无法连接，请重试。" : "已保存录音留在本机，等待入库。") + error.localizedDescription
        }
    }
}

extension RecordingSegment {
    var draft: MediaDraft {
        MediaDraft(id: id, filename: filename, storedName: stem + ".wav", kind: "audio",
                   source: (input ?? .microphone).source, occurredAt: occurredAt, needsReview: false,
                   recording: RecordingUpload(recordingID: recordingID, segmentIndex: index, sampleStart: sampleStart,
                                              sampleCount: sampleCount, sampleRate: sampleRate,
                                              startedAt: occurredAt.addingTimeInterval(-Double(sampleStart) / Double(sampleRate))))
    }
}
