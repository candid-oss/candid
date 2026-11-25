import CandidLocalization
import SwiftUI

struct AIDataPermissionsView: View {
    let workflows: WorkflowControlModel
    let automation: AutomationModel

    static let mediaSources = [
        ("import", L("导入资料")), ("mac_microphone", L("麦克风")),
        ("mac_system_audio", L("系统声音与麦克风")), ("mac_region", L("框选截图"))
    ]

    var body: some View {
        SettingsGroup(title: L("资料处理")) {
            if workflows.remoteProviders.isEmpty {
                if !workflows.loading {
                    Text(L("没有可授权的远程模型来源")).typography(.secondary).foregroundStyle(.secondary)
                        .padding(.vertical, Space.row)
                }
            } else {
                Text(L("按模型来源授权发送录音和导入资料。")).typography(.secondary).foregroundStyle(.secondary)
                ForEach(workflows.remoteProviders) { provider in
                    FoldRow(title: provider.name, detail: L("已授权 %@ 项", Self.mediaSources.filter {
                        workflows.approvals[$0.0 + ":" + provider.providerID] == true
                    }.count)) {
                        ForEach(Self.mediaSources, id: \.0) { source, title in
                            SwitchRow(title: title, isOn: Binding(
                                get: { workflows.approvals[source + ":" + provider.providerID] == true },
                                set: { on in Task { await workflows.authorize(provider, source: source, allowed: on) } }
                            ))
                            .disabled(workflows.saving || workflows.loading)
                        }
                    }
                    if provider.providerID != workflows.remoteProviders.last?.providerID { HairlineDivider() }
                }
            }
        }
        SettingsGroup(title: L("时间线与技能")) {
            if let error = automation.error { InlineMessage(kind: .error, text: error) }
            Text(L("允许这些时间线来源发送给远程模型。各功能仍需单独允许使用远程模型。"))
                .typography(.secondary).foregroundStyle(.secondary)
            let sources = automation.sources.filter(\.readable)
            ForEach(sources) { source in
                SwitchRow(title: source.display_name, isOn: Binding(
                    get: { automation.config.remote_sources.contains(source.id) },
                    set: { on in Task { await automation.authorize(source.id, allowed: on) } }
                ))
                .disabled(automation.busy || automation.loading)
                if source.id != sources.last?.id { HairlineDivider() }
            }
        }
    }
}
