import Foundation
import CandidLocalization
import Observation

/// Search the core's complete media records, including transcript and notes.
/// It does not replace the library's selection or filtered list.
@MainActor @Observable final class TimelineMediaSearch {
    private(set) var items: [MediaLibraryRecord] = []
    private(set) var version = 0
    private(set) var loadedQuery: String? = nil
    private var generation = UUID()
    func records(query: String, media: MediaWorkspaceStore?) -> [MediaLibraryRecord] {
        if loadedQuery == nil && query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return media?.records ?? [] }
        return loadedQuery == query ? items : []
    }
    private static func fingerprint(_ records: [MediaLibraryRecord]) -> [String] {
        records.map { $0.id.uuidString + $0.displayTitle + ($0.notes ?? "") + String($0.segments.count) + $0.displayedDate.ISO8601Format() }
    }

    func refresh(query: String, media: MediaWorkspaceStore?, client: MediaLibraryClient? = nil) async {
        let request = UUID(); generation = request
        do {
            let found: [MediaLibraryRecord]
            if media?.isDemo == true {
                found = (media?.records ?? []).filter { record in
                    query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || ([record.displayTitle, record.filename, record.notes ?? ""] + record.segments.map(\.text)).contains { $0.localizedCaseInsensitiveContains(query) }
                }
            } else if let client { found = try await client.list(query: query) }
            else if let channel = TimelineService.shared.channel { found = try await MediaLibraryClient(channel: channel).list(query: query) }
            else { found = [] }
            guard !Task.isCancelled, request == generation else { return }
            let filtered = found.filter { record in
                query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || ([record.displayTitle, record.filename, record.notes ?? ""] + record.segments.map(\.text)).contains { $0.localizedCaseInsensitiveContains(query) }
            }
            // Only a real change counts; an identical answer must not redraw the timeline.
            if Self.fingerprint(filtered) != Self.fingerprint(items) || query != loadedQuery { items = filtered; loadedQuery = query; version += 1 }
        } catch {
            guard request == generation else { return }
            items = []; loadedQuery = query
            NotificationCenter.default.post(name: .candidActionError, object: L("资料搜索暂不可用，可重试"))
        }
    }
}
