import AIProviders
import CandidLocalization
import SwiftUI

struct AIFunctionSettingsView: View {
    static let open = Notification.Name("Candid.openAIFunctionSettings")

    enum Tab: String, CaseIterable {
        case features, skills, sources, permissions, usage

        var title: String {
            switch self {
            case .features: L("处理流程")
            case .skills: L("助手技能")
            case .sources: L("模型来源")
            case .permissions: L("远程授权")
            case .usage: L("用量")
            }
        }
    }

    @ViewState private var workflows: WorkflowControlModel
    @ViewState private var models: ModelSettingsModel
    @ViewState private var automation = AutomationModel()
    @ViewState private var tab: Tab
    @ViewState private var addingSource = false
    @ViewState private var selectedRole: ModelRoleInfo?
    @ViewState private var openPrompt = false

    var recognition: DesktopRecognitionSession?
    private let refreshOnAppear: Bool

    init(model: WorkflowControlModel = WorkflowControlModel(), models: ModelSettingsModel = ModelSettingsModel(),
         recognition: DesktopRecognitionSession? = nil, initialTab: Tab = .features, refreshOnAppear: Bool = true) {
        _workflows = ViewState(initialValue: model)
        _models = ViewState(initialValue: models)
        _tab = ViewState(initialValue: initialTab)
        self.recognition = recognition
        self.refreshOnAppear = refreshOnAppear
    }

    var body: some View {
        SettingsPage(title: L("AI 功能")) {
            PillSwitcher(items: Tab.allCases.map { ($0, $0.title) }, selection: $tab).offset(x: -13)
            if let error = models.error { InlineMessage(kind: .error, text: error) }
            if tab == .features || tab == .permissions {
                if let error = workflows.error {
                    InlineMessage(kind: .error, text: error)
                    QuietButton(title: L("重新读取")) { Task { await workflows.refresh(includeActivity: false) } }
                        .disabled(workflows.loading || workflows.saving)
                } else if workflows.loading && workflows.features.isEmpty {
                    ProgressView().controlSize(.small)
                }
            }
            Group {
                switch tab {
                case .features:
                    WorkflowSettingsView(model: workflows, models: models, recognition: recognition, openRole: showRole)
                case .skills:
                    SkillSettingsView(model: automation, refreshOnAppear: refreshOnAppear, openRole: showRole)
                case .sources:
                    VStack(alignment: .leading, spacing: Space.l) {
                        ModelSourcesView(model: models, addingSource: $addingSource)
                        NetworkProxyRow()
                    }
                case .permissions:
                    AIDataPermissionsView(workflows: workflows, automation: automation)
                case .usage:
                    ModelUsageView(model: models)
                }
            }
            .id(tab)
        }
        .task(id: TimelineService.shared.channel?.socketPath) {
            if refreshOnAppear { await workflows.refresh(includeActivity: false) }
        }
        .task(id: tab) {
            guard refreshOnAppear else { return }
            if tab == .permissions {
                await workflows.refresh(includeActivity: false)
                await automation.refresh(includeRuns: false)
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: ModelSettingsModel.configChanged)) { _ in
            models.reload()
            if tab == .permissions { Task { await workflows.refresh(includeActivity: false) } }
        }
        .sheet(item: $selectedRole) { info in // style-exempt: focused editor on top of settings
            AIWorkspaceSheet(title: info.title) {
                if let error = models.error { InlineMessage(kind: .error, text: error) }
                ModelRoleEditor(model: models, info: info, promptOpen: openPrompt)
                if let note = sharedUses(of: info.id) {
                    Text(note).typography(.secondary).foregroundStyle(.secondary)
                }
                if models.config.providers.isEmpty && info.acceptsModels {
                    QuietButton(title: L("添加模型来源"), emphasized: true) {
                        selectedRole = nil
                        tab = .sources
                        addingSource = true
                    }
                }
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: QASnapshot.command)) { note in
            guard let line = note.object as? String else { return }
            if line.hasPrefix("modeltab ") {
                tab = Tab(rawValue: String(line.dropFirst(9))) ?? .features
            } else if line == "modeladd" {
                tab = .sources
                addingSource = true
            } else if line.hasPrefix("modelprompt "), let role = ModelRoleInfo.info(String(line.dropFirst(12))) {
                openPrompt = true
                selectedRole = role
            }
        }
    }

    private func showRole(_ id: String) {
        openPrompt = false
        selectedRole = ModelRoleInfo.info(id)
    }

    private func sharedUses(of role: String) -> String? {
        let titles = workflows.features.filter { $0.mode == "core" && $0.implemented && $0.role == role && $0.id != "transcribe" }
            .map { L($0.title) }
        return titles.count > 1 ? L("共用此模型配置：%@", titles.joined(separator: L("、"))) : nil
    }
}
