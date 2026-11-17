import SwiftUI

struct RecordingSpeakerPolicy: Codable, Equatable {
    var version = 1
    var preset = "balanced"
    var maxSpeakers = 0
}

/// Optional tuning for a whole recording; ordinary use needs no segment-level controls.
struct RecordingSpeakerSettings: View {
    let item: MediaLibraryRecord
    let store: MediaWorkspaceStore
    let close: () -> Void
    @ViewState private var policy: RecordingSpeakerPolicy
    init(item: MediaLibraryRecord, store: MediaWorkspaceStore, close: @escaping () -> Void) {
        self.item = item; self.store = store; self.close = close
        _policy = ViewState(initialValue: item.speakerPolicy ?? RecordingSpeakerPolicy())
    }
    var body: some View {
        VStack(alignment: .leading, spacing: Space.m) {
            Text("整条录音的说话人").typography(.headline)
            SettingRow(title: "合并强度") {
                MenuChoice(options: [("conservative", "保守"), ("balanced", "均衡"), ("merge_more", "更多合并")], selection: $policy.preset)
            }
            SettingRow(title: "人数上限") {
                MenuChoice(options: [(0, "自动")] + (1...12).map { ($0, "\($0) 人") }, selection: $policy.maxSpeakers)
            }
            Text("自动按声音区分，再匹配人物。调整后保留原文和时间位置。").typography(.caption).foregroundStyle(.secondary)
            HStack {
                QuietButton(title: "取消", action: close)
                Spacer()
                PrimaryButton(title: "重新整理") { Task { await store.reconcileSpeakers(item, policy: policy); close() } }
                    .disabled(store.busy)
            }
        }.padding(Space.m).frame(width: 360)
    }
}

extension MediaWorkspaceStore {
    func reconcileSpeakers(_ item: MediaLibraryRecord, policy: RecordingSpeakerPolicy) async {
        guard !isDemo, !busy else { return }
        do {
            guard let channel = TimelineService.shared.channel else { throw MediaWorkspaceError.configuration }
            let encoded = JSONEncoder(); encoded.keyEncodingStrategy = .convertToSnakeCase
            let value = try JSONSerialization.jsonObject(with: encoded.encode(policy))
            _ = try await channel.send("POST", "/local/workflows", body: JSONSerialization.data(withJSONObject: ["action":"speaker_policy", "id":item.id.uuidString.lowercased(), "policy":value])).checked()
            await refresh()
        } catch { self.error = error.localizedDescription }
    }
}
