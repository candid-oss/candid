import CandidLocalization
import SwiftUI

struct AutomationRunsView: View {
    @ViewState private var model = AutomationModel()
    @ViewState private var selectedRun: String?
    var requestedRun: String?

    private var visibleRuns: [AutomationRun] {
        let recent = Array(model.runs.prefix(5))
        guard let requestedRun, !recent.contains(where: { $0.id == requestedRun }),
              let run = model.details[requestedRun] ?? model.runs.first(where: { $0.id == requestedRun }) else { return recent }
        return [run] + Array(recent.prefix(4))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Space.s) {
            if let error = model.error { InlineMessage(kind: .error, text: error) }
            VStack(alignment: .leading, spacing: 0) {
                if visibleRuns.isEmpty { Text(L("还没有技能运行")).typography(.secondary).foregroundStyle(.secondary).padding(.vertical, Space.s) }
                ForEach(visibleRuns) { run in
                    VStack(alignment: .leading, spacing: Space.s) {
                        HStack {
                            VStack(alignment: .leading, spacing: Space.xxs) {
                                Text(model.title(run.skill)).typography(.headline)
                                HStack(spacing: Space.xs) {
                                    Text(Date(timeIntervalSince1970: Double(run.created_ms) / 1000), style: .date)
                                    Text(Date(timeIntervalSince1970: Double(run.created_ms) / 1000), style: .time)
                                }.typography(.caption).foregroundStyle(.secondary)
                            }
                            Spacer()
                            Tag(text: run.status)
                            ChipButton(title: selectedRun == run.id ? L("收起") : L("详情"), systemImage: "list.bullet") {
                                selectedRun = selectedRun == run.id ? nil : run.id
                                if selectedRun != nil && run.unavailable != true { Task { await model.detail(run.id) } }
                            }
                            if ["queued", "running"].contains(run.state) { ChipButton(title: L("取消"), systemImage: "stop") { Task { await model.command("cancel", id: run.id) } } }
                            if ["failed", "cancelled"].contains(run.state) && run.unavailable != true { ChipButton(title: L("重试"), systemImage: "arrow.clockwise") { Task { await model.command("retry", id: run.id) } } }
                        }.padding(.vertical, Space.s)
                        if selectedRun == run.id { runDetails(run.unavailable == true ? run : model.details[run.id] ?? run) }
                    }
                    if run.id != visibleRuns.last?.id { HairlineDivider() }
                }
            }
        }
        // Refreshes when the core reports a run changing state, not on a timer.
        .refresh(on: TimelineService.shared.jobsVersion) { await model.refresh() }
        .task(id: requestedRun) {
            if let requestedRun {
                selectedRun = requestedRun
                await model.detail(requestedRun)
            }
        }
    }
    private func runDetails(_ run: AutomationRun) -> some View {
        InlinePanel {
            VStack(alignment: .leading, spacing: Space.s) {
                if let model = run.model {
                    Text("Pi · " + (model.provider_name.map { $0 + " · " } ?? "") + model.model_id).typography(.caption).foregroundStyle(.secondary)
                }
                if let problem = run.problem { InlineMessage(kind: .warning, text: problem) }
                if let result = run.displayResult, run.unavailable != true { Text(result).typography(.body).textSelection(.enabled) }
                ForEach(Array(run.steps.enumerated()), id: \.offset) { _, step in
                    Text(L("%@ · %@ 条来源", Self.toolName(step.tool), step.count)).typography(.secondary).foregroundStyle(.secondary)
                }
                if run.unavailable != true {
                    ForEach(Array(run.references.enumerated()), id: \.element.id) { index, ref in
                        ChipButton(title: L("来源 %@", index + 1), systemImage: "arrow.up.right") { NotificationCenter.default.post(name: .candidOpenRecord, object: ref.event_id) }
                    }
                }
            }.frame(maxWidth: .infinity, alignment: .leading)
        }
    }
    private static func toolName(_ name: String) -> String { ["search": L("搜索"), "timeline": L("查看时间线"), "overview": L("查看概览"), "get_event": L("查看原文"), "episodes": L("查看片段"), "episode": L("查看片段"), "save_draft": L("保存草稿"), "annotate": L("修改标注"), "memories": L("查看记忆"), "commitment": L("整理承诺")][name] ?? L("查看资料") }
}
