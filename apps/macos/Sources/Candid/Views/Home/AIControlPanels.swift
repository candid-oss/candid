import AppKit
import CandidLocalization
import SwiftUI

/// Manual processing and recent jobs; feature switches live in Settings → AI 功能.
struct AIControlPanels: View {
    @Bindable var workflows: WorkflowControlModel

    var body: some View {
        workflowControls
    }

    // MARK: Panels

    /// Fixed processing in two columns: settings and flows on the left, manual runs and jobs on the right.
    var workflowControls: some View {
        HStack(alignment: .top, spacing: Space.l + Space.xs) {
            VStack(spacing: Space.m) { manualPanel }
                .frame(maxWidth: .infinity)
            VStack(spacing: Space.m) { jobsPanel }
                .frame(maxWidth: .infinity)
        }
    }

    private func panel<Content: View>(_ title: String, @ViewBuilder accessory: () -> some View = { EmptyView() }, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack { Text(title).typography(.headline); Spacer(); accessory() }
                .padding(.bottom, Space.xxs)
            content()
        }
        .padding(.horizontal, Space.l - 4).padding(.vertical, Space.m)
        .frame(maxWidth: .infinity, alignment: .leading)
        .panelSurface(radius: Radius.large)
    }

    private var manualPanel: some View {
        panel(L("手动生成")) {
            SettingRow(title: L("对象")) {
                MenuChoice(options: [("", L("选择资料"))] + (workflows.selectedID.hasPrefix("workflow-day:") ? [(workflows.selectedID, "今日录音回顾")] : [])
                           + workflows.media.map { ($0.id, $0.title?.isEmpty == false ? $0.title! : $0.filename) },
                           selection: $workflows.selectedID)
            }
            .onChange(of: workflows.selectedID) { _, _ in Task { await workflows.refresh() } }
            HStack(spacing: Space.xs) {
                if let item = workflows.media.first(where: { $0.id == workflows.selectedID }) {
                    if item.kind == "image" {
                        SecondaryButton(title: L("整理截图文字")) { Task { await workflows.enqueue("screenshot") } }
                    } else {
                        SecondaryButton(title: L("标题")) { Task { await workflows.enqueue("title") } }
                        SecondaryButton(title: L("摘要")) { Task { await workflows.enqueue("summary") } }
                        SecondaryButton(title: L("决定与待办")) { Task { await workflows.enqueue("decisions") } }
                        SecondaryButton(title: L("标签")) { Task { await workflows.enqueue("tags") } }
                    }
                } else {
                    Text(L("先选一条资料")).typography(.secondary).foregroundStyle(.secondary)
                }
                Spacer(minLength: Space.xs)
                PrimaryButton(title: L("回顾今天")) { Task { await workflows.enqueue("daily") } }
            }
            .padding(.vertical, Space.xs)
            ForEach(workflows.drafts.keys.sorted(), id: \.self) { key in
                if let draft = workflows.drafts[key] {
                    VStack(alignment: .leading, spacing: Space.xxs) {
                        HStack {
                            Text(L("AI 草稿")).typography(.caption, .semibold).foregroundStyle(Palette.ai)
                            Spacer()
                            QuietButton(title: L("复制")) { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(draft.text, forType: .string) }
                        }
                        Text(draft.text).typography(.body).textSelection(.enabled)
                    }
                    .padding(Space.s)
                    .background(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous).fill(Surface.wash.color))
                    .padding(.top, Space.xxs)
                }
            }
        }
    }

    private var jobsPanel: some View {
        panel(L("最近任务"), accessory: {
            QuietButton(title: L("刷新")) { Task { await workflows.refresh() } }.disabled(workflows.loading)
        }) {
            let jobs = Array(workflows.jobs.suffix(20).reversed())
            if jobs.isEmpty { Text(L("还没有处理任务")).typography(.secondary).foregroundStyle(.secondary).padding(.vertical, Space.xs) }
            ForEach(Array(jobs.enumerated()), id: \.element.id) { index, job in
                if index > 0 { HairlineDivider() }
                HStack(spacing: Space.s) {
                    VStack(alignment: .leading, spacing: 1) {
                        Text(job.title).typography(.body)
                        Text(workflows.sourceName(job)).typography(.secondary).foregroundStyle(.secondary).lineLimit(1)
                    }
                    Spacer(minLength: Space.xs)
                    Tag(text: job.status, tint: job.state == "done" ? Palette.success : job.state == "failed" ? Palette.warning : ["queued", "running"].contains(job.state) ? Palette.accent : nil)
                    if ["queued", "running"].contains(job.state) {
                        QuietButton(title: L("取消")) { Task { await workflows.command("cancel", job: job) } }.help(L("取消后不再保存返回结果。正在运行的模型可能继续处理。"))
                    }
                    if ["failed", "cancelled"].contains(job.state) {
                        QuietButton(title: L("重试"), emphasized: true) { Task { await workflows.command("restart", job: job) } }
                    }
                }
                .frame(minHeight: 44)
            }
            if let error = workflows.error { InlineMessage(kind: .error, text: error).padding(.top, Space.xs) }
        }
    }
}
