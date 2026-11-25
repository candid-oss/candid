import AppKit
import CandidLocalization
import SwiftUI

/// Library rows and timeline menus use the same native media originals and detail query.
@MainActor enum MediaOriginalActions {
    static func files(from data: Data) throws -> [URL] {
        guard let answer = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw MediaWorkspaceError.malformed }
        let paths: [String]
        if let path = answer["path"] as? String { paths = [path] }
        else if answer["format"] as? String == "candid-playlist/1", let parts = answer["parts"] as? [[String: Any]], !parts.isEmpty {
            paths = try parts.map { part in
                guard let path = part["path"] as? String else { throw MediaWorkspaceError.malformed }
                return path
            }
        } else { throw MediaWorkspaceError.malformed }
        guard paths.allSatisfy({ $0.hasPrefix("/") }) else { throw MediaWorkspaceError.malformed }
        return paths.map { URL(fileURLWithPath: $0) }
    }

    static func open(_ item: MediaLibraryRecord, store: MediaWorkspaceStore, reveal: Bool) async {
        do {
            let client = try MediaLibraryClient(profile: store.profile)
            let urls = try files(from: await client.content(item.id))
            if reveal { NSWorkspace.shared.activateFileViewerSelecting(urls); return }
            guard let first = urls.first else { throw MediaWorkspaceError.malformed }
            // Give the external viewer a correctly named copy; it cannot modify the stored original.
            let filename = item.filename
            let copy = try await Task.detached(priority: .utility) {
                let manager = FileManager.default
                let directory = manager.temporaryDirectory.appendingPathComponent("candid-open-originals", isDirectory: true)
                try manager.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
                let keys: Set<URLResourceKey> = [.fileSizeKey, .contentModificationDateKey, .isRegularFileKey]
                let files = try manager.contentsOfDirectory(at: directory, includingPropertiesForKeys: Array(keys))
                    .compactMap { url -> (URL, Int, Date)? in
                        guard let values = try? url.resourceValues(forKeys: keys), values.isRegularFile == true else { return nil }
                        return (url, values.fileSize ?? 0, values.contentModificationDate ?? .distantPast)
                    }.sorted { $0.2 < $1.2 }
                let required = try first.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
                var total = files.reduce(required) { $0 + $1.1 }
                for file in files where total > 1_073_741_824 || file.2 < Date().addingTimeInterval(-86_400) {
                    try manager.removeItem(at: file.0); total -= file.1
                }
                let name = URL(fileURLWithPath: filename)
                let readableName = String(decoding: name.deletingPathExtension().lastPathComponent.utf8.prefix(120), as: UTF8.self)
                let suffix = String(name.pathExtension.prefix(12))
                let destination = directory.appendingPathComponent(readableName + "-" + UUID().uuidString.prefix(8) + (suffix.isEmpty ? "" : "." + suffix))
                try manager.copyItem(at: first, to: destination)
                return destination
            }.value
            if !NSWorkspace.shared.open(copy) { throw MediaWorkspaceError.malformed }
        } catch { store.error = L("原件暂不可用，可重试。") }
    }

    static func copyText(_ item: MediaLibraryRecord, store: MediaWorkspaceStore) async {
        do {
            let detail = store.isDemo ? item : try await MediaLibraryClient(profile: store.profile).item(item.id)
            let text = detail.segments.map(\.text).joined(separator: "\n")
            guard !text.isEmpty else { store.error = L("这条资料还没有识别文字。"); return }
            NSPasteboard.general.clearContents(); NSPasteboard.general.setString(text, forType: .string)
        } catch { store.error = L("文字读取失败，可重试。") }
    }
}

struct MediaContextActions: View {
    let item: MediaLibraryRecord
    let store: MediaWorkspaceStore
    let edit: () -> Void
    let remove: () -> Void
    var body: some View {
        Button((item.recordingParts?.count ?? 0) > 1 ? L("打开第一段原件") : L("打开原件")) { Task { await MediaOriginalActions.open(item, store: store, reveal: false) } }.disabled(store.isDemo)
        Button(L("在 Finder 中显示原件")) { Task { await MediaOriginalActions.open(item, store: store, reveal: true) } }.disabled(store.isDemo)
        Button(L("复制文字")) { Task { await MediaOriginalActions.copyText(item, store: store) } }
        Button(L("复制引用")) {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(L("资料：%@\n时间：%@\n原始文件：%@\nmedia_id: %@", item.displayTitle, item.displayedDate.ISO8601Format(), item.filename, item.id.uuidString.lowercased()), forType: .string)
        }
        Button(L("编辑标题与备注"), action: edit).disabled(store.isDemo || store.busy)
        if item.kind == .audio {
            RecordingTranscriptionActions(id: item.id, store: store,
                                          active: item.processingStatus.isActive || item.processing?.isActive == true)
        }
        Divider() // style-exempt: native context menu separator
        Button(L("删除…"), role: .destructive, action: remove).disabled(store.isDemo || store.busy)
    }
}

struct RecordingTranscriptionActions: View {
    let id: UUID
    let store: MediaWorkspaceStore
    let active: Bool

    var body: some View {
        ForEach(MediaLibraryRecord.Mode.allCases) { mode in
            Button(mode.transcriptionTitle(retrying: true)) { Task { await store.recognize(id, mode: mode) } }
                .disabled(store.isDemo || store.busy || active)
        }
    }
}
