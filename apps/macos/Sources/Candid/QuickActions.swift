import SwiftUI
import CandidLocalization
import CandidClient

extension Notification.Name {
    static let candidActionError = Notification.Name("Candid.actionError")
    static let candidAgentConsentChanged = Notification.Name("Candid.agentConsentChanged")
    static let candidQuickAction = Notification.Name("Candid.quickAction")
}

enum QuickAction: String, CaseIterable, Codable, Identifiable {
    case screenshot, marker, recording, note, search
    var id: String { rawValue }
    var title: String { switch self { case .screenshot: L("截图"); case .marker: L("标记此刻"); case .recording: L("开始或停止录音"); case .note: L("快速记一笔"); case .search: L("搜索") } }
}

/// Notes are evidence in the existing core, not a second notes database.
@MainActor @Observable final class QuickNote {
    var text = "" { didSet { if text != oldValue { key = UUID().uuidString; created = Date() } } }
    var target: String? { didSet { if target != oldValue { key = UUID().uuidString; created = Date() } } }
    var error: String?
    var saving = false
    private var key = UUID().uuidString
    private var created = Date()
    private let channel: LocalChannel?
    init(channel: LocalChannel? = nil) { self.channel = channel }
    func save() async -> Bool {
        guard !saving, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return false }
        let submittedText = text, submittedKey = key, submittedDate = created, submittedTarget = target
        saving = true; defer { saving = false }
        do {
            guard let channel = channel ?? TimelineService.shared.channel else { throw CoreError.unreachable(L("尚未就绪")) }
            let source = try await channel.registerSource(kind: "manual", name: L("手动笔记"), policy: SourcePolicy(store_text: true, store_media: false))
            var event = IngestEvent(source_key: submittedKey, kind: "note", start_ms: Int64(submittedDate.timeIntervalSince1970 * 1000))
            event.text = [IngestText(field: "body", content: submittedText)]
            if let submittedTarget {
                let bytes = try await channel.tool("source_query", arguments: JSONEncoder().encode(["action": CoreJSON.string("reference"), "reference": .string(submittedTarget)]))
                let links = try JSONDecoder().decode(CoreJSON.self, from: bytes)
                event.payload["source_links"] = links
                event.payload["reference"] = .string(submittedTarget)
            }
            let result = try await channel.ingest(IngestBatch(source_id: source, events: [event]))
            guard result.conflicts == 0, result.tombstoned == 0 else { throw CoreError.badResponse }
            if let submittedTarget {
                _ = try await channel.tool("source_query", arguments: JSONEncoder().encode(["action": CoreJSON.string("reference"), "reference": .string(submittedTarget)]))
            }
            if key == submittedKey { text = ""; key = UUID().uuidString; created = Date() }
            error = nil
            return true
        } catch { self.error = L("笔记未保存，可重试"); return false }
    }
}

struct QuickNoteView: View {
    @Bindable var model: QuickNote
    let close: () -> Void
    var body: some View {
        VStack(alignment: .leading, spacing: Space.s) {
            Text(model.target == nil ? L("快速记一笔") : L("添加备注")).typography(.headline)
            TextField(L("写下要记住的内容"), text: $model.text, axis: .vertical).textFieldStyle(.plain).typography(.body)
            if let error = model.error { Text(error).typography(.secondary).foregroundStyle(.red) }
            HStack { QuietButton(title: L("取消"), action: close); Spacer(); QuietButton(title: model.saving ? L("保存中…") : L("保存")) { Task { if await model.save(), model.text.isEmpty { close() } } }.disabled(model.saving || model.text.isEmpty) }
        }.padding(Space.m).frame(width: 340).floatingSurface()
    }
}
