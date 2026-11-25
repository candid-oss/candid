import AIProviders
import SwiftUI
import CandidLocalization
import UniformTypeIdentifiers

struct SkillSettingsView: View {
    @ViewState private var model = AutomationModel()
    private var refreshOnAppear = true
    let openRole: (String) -> Void

    init(model: AutomationModel = AutomationModel(), expanded: String? = nil, refreshOnAppear: Bool = true, openRole: @escaping (String) -> Void = { _ in }) {
        _model = ViewState(wrappedValue: model)
        _expanded = ViewState(initialValue: expanded)
        self.refreshOnAppear = refreshOnAppear
        self.openRole = openRole
    }
    @ViewState private var expanded: String?
    @ViewState private var evaluation = false
    @ViewState private var timeDraft = ""
    @ViewState private var timezoneDraft = ""

    var body: some View {
        VStack(alignment: .leading, spacing: Space.m) {
            SettingsGroup(title: L("运行方式")) {
                if let error = model.error { InlineMessage(kind: .error, text: error) }
                if !model.passed && !model.catalog.isEmpty {
                    HStack {
                        Text(L("自动运行等待质量评估")).typography(.secondary).foregroundStyle(.secondary)
                        Spacer()
                        ChipButton(title: L("载入评估结果"), systemImage: "doc.badge.checkmark") { evaluation = true }
                    }.padding(.vertical, Space.s)
                }
                ForEach(model.catalog) { feature in
                    if let policy = model.config.skills[feature.id] {
                        row(feature, policy)
                        if feature.id != model.catalog.last?.id { HairlineDivider() }
                    }
                }
            }
        }
        .task(id: TimelineService.shared.channel?.socketPath) {
            if refreshOnAppear { await model.refresh(includeRuns: false) }
        }
        .fileImporter(isPresented: $evaluation, allowedContentTypes: [.json]) { result in
            if case .success(let url) = result {
                let scoped = url.startAccessingSecurityScopedResource(); defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                do { let data = try Data(contentsOf: url); Task { await model.loadEvaluation(data) } }
                catch { model.error = error.localizedDescription }
            }
        }
    }
    private func row(_ feature: WorkflowControlModel.Feature, _ policy: SkillPolicy) -> some View {
        VStack(alignment: .leading, spacing: Space.s) {
            SettingRow(title: L(feature.title), detail: policy.enabled && policy.automatic && policy.trigger != "manual" && !model.passed ? L("自动运行等待评估") : nil) {
                MenuChoice(options: Self.triggers, selection: Binding(
                    get: { policy.enabled ? (policy.automatic ? policy.trigger : "manual") : "off" },
                    set: { trigger in Task { await model.update(feature.id) {
                        $0.enabled = trigger != "off"
                        if trigger != "off" {
                            $0.automatic = trigger != "manual"
                            if $0.automatic { $0.trigger = trigger }
                        }
                    } } }
                ))
                .accessibilityLabel(L(feature.title))
                .disabled(model.busy || model.loading)
                IconChipButton(title: L("%@设置", L(feature.title)), symbol: expanded == feature.id ? "chevron.up" : "slider.horizontal.3") {
                    timeDraft = policy.time
                    timezoneDraft = policy.timezone
                    expanded = expanded == feature.id ? nil : feature.id
                }
                .accessibilityValue(expanded == feature.id ? L("已展开") : L("已收起"))
            }
            if expanded == feature.id {
                InlinePanel { policyEditor(feature.id, policy) }
            }
        }
    }
    private func policyEditor(_ id: String, _ p: SkillPolicy) -> some View {
        VStack(alignment: .leading, spacing: Space.xs) {
            if p.automatic && p.trigger == "daily" {
                HStack(alignment: .bottom) {
                    QuietField(title: L("时间"), text: $timeDraft, placeholder: "21:00")
                    QuietField(title: L("时区"), text: $timezoneDraft, placeholder: TimeZone.current.identifier)
                    ChipButton(title: L("保存时间"), systemImage: "checkmark") { Task { await model.update(id) { $0.time = timeDraft; $0.timezone = timezoneDraft } } }
                }
            }
            SettingRow(title: L("使用模型")) {
                MenuChoice(options: ["agent_chat", "summary", "episode", "title"].map { ($0, ModelRoleInfo.info($0)?.title ?? $0) },
                           selection: binding(id, \.role, fallback: p.role))
                ChipButton(title: L("配置"), systemImage: "cpu") { openRole(p.role) }
            }
            FoldRow(title: L("读取来源"), detail: p.sources.contains("*") ? L("全部已授权") : L("已选 %@ 项", p.sources.count)) {
                SwitchRow(title: L("全部已授权"), isOn: Binding(get: { p.sources.contains("*") }, set: { all in
                    Task { await model.update(id) { $0.sources = all ? ["*"] : model.sources.filter(\.readable).map(\.id) } }
                }))
                ForEach(model.sources.filter(\.readable)) { source in
                    SwitchRow(title: source.display_name, isOn: Binding(get: { p.sources.contains("*") || p.sources.contains(source.id) }, set: { on in
                        Task { await model.update(id) { policy in
                            if policy.sources.contains("*") { policy.sources = model.sources.filter(\.readable).map(\.id) }
                            policy.sources.removeAll { $0 == source.id }
                            if on { policy.sources.append(source.id) }
                        } }
                    }))
                }
            }
            SwitchRow(title: L("允许远程模型"), isOn: binding(id, \.allow_remote, fallback: p.allow_remote))
            FoldRow(title: L("调用上限")) {
                SettingRow(title: L("每天最多运行")) { StepperControl(text: String(p.daily_runs), value: binding(id, \.daily_runs, fallback: p.daily_runs), range: 0...100) }
                SettingRow(title: L("每次模型调用")) { StepperControl(text: String(p.max_model_calls), value: binding(id, \.max_model_calls, fallback: p.max_model_calls), range: 1...200) }
                SettingRow(title: L("每次工具调用")) { StepperControl(text: String(p.max_tool_calls), value: binding(id, \.max_tool_calls, fallback: p.max_tool_calls), range: 1...500) }
                SettingRow(title: L("每次输出上限")) { StepperControl(text: String(p.max_output_tokens), value: binding(id, \.max_output_tokens, fallback: p.max_output_tokens), range: 256...400000, step: 256) }
            }
            SettingRow(title: L("完成后通知")) {
                Toggle(L("完成后通知"), isOn: Binding(get: { p.notify }, set: { on in Task {
                    let allowed = on ? await AutomationNotifier.shared.requestAccess() : true
                    if allowed { await model.update(id) { $0.notify = on } }
                    else { model.error = L("通知未获授权，可在系统设置中为 Candid 开启") }
                } })).labelsHidden().toggleStyle(.switch).controlSize(.small)
            }
        }.disabled(model.busy || model.loading)
    }
    private func binding<T>(_ id: String, _ key: WritableKeyPath<SkillPolicy, T>, fallback: T) -> Binding<T> {
        Binding(get: { model.config.skills[id]?[keyPath: key] ?? fallback }, set: { value in Task { await model.update(id) { $0[keyPath: key] = value } } })
    }
    private static let triggers = [
        ("off", L("关闭")), ("manual", L("仅手动")), ("daily", L("每天定时")),
        ("recording_sealed", L("录音保存后")), ("episode_ended", L("片段结束后"))
    ]
}
