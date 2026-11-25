import SwiftUI
import CandidLocalization

struct ScreenProcessingView: View {
    @ViewState private var model = ScreenProcessingModel()
    @ViewState private var visibleCount = 3
    private let refreshOnAppear: Bool
    private let showsHeading: Bool
    init(model: ScreenProcessingModel = ScreenProcessingModel(), refreshOnAppear: Bool = true, showsHeading: Bool = true) {
        _model = ViewState(wrappedValue: model)
        self.refreshOnAppear = refreshOnAppear
        self.showsHeading = showsHeading
    }
    var body: some View {
        VStack(alignment: .leading, spacing: Space.xs) {
            HStack {
                if showsHeading { Text(L("屏幕片段")).typography(.headline) }
                Spacer()
                ChipButton(title: model.busy ? L("正在整理") : L("重新整理"), systemImage: "arrow.clockwise") { Task { await model.command("rebuild") } }.disabled(model.busy)
            }
            if let error = model.error { InlineMessage(kind: .error, text: error) }
            if let report = model.report, report.raw_chars > 0 {
                Text(L("%@ 个已结束片段 · 重复与低权重文字减少约 %@%% 的模型输入", report.ended, max(0, 100 - report.input_chars * 100 / report.raw_chars)))
                    .typography(.secondary).foregroundStyle(.secondary).padding(.vertical, Space.s)
                if report.bounded { InlineMessage(kind: .warning, text: L("本次已到整理数量或文本上限，更早的原文仍可搜索")) }
            }
            if model.episodes.isEmpty { Text(L("还没有已结束的屏幕片段")).typography(.secondary).foregroundStyle(.secondary).padding(.vertical, Space.s) }
            ForEach(Array(model.episodes.prefix(visibleCount))) { episode in
                VStack(alignment: .leading, spacing: Space.s) {
                    HStack {
                        VStack(alignment: .leading, spacing: Space.xxs) {
                            Text(episode.title ?? L("屏幕片段")).typography(.headline).lineLimit(2)
                            Text(Date(timeIntervalSince1970: Double(episode.start_ms) / 1000), style: .time).typography(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Tag(text: episode.status == "edited" ? L("你修改的") : episode.model == nil ? L("规则分段") : L("AI 草稿"))
                        ChipButton(title: model.selected == episode.id ? L("收起") : L("查看"), systemImage: "text.alignleft") { Task { await model.select(episode) } }
                    }.padding(.vertical, Space.s)
                    if let summary = episode.summary { Text(summary).typography(.body).textSelection(.enabled) }
                    if model.selected == episode.id {
                        InlinePanel {
                            VStack(alignment: .leading, spacing: Space.s) {
                                HStack {
                                    ChipButton(title: L("生成摘要"), systemImage: "sparkles") { Task { await model.command("run", id: episode.id) } }.disabled(model.busy)
                                    ChipButton(title: model.editing ? L("取消修改") : L("修改"), systemImage: "pencil") { model.editing.toggle() }
                                }
                                if model.editing {
                                    QuietField(title: L("标题"), text: $model.title)
                                    TextEditor(text: $model.summary).typography(.body).frame(minHeight: 100).scrollContentBackground(.hidden)
                                    PrimaryButton(title: L("保存")) { Task { await model.command("edit", id: episode.id) } }
                                } else {
                                    ForEach(Array(model.lines.prefix(50).enumerated()), id: \.offset) { _, line in
                                        VStack(alignment: .leading, spacing: Space.xxs) {
                                            Text(line.text).typography(.body).textSelection(.enabled)
                                            HStack {
                                                if !line.is_new { Tag(text: L("重复")) }
                                                if line.weight < 0.5 {
                                                    Tag(text: L("低权重"))
                                                    ChipButton(title: L("保留权重"), systemImage: "pin") { Task { await model.command("important", family: line.family) } }
                                                }
                                                ChipButton(title: L("原文"), systemImage: "arrow.up.right") { NotificationCenter.default.post(name: .candidOpenRecord, object: line.event_id) }
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
                ChipButton(title: L("再看 5 个片段"), systemImage: "chevron.down") { visibleCount += 5 }
            }

        }
        .task {
            guard refreshOnAppear else { return }
            while !Task.isCancelled { await model.refresh(); try? await Task.sleep(for: .seconds(5)) }
        }
    }
}
