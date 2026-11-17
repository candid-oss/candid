import AppKit
import SwiftUI
import DesktopCore
import CandidClient

/// Shared menu for carousel cards and their icons on the time axis.
struct EpisodeContextActions: View {
    let episode: TimelineEpisode
    let session: DesktopSession
    let media: MediaWorkspaceStore?
    var resolvedMediaRecord: MediaLibraryRecord? = nil
    let remove: () -> Void
    private var record: MediaLibraryRecord? { resolvedMediaRecord ?? media?.records.first { $0.id.uuidString == episode.mediaID } }
    var body: some View {
        Button((record?.recordingParts?.count ?? 0) > 1 ? "打开第一段原件" : "打开原件") { original(reveal: false) }
            .disabled(episode.screenshotSpans.isEmpty && record == nil)
        Button("在 Finder 中显示原件") { original(reveal: true) }
            .disabled(episode.screenshotSpans.isEmpty && record == nil)
        Button("复制文字") { Task {
            if let record, let media { await MediaOriginalActions.copyText(record, store: media); return }
            let text: String
            if let span = episode.latest, let event = await session.eventDetail(id: span.id) { text = event.observations.compactMap { $0.context?.readableText ?? $0.context?.selectedText }.joined(separator: "\n\n") }
            else { text = episode.title }
            NSPasteboard.general.clearContents(); NSPasteboard.general.setString(text, forType: .string)
        } }
        Button("复制引用") { Task {
            if let reference = await reference() { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(reference, forType: .string) }
        } }
        Button("编辑标题与备注") { NotificationCenter.default.post(name: .candidEditFragment, object: FragmentEditTarget(activities: episode.spans.map(\.id), mediaID: record?.id)) }.disabled(session.isDemo)
        Button("添加备注") { Task { if let reference = await reference() { NotificationCenter.default.post(name: .candidNoteReference, object: reference) } } }.disabled(session.isDemo)
        Button("重新识别") { Task {
            if let record, let media { await media.recognize(record, mode: .quick) }
            else if let span = episode.screenshotSpans.last, let shot = await session.screenshotRecords(forEventID: span.id).last { session.recognition.runManually(.ocr, screenshot: shot) }
        } }.disabled(session.isDemo || (episode.screenshotSpans.isEmpty && record?.kind != .audio))
        if episode.kind == .app, !episode.bundleID.isEmpty {
            Button("以后不记这个 App") { session.setExcludedApp(episode.bundleID, true) }.disabled(session.isDemo)
        }
        Divider() // style-exempt: native context menu separator
        Button("删除…", role: .destructive, action: remove).disabled(session.isDemo)
    }
    private func reference() async -> String? {
        if let record { return "资料：\(record.displayTitle)\n时间：\(record.displayedDate.ISO8601Format())\nmedia_id: \(record.id.uuidString)" }
        guard let id = episode.latest?.id else { return nil }
        return await TimelineSourceReference.resolve(activityID: id)
    }
    private func original(reveal: Bool) {
        Task {
            let url: URL?
            if let record, let media {
                await MediaOriginalActions.open(record, store: media, reveal: reveal)
                return
            } else if let span = episode.screenshotSpans.last, let shot = await session.screenshotRecords(forEventID: span.id).last, let hash = shot.imageHash {
                url = await session.verifiedImageURL(hash: hash)
            } else { url = nil }
            guard let url else { NotificationCenter.default.post(name: .candidActionError, object: "原件暂不可用，可重试"); return }
            if reveal { NSWorkspace.shared.activateFileViewerSelecting([url]) } else { NSWorkspace.shared.open(url) }
        }
    }
}
extension Notification.Name { static let candidNoteReference = Notification.Name("Candid.noteReference") }


@MainActor enum TimelineSourceReference {
    static func resolve(activityID: String, screenshotID: String? = nil) async -> String? {
        guard let channel = TimelineService.shared.channel else { return nil }
        struct Answer: Decodable { var activity: AppActivity? }
        do {
            let answer: Answer = try await channel.app("app_activity", ["id": .string(activityID), "limit": .int(10000)])
            guard let activity = answer.activity else { return nil }
            let record = screenshotID.flatMap { id in activity.screenshots.first { $0.id == id } } ?? activity.anchor
            return "\(record.window_title) · \(Date(timeIntervalSince1970: Double(record.ts_ms) / 1000).ISO8601Format())\nevent_id: \(record.event_id)"
        } catch { NotificationCenter.default.post(name: .candidActionError, object: "来源引用暂不可用，可重试"); return nil }
    }
}

struct ScreenshotContextActions: View {
    let record: DesktopRecord
    let activityID: String
    let text: String
    let session: DesktopSession
    let remove: () -> Void
    var body: some View {
        Button("打开原件") { original(reveal: false) }
        Button("在 Finder 中显示") { original(reveal: true) }
        Button("复制文字") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(text, forType: .string) }.disabled(text.isEmpty)
        Button("复制引用") { Task {
            if let ref = await TimelineSourceReference.resolve(activityID: activityID, screenshotID: record.id) { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(ref, forType: .string) }
        } }
        Button("编辑标题与备注") { NotificationCenter.default.post(name: .candidEditFragment, object: FragmentEditTarget(activities: [activityID], screenshotID: record.id)) }.disabled(session.isDemo)
        Button("添加备注") { Task {
            if let ref = await TimelineSourceReference.resolve(activityID: activityID, screenshotID: record.id) { NotificationCenter.default.post(name: .candidNoteReference, object: ref) }
        } }.disabled(session.isDemo)
        Button("重新识别") { session.recognition.runManually(.ocr, screenshot: record) }.disabled(session.isDemo)
        Button("以后不记这个 App") { session.setExcludedApp(record.bundleID, true) }.disabled(session.isDemo || record.bundleID.isEmpty)
        Button("删除所在活动…", role: .destructive, action: remove).disabled(session.isDemo)
    }
    private func original(reveal: Bool) { Task {
        guard let hash = record.imageHash, let url = await session.verifiedImageURL(hash: hash) else { NotificationCenter.default.post(name: .candidActionError, object: "原件暂不可用"); return }
        if reveal { NSWorkspace.shared.activateFileViewerSelecting([url]) } else { NSWorkspace.shared.open(url) }
    } }
}
