import AIProviders
import CandidLocalization
import SwiftUI

struct AIFunctionSettingsView: View {
    static let open = Notification.Name("Candid.openAIFunctionSettings")
    @ViewState private var model = WorkflowControlModel()
    @ViewState private var expanded: String?
    private var refreshOnAppear = true

    init(model: WorkflowControlModel = WorkflowControlModel(), refreshOnAppear: Bool = true) {
        _model = ViewState(wrappedValue: model); self.refreshOnAppear = refreshOnAppear
    }

    var body: some View {
        SettingsPage(title: L("AI 功能")) {
            if let error = model.error { InlineMessage(kind: .error, text: error) }
            if model.loading { ProgressView().controlSize(.small) }
            if model.features.isEmpty && !model.loading {
                QuietButton(title: L("重新连接时间线库")) { Task { await model.refresh() } }
            }
            ForEach(["core"], id: \.self) { mode in
                SettingsGroup(title: mode == "core" ? L("自动与固定流程") : L("AI 助手技能")) {
                    ForEach(model.features.filter { $0.mode == mode }) { feature in
                        featureRow(feature)
                        HairlineDivider()
                    }
                }
            }
            if model.featureSettings.items["screen-episodes"]?.enabled == true { ScreenProcessingView() }
            SkillSettingsView()
            AIControlPanels(workflows: model)
            if !model.remoteProviders.isEmpty { SettingsGroup(title: L("远程来源授权")) {
                ForEach(model.remoteProviders) { provider in
                    SettingRow(title: provider.name) {
                        ForEach(Self.sources, id: \.0) { source, name in
                            ToggleChip(title: name, isOn: Binding(get: { model.approvals[source + ":" + provider.providerID] == true },
                                                                set: { on in Task { await model.authorize(provider, source: source, allowed: on) } }))
                        }
                    }
                }
            }
            }
        }.task { if refreshOnAppear { await model.refresh() } }
    }

    private static let sources = [("import", L("导入资料")), ("mac_microphone", L("麦克风")), ("mac_system_audio", L("系统声音与麦克风")), ("mac_region", L("框选截图"))]

    private func featureRow(_ feature: WorkflowControlModel.Feature) -> some View {
        VStack(alignment: .leading, spacing: Space.s) {
            HStack(spacing: Space.s) {
                VStack(alignment: .leading, spacing: Space.xxs) {
                    Text(feature.title).typography(.headline)
                    Text(triggerLabel(feature)).typography(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                if feature.implemented {
                    if let job = model.jobs.last(where: { $0.kind == feature.job_kind }) { Tag(text: job.status) }
                    ChipButton(title: expanded == feature.id ? L("收起") : L("设置"), systemImage: "slider.horizontal.3") {
                        expanded = expanded == feature.id ? nil : feature.id
                    }
                    Toggle(feature.title, isOn: policyBinding(feature.id, \.enabled))
                        .labelsHidden().toggleStyle(.switch).controlSize(.small).disabled(model.saving)
                } else { Tag(text: feature.mode == "agent" ? L("仅手动") : L("尚未接入")) }
            }.padding(.vertical, Space.s)
            if expanded == feature.id, let policy = model.featureSettings.items[feature.id] {
                InlinePanel {
                    VStack(spacing: Space.s) {
                        SettingRow(title: L("使用角色")) { Text(ModelRoleInfo.info(feature.role)?.title ?? feature.role).typography(.body) }
                        if feature.id == "transcribe" {
                            SettingRow(title: L("保存后自动转写")) {
                                Toggle(L("自动转写"), isOn: Binding(get: { model.settings.automatic }, set: { on in model.settings.automatic = on; Task { await model.save() } }))
                                    .labelsHidden().toggleStyle(.switch).controlSize(.small)
                            }
                            SettingRow(title: L("转写方式")) {
                                CapsuleChoice(options: [("quick", L("快速转写")), ("conversation", L("多人分段"))], selection: Binding(get: { model.settings.mode }, set: { mode in model.settings.mode = mode; Task { await model.save() } }))
                            }
                        }
                        if ["title", "summary"].contains(feature.id) {
                            SettingRow(title: L("自动生成")) {
                                Toggle(L("自动生成"), isOn: Binding(get: { feature.id == "title" ? model.settings.titles : model.settings.summary }, set: { on in
                                    if feature.id == "title" { model.settings.titles = on } else { model.settings.summary = on }
                                    Task { await model.save() }
                                })).labelsHidden().toggleStyle(.switch).controlSize(.small)
                            }
                        }
                        SettingRow(title: L("读取来源")) {
                            ForEach(sourceOptions(feature), id: \.0) { source, name in
                                ToggleChip(title: name, isOn: Binding(get: { policy.sources.contains(source) || policy.sources.contains("*") }, set: { on in
                                    var next = policy
                                    if next.sources.contains("*") { next.sources = sourceOptions(feature).map(\.0) }
                                    next.sources.removeAll { $0 == source }; if on { next.sources.append(source) }
                                    Task { await model.setPolicy(feature.id, next) }
                                })).disabled(model.saving)
                            }
                        }
                        SettingRow(title: L("允许远程模型")) {
                            Toggle(L("允许远程模型"), isOn: policyBinding(feature.id, \.allow_remote))
                                .labelsHidden().toggleStyle(.switch).controlSize(.small).disabled(model.saving)
                        }
                        SettingRow(title: L("每日调用上限"), detail: L("按 UTC 日统计；设为 0 暂停模型调用")) {
                            StepperControl(text: String(policy.daily_calls), value: Binding(get: { policy.daily_calls }, set: { value in
                                var next = policy; next.daily_calls = value; Task { await model.setPolicy(feature.id, next) }
                            }), range: 0...10000).disabled(model.saving)
                        }
                        Text(L("模型在「模型」页分配；远程调用同时受来源授权限制。")).typography(.caption).foregroundStyle(.secondary)
                    }
                }
            }
        }
    }

    private func sourceOptions(_ feature: WorkflowControlModel.Feature) -> [(String, String)] {
        feature.id == "screen-episodes" ? model.sourceCatalog.filter(\.readable).map { ($0.id, $0.display_name) } : Self.sources
    }

    private func triggerLabel(_ feature: WorkflowControlModel.Feature) -> String {
        switch feature.id {
        case "transcribe": return model.settings.automatic ? feature.trigger : L("手动")
        case "title": return model.settings.titles ? feature.trigger : L("手动")
        case "summary": return model.settings.summary ? feature.trigger : L("手动")
        default: return feature.trigger
        }
    }

    private func policyBinding(_ id: String, _ key: WritableKeyPath<WorkflowControlModel.FeaturePolicy, Bool>) -> Binding<Bool> {
        Binding(get: { model.featureSettings.items[id]?[keyPath: key] ?? false }, set: { value in
            guard var policy = model.featureSettings.items[id] else { return }
            policy[keyPath: key] = value
            Task { await model.setPolicy(id, policy) }
        })
    }
}
