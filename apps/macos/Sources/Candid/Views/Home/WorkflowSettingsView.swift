import AIProviders
import CandidLocalization
import DesktopCore
import SwiftUI

/// The workflow catalog supplies the policy IDs. Related tasks are shown together with their model and trigger.
struct WorkflowSettingsView: View {
    let model: WorkflowControlModel
    let models: ModelSettingsModel
    var recognition: DesktopRecognitionSession?
    let openRole: (String) -> Void
    @ViewState private var expanded: String?

    private static let recording = ["transcribe", "title", "summary"]
    private static let screen = ["screen-excerpt", "screen-episodes"]
    private static let organization = ["item-extract", "people-judge"]

    var body: some View {
        let recording = features(Self.recording)
        let screen = features(Self.screen)
        let organization = features(Self.organization)
        if !recording.isEmpty {
            SettingsGroup(title: L("录音处理")) { featureRows(recording) }
        }
        if recognition != nil || !screen.isEmpty {
            SettingsGroup(title: L("截图与屏幕")) {
                if let recognition {
                    ForEach(DesktopRecognitionKind.allCases, id: \.self) { kind in
                        recognitionRow(kind, session: recognition)
                        if kind != DesktopRecognitionKind.allCases.last || !screen.isEmpty {
                            HairlineDivider()
                        }
                    }
                }
                featureRows(screen)
            }
        }
        if !organization.isEmpty {
            SettingsGroup(title: L("待办与人物")) { featureRows(organization) }
        }
        SettingsGroup(title: L("AI 对话")) {
            if let info = ModelRoleInfo.info("agent_chat") {
                SettingRow(title: info.title) { modelButton(info.id) }
            }
        }
        FoldRow(title: L("按需处理与其他模型")) {
            let assigned = Self.recording + Self.screen + Self.organization
            featureRows(model.features.filter { $0.implemented && $0.mode == "core" && !assigned.contains($0.id) })
            ForEach(ModelRoleInfo.catalog.filter { !$0.inUse || (recognition == nil && $0.id == "ocr") }) { info in
                HairlineDivider()
                SettingRow(title: info.title) { modelButton(info.id) }
            }
        }
    }

    private func features(_ ids: [String]) -> [WorkflowControlModel.Feature] {
        ids.compactMap { id in model.features.first { $0.id == id && $0.implemented } }
    }

    private func featureRows(_ features: [WorkflowControlModel.Feature]) -> some View {
        ForEach(features) { feature in
            if feature.id != features.first?.id { HairlineDivider() }
            if let policy = model.featureSettings.items[feature.id] {
                featureRow(feature, policy: policy)
            }
        }
    }

    private func featureRow(_ feature: WorkflowControlModel.Feature, policy: WorkflowControlModel.FeaturePolicy) -> some View {
        let id = feature.id
        let hasTrigger = WorkflowControlModel.Settings.automaticKey(for: id) != nil
        let role = id == "transcribe" ? (model.settings.mode == "quick" ? "asr" : "diarization") : feature.role
        let detail = model.processingMode(id) == .automatic ? L(feature.trigger) : nil
        return VStack(alignment: .leading, spacing: 0) {
            SettingRow(title: L(feature.title), detail: detail) {
                modelButton(role)
                if hasTrigger {
                    MenuChoice(options: WorkflowControlModel.ProcessingMode.allCases.map { ($0, $0.title) },
                               selection: processingModeBinding(id))
                    .accessibilityLabel(L(feature.title))
                    .disabled(model.saving || model.loading)
                } else {
                    Toggle(L(feature.title), isOn: policyBinding(id, \.enabled, fallback: policy.enabled))
                        .labelsHidden().toggleStyle(.switch).controlSize(.small)
                        .disabled(model.saving || model.loading)
                }
                optionsButton(id, title: L(feature.title))
            }
            if expanded == id {
                InlinePanel {
                    featureOptions(feature, policy: policy)
                        .disabled(model.saving || model.loading)
                }
                .padding(.bottom, Space.s)
            }
        }
    }

