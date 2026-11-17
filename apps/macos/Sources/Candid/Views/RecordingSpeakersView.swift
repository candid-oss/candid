import SwiftUI

/// 「说话人」 in a recording (agent-platform-v1 §2.6; mockup ai-pages-v1 ①): each speaker with how sure the voice
/// match is. Clear matches show the name with 「自动」, close ones ask 「可能是 X？」, the rest stay anonymous and
/// need nothing. 「这是谁？」 names a speaker; the core also keeps that voice as samples for next time.
struct RecordingSpeakersView: View {
    let item: MediaLibraryRecord
    let store: MediaWorkspaceStore
    @ViewState private var picking: String?
    @ViewState private var showingSettings = false

    private var speakers: [MediaLibraryRecord.Speaker] { item.speakers ?? [] }

    var body: some View {
        if item.speakerGrouping == nil && (item.speakerProcessing?.isActive == true || item.processingStatus.isActive || item.recordingState == "recording") {
            HStack(spacing: Space.s) {
                if item.recordingState != "recording" { ProgressView().controlSize(.small) }
                Text(item.recordingState == "recording" ? "录音结束后整理说话人" : "正在整理整条录音的说话人").typography(.secondary).foregroundStyle(.secondary)
            }
        } else if item.speakerGrouping != nil || !speakers.isEmpty {
            VStack(alignment: .leading, spacing: 4) {
                HStack {
                    Text(item.speakerGrouping.map { "说话人 · \($0.speakerCount) 人" } ?? "说话人").typography(.headline)
                    Spacer()
                    QuietButton(title: "设置") { showingSettings = true }
                        .popover(isPresented: $showingSettings) { RecordingSpeakerSettings(item: item, store: store) { showingSettings = false } }
                        .disabled(store.isDemo)
                    if store.labelingSpeaker != nil || item.speakerProcessing?.isActive == true { ProgressView().controlSize(.mini).accessibilityLabel("正在保存") }
                }
                ForEach(Array(speakers.enumerated()), id: \.element.id) { index, speaker in
                    row(speaker, tint: Self.tint(index))
                }
                if let count = item.speakerGrouping?.unresolvedSegments, count > 0 {
                    Text("\(count) 处发言归属未定").typography(.caption).foregroundStyle(.secondary)
                }
                if let note = store.speakerNote { Text(note).typography(.caption).foregroundStyle(.secondary).padding(.top, 2) }
                if speakers.allSatisfy({ $0.matchReason == "no_sample" }) {
                    Text("发言过短，暂未提取到可匹配的声纹。").typography(.caption).foregroundStyle(.secondary)
                } else {
                Text("匹配度是和此人样本最像的一段的相似度，不是概率；不低于 \(threshold)% 且明显领先才自动标名字。")
                    .typography(.caption).foregroundStyle(.tertiary).padding(.top, 4)
                }
                if item.speakerProcessing == .idle && item.speakerGrouping == nil {
                    HStack {
                        Text("整条录音的说话人整理已停止").typography(.caption).foregroundStyle(.secondary)
                        QuietButton(title: "重新整理") { Task { await store.reconcileSpeakers(item, policy: item.speakerPolicy ?? RecordingSpeakerPolicy()) } }
                    }
                }
            }
            .task { await store.loadSpeakerPeople() }
        }
    }

    private var threshold: Int { Int(((speakers.first?.threshold ?? 0.65) * 100).rounded()) }

    static func tint(_ index: Int) -> Color {
        [Color.blue, .orange, .purple, .teal, .pink, .green, .indigo, .brown][index % 8]
    }

    private func title(_ s: MediaLibraryRecord.Speaker) -> String {
        switch s.status {
        case .auto, .confirmed: s.name ?? s.key
        case .maybe: "可能是 \(s.name ?? "")？"
        case .anonymous: s.label ?? s.key
        }
    }

    private func detail(_ s: MediaLibraryRecord.Speaker) -> String {
        let label = switch s.status {
        case .auto: "自动"
        case .maybe: s.margin.map { $0 < 0.05 ? "和第二像的只差 \(Int(($0 * 100).rounded()))%" : "待确认" } ?? "待确认"
        case .confirmed: s.samplesSaved == false ? "你指定的 · 样本未保存" : "你指定的"
        case .anonymous: s.dismissed == true ? "保持匿名" : ["no_sample":"声纹样本不足", "no_candidate":"暂无匹配候选", "below_threshold":"匹配度未达阈值"][s.matchReason ?? ""] ?? "匿名"
        }
        return "\(label) · \(s.segments) 次发言"
    }

