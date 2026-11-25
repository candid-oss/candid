import AppKit
import CandidLocalization
import SwiftUI

/// Runs fixed workflows on a selected item and shows its drafts in the AI workspace.
struct ManualWorkflowView: View {
    @Bindable var workflows: WorkflowControlModel
    var refreshOnAppear = true
    @ViewState private var submitting = false
    @ViewState private var queued = false

    var body: some View {
        VStack(alignment: .leading, spacing: Space.m) {
            SettingRow(title: L("资料")) {
                let dayChoice = workflows.selectedID.hasPrefix("workflow-day:")
                    ? [(workflows.selectedID, L("录音日回顾"))] : []
                let items = workflows.media.map { item in
                    let text = item.title.flatMap { $0.isEmpty ? nil : $0 } ?? item.filename
                    return (item.id, text.count > 40 ? String(text.prefix(40)) + "…" : text)
                }
                MenuChoice(options: [("", L("选择资料"))] + dayChoice + items, selection: $workflows.selectedID)
            }
            HStack(spacing: Space.xs) {
                if let item = workflows.media.first(where: { $0.id == workflows.selectedID }) {
                    if item.kind == "image" {
                        action(L("整理截图文字"), step: "screenshot")
                    } else {
                        action(L("标题"), step: "title")
                        action(L("摘要"), step: "summary")
                        action(L("决定与待办"), step: "decisions")
                        action(L("标签"), step: "tags")
                    }
                }
                Spacer(minLength: 0)
                action(L("回顾今日录音"), step: "daily")
            }
            if let error = workflows.error { InlineMessage(kind: .error, text: error) }
            if queued && workflows.error == nil { InlineMessage(kind: .note, text: L("已提交处理，可在「今天」查看进度。")) }
            ForEach(workflows.drafts.keys.sorted(), id: \.self) { key in
                if let draft = workflows.drafts[key] {
                    InlinePanel {
                        VStack(alignment: .leading, spacing: Space.xs) {
                            HStack {
                                Text(L("AI 草稿")).typography(.headline)
                                Spacer()
                                QuietButton(title: L("复制")) {
                                    NSPasteboard.general.clearContents()
                                    NSPasteboard.general.setString(draft.text, forType: .string)
                                }
                            }
                            Text(draft.text).typography(.body).textSelection(.enabled)
                        }
                    }
                }
            }
        }
        .task { if refreshOnAppear { await workflows.refresh() } }
        .task(id: workflows.selectedID) {
            guard refreshOnAppear else { return }
            workflows.drafts = [:]
            while !Task.isCancelled {
                await workflows.loadDrafts()
                try? await Task.sleep(for: .seconds(3))
            }
        }
    }

    private func action(_ title: String, step: String) -> some View {
        ChipButton(title: title) {
            submitting = true
            queued = false
            Task {
                await workflows.enqueue(step)
                queued = workflows.error == nil
                submitting = false
            }
        }.disabled(submitting || workflows.loading)
    }
}
