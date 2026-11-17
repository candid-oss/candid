import AIProviders
import CandidLocalization
import SwiftUI

/// Search and capability matching share one rule for the list and suggested assignments.
enum RoleModelMatch {
    static func reason(_ model: AIModel, role: ModelRoleInfo, provider: AIProvider?) -> String? {
        if provider?.enabled != true { return L("来源已停用") }
        if role.needsImages && model.supportsImages == false { return L("不接受图片") }
        if role.needsTools && model.supportsTools == false { return L("不支持工具调用") }
        return nil
    }

    static func candidates(_ config: ProviderConfig, role: ModelRoleInfo, query: String, filter: String) -> [AIModel] {
        config.models.filter { m in
            let provider = config.provider(m.providerID)
            let text = [provider?.name ?? "", m.displayName ?? "", m.modelID].joined(separator: " ")
            return (query.isEmpty || text.localizedCaseInsensitiveContains(query))
                && (filter != "images" || m.supportsImages == true)
                && (filter != "tools" || m.supportsTools == true)
                && (filter != "suitable" || reason(m, role: role, provider: provider) == nil)
                && (filter != "local" || provider?.destination == .onDevice)
                && (filter != "free" || (m.cost?.perMtok.input == 0 && m.cost?.perMtok.output == 0))
        }.sorted { a, b in
            let aOK = reason(a, role: role, provider: config.provider(a.providerID)) == nil
            let bOK = reason(b, role: role, provider: config.provider(b.providerID)) == nil
            if aOK != bOK { return aOK }
            return (config.provider(a.providerID)?.name ?? "", a.displayName ?? a.modelID)
                < (config.provider(b.providerID)?.name ?? "", b.displayName ?? b.modelID)
        }
    }
}

struct RoleModelChooser: View {
    let model: ModelSettingsModel
    let info: ModelRoleInfo
    let close: () -> Void
    @ViewState private var query = ""
    @ViewState private var filter = "suitable"

    var body: some View {
        InlinePanel {
            VStack(alignment: .leading, spacing: Space.s) {
                HStack {
                    SearchField(prompt: L("搜索来源或模型"), text: $query)
                    ChipButton(title: L("收起"), systemImage: "chevron.up", action: close)
                }
                PillSwitcher(items: [("suitable", L("适合此角色")), ("all", L("全部")), ("tools", L("工具")), ("images", L("看图")), ("local", L("本机")), ("free", L("免费"))], selection: $filter, scale: .secondary)
                if query.isEmpty && ["all", "suitable"].contains(filter) {
                    ForEach(info.engines) { engine in
                        choice(engine.title, detail: engine.detail, selected: model.config.assignment(info.id)?.model == nil,
                               reason: nil) { model.useEngine(role: info.id, engine); close() }
                    }
                    if info.engines.isEmpty {
                        choice(L("不使用"), detail: nil, selected: model.config.assignment(info.id)?.model == nil,
                               reason: nil) { model.assign(role: info.id, to: nil); close() }
                    }
                }
                let options = RoleModelMatch.candidates(model.config, role: info, query: query, filter: filter)
                if options.isEmpty { Text(L("没有匹配的模型")).typography(.secondary).foregroundStyle(.secondary) }
                ScrollView {
                    LazyVStack(spacing: Space.xxs) {
                        ForEach(options) { m in
                            let provider = model.config.provider(m.providerID)
                            choice(m.displayName ?? m.modelID,
                                   detail: [provider?.name ?? "", m.supportsImages == true ? L("看图") : nil, m.supportsTools == true ? L("工具") : nil,
                                            m.contextWindow.map { L("上下文 %@", $0) }].compactMap { $0 }.joined(separator: " · "),
                                   selected: model.config.assignment(info.id)?.model == m.ref,
                                   reason: RoleModelMatch.reason(m, role: info, provider: provider)) {
                                model.assign(role: info.id, to: m.ref); close()
                            }
                        }
                    }
                }.frame(maxHeight: 280)
            }
        }
    }

    private func choice(_ title: String, detail: String?, selected: Bool, reason: String?, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: Space.s) {
                VStack(alignment: .leading, spacing: Space.xxs) {
                    Text(title).typography(.body, .medium)
                    if let detail { Text(detail).typography(.caption).foregroundStyle(.secondary) }
                    if let reason { Text(reason).typography(.caption).foregroundStyle(.secondary) }
                }
                Spacer()
                if selected { Image(systemName: "checkmark").foregroundStyle(Color.accentColor) }
            }
            .padding(Space.s).frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: Radius.medium).fill(selected ? Surface.selected.color : Surface.fill.color))
            .contentShape(Rectangle())
        }.buttonStyle(.plain).disabled(reason != nil).opacity(reason == nil ? 1 : 0.5)
    }
}
