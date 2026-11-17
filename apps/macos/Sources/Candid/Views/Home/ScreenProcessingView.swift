import SwiftUI

struct ScreenProcessingView: View {
    @ViewState private var model = ScreenProcessingModel()
    @ViewState private var visibleCount = 3
    private var refreshOnAppear = true
    init(model: ScreenProcessingModel = ScreenProcessingModel(), refreshOnAppear: Bool = true) { _model = ViewState(wrappedValue: model); self.refreshOnAppear = refreshOnAppear }
    var body: some View {
        SettingsGroup(title: "屏幕片段") {
            HStack {
                Spacer()
                ChipButton(title: model.busy ? "正在整理" : "重新整理", systemImage: "arrow.clockwise") { Task { await model.command("rebuild") } }.disabled(model.busy)
            }
            if let error = model.error { InlineMessage(kind: .error, text: error) }
            if let report = model.report, report.raw_chars > 0 {
                Text("\(report.ended) 个已结束片段 · 重复与低权重文字减少约 \(max(0, 100 - report.input_chars * 100 / report.raw_chars))% 的模型输入")
                    .typography(.secondary).foregroundStyle(.secondary).padding(.vertical, Space.s)
                if report.bounded { InlineMessage(kind: .warning, text: "本次已到整理数量或文本上限，更早的原文仍可搜索") }
            }
            if model.episodes.isEmpty { Text("还没有已结束的屏幕片段").typography(.secondary).foregroundStyle(.secondary).padding(.vertical, Space.s) }
            ForEach(Array(model.episodes.prefix(visibleCount))) { episode in
                VStack(alignment: .leading, spacing: Space.s) {
                    HStack {
                        VStack(alignment: .leading, spacing: Space.xxs) {
                            Text(episode.title ?? "屏幕片段").typography(.headline).lineLimit(2)
                            Text(Date(timeIntervalSince1970: Double(episode.start_ms) / 1000), style: .time).typography(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Tag(text: episode.status == "edited" ? "你修改的" : episode.model == nil ? "规则分段" : "AI 草稿")
                        ChipButton(title: model.selected == episode.id ? "收起" : "查看", systemImage: "text.alignleft") { Task { await model.select(episode) } }
                    }.padding(.vertical, Space.s)
                    if let summary = episode.summary { Text(summary).typography(.body).textSelection(.enabled) }
                    if model.selected == episode.id {
                        InlinePanel {
                            VStack(alignment: .leading, spacing: Space.s) {
                                HStack {
                                    ChipButton(title: "生成摘要", systemImage: "sparkles") { Task { await model.command("run", id: episode.id) } }.disabled(model.busy)
                                    ChipButton(title: model.editing ? "取消修改" : "修改", systemImage: "pencil") { model.editing.toggle() }
                                }
                                if model.editing {
                                    QuietField(title: "标题", text: $model.title)
                                    TextEditor(text: $model.summary).typography(.body).frame(minHeight: 100).scrollContentBackground(.hidden)
                                    PrimaryButton(title: "保存") { Task { await model.command("edit", id: episode.id) } }
                                } else {
                                    ForEach(Array(model.lines.prefix(50).enumerated()), id: \.offset) { _, line in
                                        VStack(alignment: .leading, spacing: Space.xxs) {
                                            Text(line.text).typography(.body).textSelection(.enabled)
                                            HStack {
                                                if !line.is_new { Tag(text: "重复") }
                                                if line.weight < 0.5 {
                                                    Tag(text: "低权重")
                                                    ChipButton(title: "保留权重", systemImage: "pin") { Task { await model.command("important", family: line.family) } }
                                                }
                                                ChipButton(title: "原文", systemImage: "arrow.up.right") { NotificationCenter.default.post(name: .candidOpenRecord, object: line.event_id) }
                                            }
                                        }.padding(.vertical, Space.xxs)
                                    }
                                }
                            }
                        }
                    }
                }
                HairlineDivider()
            }
            if model.episodes.count > visibleCount {
                ChipButton(title: "再看 5 个片段", systemImage: "chevron.down") { visibleCount += 5 }
            }

        }
        .task {
            guard refreshOnAppear else { return }
            while !Task.isCancelled { await model.refresh(); try? await Task.sleep(for: .seconds(5)) }
        }
    }
}