    private func row(_ s: MediaLibraryRecord.Speaker, tint: Color) -> some View {
        HStack(spacing: Space.s) {
            Text(String((s.status == .anonymous ? (s.label ?? s.key).last.map(String.init) : s.name?.first.map(String.init)) ?? "?"))
                .typography(.secondary, .semibold).foregroundStyle(.white)
                .frame(width: 26, height: 26).background(Circle().fill(tint))
            VStack(alignment: .leading, spacing: 2) {
                Text(title(s)).typography(.body, .semibold).lineLimit(1)
                Text(detail(s)).typography(.caption).foregroundStyle(.secondary).lineLimit(1)
                if s.status != .confirmed, let p = s.percent { ConfidenceBar(value: Double(p) / 100, status: s.status) }
            }
            Spacer(minLength: Space.s)
            if s.status != .confirmed, let p = s.percent {
                Text("\(p)%").typography(.secondary, digits: true).foregroundStyle(s.status == .anonymous ? .tertiary : .secondary)
            }
            if s.key != "unresolved" { action(s) }
        }
        .padding(.vertical, 6).padding(.horizontal, 6)
        .background(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous).fill(picking == s.key ? Palette.accent.opacity(0.08) : .clear))
        .popover(isPresented: Binding(get: { picking == s.key }, set: { if !$0 { picking = nil } }), arrowEdge: .leading) {
            SpeakerPicker(speaker: s, people: store.speakerPeople) { choice in
                picking = nil
                Task { await store.label(item, speaker: s.key, as: choice) }
            }
        }
    }

    @ViewBuilder private func action(_ s: MediaLibraryRecord.Speaker) -> some View {
        switch s.status {
        case .maybe:
            QuietButton(title: "不是") { picking = s.key }
            QuietButton(title: "是", emphasized: true) {
                guard let name = s.name else { return }
                let choice: SpeakerChoice = s.voiceId.map { .voice($0, name: name) } ?? .new(name)
                Task { await store.label(item, speaker: s.key, as: choice) }
            }
        case .anonymous: QuietButton(title: "这是谁？", emphasized: s.dismissed != true) { picking = s.key }
        default: QuietButton(title: "改") { picking = s.key }
        }
    }
}

private struct ConfidenceBar: View {
    let value: Double
    let status: MediaLibraryRecord.Speaker.Status
    var body: some View {
        Capsule().fill(Surface.fill.color).frame(width: 120, height: 3)
            .overlay(alignment: .leading) {
                Capsule().fill(status == .auto ? Palette.success : status == .maybe ? Palette.warning : Color.secondary.opacity(0.5))
                    .frame(width: 120 * max(0, min(1, value)), height: 3)
            }
            .padding(.top, 2)
            .accessibilityLabel("匹配度 \(Int((value * 100).rounded()))%")
    }
}

/// 「这是谁？」: an existing person, a new name, or keep anonymous.
private struct SpeakerPicker: View {
    let speaker: MediaLibraryRecord.Speaker
    let people: [SpeakerPerson]
    let choose: (SpeakerChoice) -> Void
    @ViewState private var query = ""
    @FocusState private var focused: Bool

    private var trimmed: String { query.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var matches: [SpeakerPerson] {
        trimmed.isEmpty ? Array(people.prefix(8)) : people.filter { $0.name.localizedCaseInsensitiveContains(trimmed) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            SearchField(prompt: "名字", text: $query, focus: $focused).onSubmit(submit)
                .padding(.bottom, 4)
            if let name = speaker.name, speaker.status != .confirmed, trimmed.isEmpty {
                option(name, detail: speaker.percent.map { "\($0)%" }, glyph: "person.wave.2") {
                    choose(speaker.voiceId.map { .voice($0, name: name) } ?? .new(name))
                }
            }
            ForEach(matches) { person in
                option(person.name, detail: nil, glyph: "person") { choose(.person(person)) }
            }
            if !trimmed.isEmpty, !people.contains(where: { $0.name == trimmed }) {
                option("新建「\(trimmed)」", detail: nil, glyph: "plus") { choose(.new(trimmed)) }
            }
            option("保持匿名", detail: nil, glyph: "minus", secondary: true) { choose(.anonymous) }
            Text("确认后，这个说话人最清楚的几段存为样本，只在本机。").typography(.caption).foregroundStyle(.tertiary)
                .padding(.top, 6).fixedSize(horizontal: false, vertical: true)
        }
        .padding(Space.s).frame(width: 260)
        .onAppear { focused = true }
    }

    private func submit() {
        if let exact = people.first(where: { $0.name == trimmed }) { choose(.person(exact)) }
        else if !trimmed.isEmpty { choose(.new(trimmed)) }
    }

    private func option(_ title: String, detail: String?, glyph: String, secondary: Bool = false, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: Space.xs) {
                Image(systemName: glyph).frame(width: 18).foregroundStyle(.secondary)
                Text(title).typography(.body).foregroundStyle(secondary ? .secondary : .primary).lineLimit(1)
                Spacer()
                if let detail { Text(detail).typography(.caption, digits: true).foregroundStyle(.secondary) }
            }
            .padding(.horizontal, 6).frame(height: 28).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}