    private func recognitionRow(_ kind: DesktopRecognitionKind, session: DesktopRecognitionSession) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            SettingRow(title: kind.title) {
                modelButton(kind == .vision ? "vision" : "ocr")
                MenuChoice(options: DesktopRecognitionMode.allCases.map { ($0, $0.title) }, selection: Binding(
                    get: { session.policy(kind).mode },
                    set: { mode in
                        var policy = session.policy(kind)
                        policy.mode = mode
                        session.setPolicy(policy, for: kind)
                    }
                ))
                .accessibilityLabel(L("处理时机"))
                optionsButton(kind.rawValue, title: kind.title)
            }
            if expanded == kind.rawValue {
                InlinePanel { DesktopRecognitionOptions(session: session, kind: kind) }
                    .padding(.bottom, Space.s)
            }
        }
    }

    private func optionsButton(_ id: String, title: String) -> some View {
        IconChipButton(title: L("%@设置", title), symbol: expanded == id ? "chevron.up" : "slider.horizontal.3") {
            expanded = expanded == id ? nil : id
        }
        .accessibilityValue(expanded == id ? L("已展开") : L("已收起"))
    }

    private func modelButton(_ role: String) -> some View {
        let title = ModelRoleInfo.info(role)?.title ?? role
        let name = models.assignedName(for: role) ?? L("选择模型")
        return ChipButton(title: name, systemImage: "cpu") { openRole(role) }
            .lineLimit(1)
            .frame(maxWidth: 188, alignment: .trailing)
            .accessibilityLabel(L("为%@选择模型", title))
            .accessibilityValue(name)
    }

    private func featureOptions(_ feature: WorkflowControlModel.Feature, policy: WorkflowControlModel.FeaturePolicy) -> some View {
        let id = feature.id
        let sources = sourceOptions(id)
        return VStack(alignment: .leading, spacing: 0) {
            if id == "transcribe" {
                SettingRow(title: L("自动转写方式")) {
                    MenuChoice(options: [("quick", L("快速转写")), ("conversation", L("多人分段"))],
                               selection: Binding(get: { model.settings.mode }, set: { mode in Task { await model.setTranscriptionMode(mode) } }))
                }
                if model.settings.mode == "quick" {
                    SettingRow(title: L("多人分段")) { modelButton("diarization") }
                } else {
                    SettingRow(title: L("快速转写")) { modelButton("asr") }
                }
                HairlineDivider()
            }
            FoldRow(title: L("读取来源"), detail: policy.sources.contains("*") ? L("全部已授权") : L("已选 %@ 项", policy.sources.count)) {
                if policy.sources.contains("*") || supportsWildcard(id) {
                    SwitchRow(title: L("全部已授权"), isOn: Binding(get: { policy.sources.contains("*") }, set: { all in
                        updateSources(id, all ? ["*"] : sources.map(\.0))
                    }))
                }
                ForEach(sources, id: \.0) { source, label in
                    SwitchRow(title: label, isOn: Binding(get: { policy.sources.contains("*") || policy.sources.contains(source) }, set: { on in
                        var next = policy.sources.contains("*") ? sources.map(\.0) : policy.sources
                        next.removeAll { $0 == source }
                        if on { next.append(source) }
                        updateSources(id, next)
                    }))
                }
            }
            HairlineDivider()
            SwitchRow(title: L("允许远程模型"), isOn: policyBinding(id, \.allow_remote, fallback: policy.allow_remote))
            HairlineDivider()
            SettingRow(title: L("每日调用上限"), detail: L("按 UTC 日统计；设为 0 暂停模型调用")) {
                StepperControl(text: String(policy.daily_calls), value: policyBinding(id, \.daily_calls, fallback: policy.daily_calls), range: 0...10000)
            }
        }
    }

    private func supportsWildcard(_ id: String) -> Bool { (Self.screen + Self.organization).contains(id) }

    private func sourceOptions(_ id: String) -> [(String, String)] {
        if supportsWildcard(id) { return model.sourceCatalog.filter(\.readable).map { ($0.id, $0.display_name) } }
        return AIDataPermissionsView.mediaSources
    }

    private func updateSources(_ id: String, _ sources: [String]) {
        guard var policy = model.featureSettings.items[id] else { return }
        policy.sources = sources
        Task { await model.setPolicy(id, policy) }
    }

    private func processingModeBinding(_ id: String) -> Binding<WorkflowControlModel.ProcessingMode> {
        Binding(get: { model.processingMode(id) }, set: { mode in
            Task { await model.setProcessingMode(id, mode) }
        })
    }

    private func policyBinding<Value>(_ id: String, _ key: WritableKeyPath<WorkflowControlModel.FeaturePolicy, Value>, fallback: Value) -> Binding<Value> {
        Binding(get: { model.featureSettings.items[id]?[keyPath: key] ?? fallback }, set: { value in
            guard var policy = model.featureSettings.items[id] else { return }
            policy[keyPath: key] = value
            Task { await model.setPolicy(id, policy) }
        })
    }
}
