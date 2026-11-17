import SwiftUI
import UniformTypeIdentifiers

struct SkillSettingsView: View {
    @ViewState private var model = AutomationModel()
    private var refreshOnAppear = true
    init(model: AutomationModel = AutomationModel(), expanded: String? = nil, refreshOnAppear: Bool = true) {
        _model = ViewState(wrappedValue: model); _expanded = ViewState(initialValue: expanded); self.refreshOnAppear = refreshOnAppear
    }
    @ViewState private var expanded: String?
    @ViewState private var selectedRun: String?
    @ViewState private var evaluation = false
    @ViewState private var timeDraft = ""
    @ViewState private var timezoneDraft = ""

    var body: some View {
        VStack(alignment: .leading, spacing: Space.m) {
            SettingsGroup(title: "AI 助手技能") {
                if let error = model.error { InlineMessage(kind: .error, text: error) }
                if !model.passed {
                    HStack {
                        Text("自动运行等待质量评估").typography(.secondary).foregroundStyle(.secondary)
                        Spacer()
                        ChipButton(title: "载入评估结果", systemImage: "doc.badge.checkmark") { evaluation = true }
                    }.padding(.vertical, Space.s)
                }
                ForEach(model.catalog) { feature in
                    if let policy = model.config.skills[feature.id] {
                        row(feature, policy)
                        HairlineDivider()
                    }
                }
            }
            SettingsGroup(title: "最近运行") {
                if model.runs.isEmpty { Text("还没有技能运行").typography(.secondary).foregroundStyle(.secondary).padding(.vertical, Space.s) }
                ForEach(model.runs) { run in
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
                            ChipButton(title: selectedRun == run.id ? "收起" : "详情", systemImage: "list.bullet") { selectedRun = selectedRun == run.id ? nil : run.id; if selectedRun != nil { Task { await model.detail(run.id) } } }
                            if ["queued", "running"].contains(run.state) { ChipButton(title: "取消", systemImage: "stop") { Task { await model.command("cancel", id: run.id) } } }
                            if ["failed", "cancelled"].contains(run.state) && run.unavailable != true { ChipButton(title: "重试", systemImage: "arrow.clockwise") { Task { await model.command("retry", id: run.id) } } }
                        }.padding(.vertical, Space.s)
                        if selectedRun == run.id { runDetails(model.details[run.id] ?? run) }
                    }
                    HairlineDivider()
                }
            }
        }
        .task {
            guard refreshOnAppear else { return }
            while !Task.isCancelled { await model.refresh(); try? await Task.sleep(for: .seconds(5)) }
        }
        .onAppear { selectedRun = UserDefaults.standard.string(forKey: "automation.selectedRun") }
        .onReceive(NotificationCenter.default.publisher(for: .candidAutomationRun)) { note in selectedRun = note.object as? String }
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
            HStack {
                VStack(alignment: .leading, spacing: Space.xxs) {
                    Text(feature.title).typography(.headline)
                    Text(policy.automatic ? (model.passed ? triggerLabel(policy) : "自动运行等待评估") : "手动运行").typography(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                ChipButton(title: "运行", systemImage: "play.fill") { NotificationCenter.default.post(name: .candidRunSkill, object: feature.id) }
                ChipButton(title: expanded == feature.id ? "收起" : "设置", systemImage: "slider.horizontal.3") { timeDraft = policy.time; timezoneDraft = policy.timezone; expanded = expanded == feature.id ? nil : feature.id }
                Toggle(feature.title, isOn: binding(feature.id, \.enabled)).labelsHidden().toggleStyle(.switch).controlSize(.small).disabled(model.busy)
            }.padding(.vertical, Space.s)
            if expanded == feature.id {
                InlinePanel { policyEditor(feature.id, policy) }
            }
        }
    }
    private func policyEditor(_ id: String, _ p: SkillPolicy) -> some View {
        VStack(spacing: Space.s) {
            SettingRow(title: "自动运行") {
                Toggle("自动运行", isOn: binding(id, \.automatic)).labelsHidden().toggleStyle(.switch).controlSize(.small).disabled(model.busy)
            }
            SettingRow(title: "触发方式") {
                MenuChoice(options: [("manual", "仅手动"), ("daily", "每天定时"), ("recording_sealed", "录音保存后"), ("episode_ended", "片段结束后")], selection: binding(id, \.trigger))
            }
            if p.trigger == "daily" {
                HStack {
                    QuietField(title: "时间", text: $timeDraft, placeholder: "21:00")
                    QuietField(title: "时区", text: $timezoneDraft, placeholder: TimeZone.current.identifier)
                    ChipButton(title: "保存时间", systemImage: "checkmark") { Task { await model.update(id) { $0.time = timeDraft; $0.timezone = timezoneDraft } } }
                }
            }
            SettingRow(title: "模型角色") { MenuChoice(options: [("agent_chat", "对话 Agent"), ("summary", "摘要"), ("episode", "片段整理"), ("title", "标题")], selection: binding(id, \.role)) }
            SettingRow(title: "读取来源") {
                ToggleChip(title: "全部已授权", isOn: Binding(get: { p.sources.contains("*") }, set: { on in Task { await model.update(id) { $0.sources = on ? ["*"] : [] } } }))
            }
            ForEach(model.sources.filter(\.readable)) { source in
                SettingRow(title: source.display_name) {
                    ToggleChip(title: "读取", isOn: Binding(get: { p.sources.contains("*") || p.sources.contains(source.id) }, set: { on in
                        Task { await model.update(id) { policy in
                            if policy.sources.contains("*") { policy.sources = model.sources.filter(\.readable).map(\.id) }
                            policy.sources.removeAll { $0 == source.id }; if on { policy.sources.append(source.id) }
                        } }
                    }))
                    ToggleChip(title: "可发往远程", isOn: Binding(get: { model.config.remote_sources.contains(source.id) }, set: { on in
                        var config = model.config; config.remote_sources.removeAll { $0 == source.id }; if on { config.remote_sources.append(source.id) }
                        Task { await model.save(config) }
                    }))
                }
            }
            SettingRow(title: "允许远程模型") { Toggle("允许远程模型", isOn: binding(id, \.allow_remote)).labelsHidden().toggleStyle(.switch).controlSize(.small) }
            SettingRow(title: "每天最多运行") { StepperControl(text: String(p.daily_runs), value: binding(id, \.daily_runs), range: 0...100) }
            SettingRow(title: "每次模型调用") { StepperControl(text: String(p.max_model_calls), value: binding(id, \.max_model_calls), range: 1...200) }
            SettingRow(title: "每次工具调用") { StepperControl(text: String(p.max_tool_calls), value: binding(id, \.max_tool_calls), range: 1...500) }
            SettingRow(title: "每次输出上限") { StepperControl(text: String(p.max_output_tokens), value: binding(id, \.max_output_tokens), range: 256...400000, step: 256) }
            SettingRow(title: "完成后通知") {
                Toggle("完成后通知", isOn: Binding(get: { p.notify }, set: { on in Task {
                    let allowed = on ? await AutomationNotifier.shared.requestAccess() : true
                    if allowed { await model.update(id) { $0.notify = on } }
                    else { model.error = "通知未获授权，可在系统设置中为 Candid 开启" }
                } })).labelsHidden().toggleStyle(.switch).controlSize(.small)
            }
        }.disabled(model.busy)
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
                    Text("\(Self.toolName(step.tool)) · \(step.count) 条来源").typography(.secondary).foregroundStyle(.secondary)
                }
                if run.unavailable != true {
                    ForEach(Array(run.references.enumerated()), id: \.element.id) { index, ref in
                        ChipButton(title: "来源 \(index + 1)", systemImage: "arrow.up.right") { NotificationCenter.default.post(name: .candidOpenRecord, object: ref.event_id) }
                    }
                }
            }.frame(maxWidth: .infinity, alignment: .leading)
        }
    }
    private func binding<T>(_ id: String, _ key: WritableKeyPath<SkillPolicy, T>) -> Binding<T> {
        Binding(get: { model.config.skills[id]![keyPath: key] }, set: { value in Task { await model.update(id) { $0[keyPath: key] = value } } })
    }
    private func triggerLabel(_ p: SkillPolicy) -> String { ["daily": "每天 " + p.time, "recording_sealed": "录音保存后", "episode_ended": "片段结束后", "manual": "仅手动"][p.trigger] ?? "手动" }
    private static func toolName(_ name: String) -> String { ["search": "搜索", "timeline": "查看时间线", "overview": "查看概览", "get_event": "查看原文", "episodes": "查看片段", "episode": "查看片段", "save_draft": "保存草稿", "annotate": "修改标注", "memories": "查看记忆", "commitment": "整理承诺"][name] ?? "查看资料" }
}
