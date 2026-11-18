import DesktopCore
import CandidLocalization
import Foundation

/// 导出: an independently readable package of the chosen activities (desktop-export/0.3): `records.jsonl`, a
/// `README.md` listing, the screenshots under `images/`, and `manifest.json` with every file's hash. Activities are
/// re-read from the timeline library in full at export time, so a UI page never truncates an export.
enum DesktopExport {
    static func write(events: [DesktopTimelineEvent], store: DesktopStore, to destination: URL) async throws {
        guard destination.isFileURL, events.count <= 10_000 else { throw DesktopStoreError.invalidRecord }
        let manager = FileManager.default
        guard !manager.fileExists(atPath: destination.path) else { throw DesktopStoreError.destinationExists }
        var groups: [[DesktopRecord]] = []
        var total = 0
        for event in events {
            guard let full = try await store.activity(id: event.id, limit: 10_000) else { throw DesktopStoreError.deletedRecord }
            let records = full.allRecords
            total += records.count
            guard total <= 10_000 else { throw DesktopStoreError.invalidRecord }
            groups.append(records)
        }
        var images: [String: URL] = [:]
        for hash in Set(groups.joined().compactMap(\.imageHash)) {
            guard let url = await store.imageURL(hash: hash) else { throw DesktopStoreError.missingImage }
            images[hash] = url
        }
        let temporary = destination.deletingLastPathComponent().appendingPathComponent(".desktop-export-" + UUID().uuidString)
        try manager.createDirectory(at: temporary, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        do {
            let encoder = JSONEncoder(); encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]; encoder.dateEncodingStrategy = .iso8601
            var jsonl = Data()
            var markdown = L("# 桌面记录导出\n\n范围：用户选定的活动。格式 desktop-export/0.3；不包含云端数据或AI推断。\n\n")
            var entries: [[String: String]] = []
            var seen = Set<String>()
            for records in groups {
                for (index, record) in records.enumerated() where seen.insert(record.id).inserted {
                    jsonl.append(try encoder.encode(record)); jsonl.append(0x0A)
                    let indentation = index == 0 ? "" : "  "
                    markdown += "\(indentation)- \(record.timestamp.ISO8601Format()) · \(record.kind.title) · \(markdownText(record.displayTitle))\n"
                    func field(_ label: String, _ value: String?) {
                        if let value { markdown += L("%@  %@：%@\n", indentation, label, markdownText(value)) }
                    }
                    field(L("详情"), record.detail.isEmpty ? nil : record.detail)
                    field(L("触发"), record.trigger); field(L("活动标识"), record.activityID)
                    if let context = record.context {
                        field(L("可读文字"), context.readableText); field(L("选择文字"), context.selectedText)
                        field(L("焦点角色"), context.focusedRole); field(L("焦点标签"), context.focusedLabel)
                        field(L("文字状态"), context.textStatus); field(L("热状态"), context.thermalState)
                        field(L("受保护"), context.isProtected.map { $0 ? L("是") : L("否") })
                        field(L("输入法组合中"), context.isComposing.map { $0 ? L("是") : L("否") })
                        field(L("会话活跃"), context.sessionActive.map { $0 ? L("是") : L("否") })
                        field(L("电池供电"), context.onBattery.map { $0 ? L("是") : L("否") })
                        field(L("低电量模式"), context.lowPower.map { $0 ? L("是") : L("否") })
                        field(L("空闲秒数"), context.idleSeconds.map { String($0) })
                    }
                    if let hash = record.imageHash, let source = images[hash] {
                        let folder = temporary.appendingPathComponent("images", isDirectory: true)
                        try manager.createDirectory(at: folder, withIntermediateDirectories: true)
                        // New originals are WebP, older ones JPEG: the file keeps its own format and extension.
                        let original = try Data(contentsOf: source)
                        guard DesktopEventMapping.sha256(original) == hash else { throw DesktopStoreError.missingImage }
                        let name = hash + "." + (DesktopImageFormat(original) ?? .jpeg).fileExtension
                        let target = folder.appendingPathComponent(name)
                        if !manager.fileExists(atPath: target.path) {
                            try original.write(to: target)
                            entries.append(["path": "images/\(name)", "sha256": hash])
                        }
                        markdown += L("%@  [图片](images/%@)\n", indentation, name)
                    }
                }
            }
            try jsonl.write(to: temporary.appendingPathComponent("records.jsonl"))
            let markdownData = Data(markdown.utf8)
            try markdownData.write(to: temporary.appendingPathComponent("README.md"))
            entries.append(["path": "records.jsonl", "sha256": DesktopEventMapping.sha256(jsonl)])
            entries.append(["path": "README.md", "sha256": DesktopEventMapping.sha256(markdownData)])
            let manifest: [String: Any] = ["format": "desktop-export/0.3", "created_at": Date().ISO8601Format(), "record_count": seen.count,
                                           "files": entries, "missing_refs": [], "scope": "selected_activities"]
            try JSONSerialization.data(withJSONObject: manifest, options: [.prettyPrinted, .sortedKeys]).write(to: temporary.appendingPathComponent("manifest.json"))
            try manager.moveItem(at: temporary, to: destination)
        } catch { try? manager.removeItem(at: temporary); throw error }
    }

    static func markdownText(_ value: String) -> String {
        var result = value.replacingOccurrences(of: "\n", with: " ").replacingOccurrences(of: "\r", with: " ")
        result = result.replacingOccurrences(of: "&", with: "&amp;").replacingOccurrences(of: "<", with: "&lt;").replacingOccurrences(of: ">", with: "&gt;")
        for character in ["\\", "[", "]", "(", ")", "*", "_", "`", "!", "#"] { result = result.replacingOccurrences(of: character, with: "\\" + character) }
        return result
    }
}
