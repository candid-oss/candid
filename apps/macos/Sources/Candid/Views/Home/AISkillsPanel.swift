import SwiftUI
import CandidLocalization

/// The AI page's skill list (opened from 「技能」 in the header): what Pi can run, what triggers it, whether the hook is
/// on, and 运行; below, the fixed flows inside the core. Per-feature settings come later in 设置 → AI 功能 (§4.2).
struct AISkillsPanel: View {
    @Bindable var model: AIPageModel
    let close: () -> Void

    var body: some View {
        InlinePanel {
            VStack(alignment: .leading, spacing: Space.s) {
                HStack {
                    Text(L("AI 助手技能")).typography(.headline)
                    Text(L("由挂接的 Pi 运行，结果出现在对话里")).typography(.caption).foregroundStyle(.tertiary)
                    Spacer()
                    ChipButton(title: L("收起"), systemImage: "chevron.up", action: close)
                }
                if model.skills.isEmpty {
                    InlineMessage(kind: .warning, text: L("没有找到技能：App 里的 Resources/Pi/skills 为空（开发时读 agents/skills）"))
                }
                ForEach(Array(model.skills.enumerated()), id: \.element.id) { index, skill in
                    if index > 0 { HairlineDivider() }
                    skillRow(skill)
                }
                if !model.workflows.definitions.isEmpty {
                    Text(L("核心内流程")).typography(.headline).padding(.top, Space.xs)
                    ForEach(Array(model.workflows.definitions.enumerated()), id: \.element.id) { index, flow in
                        if index > 0 { HairlineDivider() }
                        HStack(spacing: Space.xs) {
                            Text(flow.title).typography(.body).foregroundStyle(flow.implemented ? .primary : .secondary)
                            Spacer(minLength: Space.xs)
                            Tag(text: Self.flowTrigger(flow.id, automatic: model.workflows.settings.automatic))
                            Tag(text: flow.implemented ? L("可用") : L("尚未接入"), tint: flow.implemented ? Palette.success : nil)
                        }
                        .frame(minHeight: 30)
                    }
                    Text(L("功能开关在 设置 → AI 功能。")).typography(.caption).foregroundStyle(.tertiary)
                }
            }
        }
    }

    private func skillRow(_ skill: AISkill) -> some View {
        let run = AIRun.skill(skill.id)
        return VStack(alignment: .leading, spacing: Space.xxs) {
            HStack(alignment: .firstTextBaseline, spacing: Space.xs) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(skill.title).typography(.body, .semibold)
                    Text(skill.summary).typography(.secondary).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    if let input = skill.input { Text(L("需要：") + input).typography(.caption).foregroundStyle(.tertiary) }
                }
                Spacer(minLength: Space.xs)
                Tag(text: skill.triggerLabel)
                Tag(text: model.skillStatus(skill), tint: model.canRunSkill(skill.id) ? Palette.success : nil)
                RunButton(title: L("运行"), systemImage: "play.fill", runningTitle: L("正在运行…"), running: model.isRunning(run),
                          start: { Task { await model.runSkill(skill) } }, cancel: { model.cancelRun() })
                    .disabled(!model.canRunSkill(skill.id) || (model.turnRunning && !model.isRunning(run)))
            }
            if let problem = model.problems["run:" + run.key] { InlineMessage(kind: .error, text: problem) }
        }
    }

    static func flowTrigger(_ id: String, automatic: Bool) -> String {
        switch id {
        case "recording": automatic ? L("录音保存后 · 已开") : L("录音保存后 · 未开")
        case "conversation": L("随录音转写")
        default: L("手动")
        }
    }
}
