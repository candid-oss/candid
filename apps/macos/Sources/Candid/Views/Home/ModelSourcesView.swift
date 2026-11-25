import AIProviders
import CandidLocalization
import CandidShared
import SwiftUI

struct ModelSourcesView: View {
    let model: ModelSettingsModel
    @Binding var addingSource: Bool
    /// providerID being edited, or "new"; one panel open at a time, unfolded in place.
    @ViewState private var editing: String?
    @ViewState private var newDraft = ProviderPreset.all[0].makeProvider()

    var body: some View {
        VStack(alignment: .leading, spacing: Space.s) {
            HStack {
                Text(L("服务商与本机模型")).typography(.headline)
                Spacer()
                ChipButton(title: L("添加来源"), systemImage: "plus", emphasized: true) {
                    addingSource = true
                }
            }
            if model.config.providers.isEmpty && editing != "new" {
                Text(L("还没有模型来源")).typography(.secondary).foregroundStyle(.secondary)
                    .padding(.vertical, Space.row)
            }
            if editing == "new" {
                InlinePanel { ProviderForm(model: model, initial: newDraft, isNew: true) { editing = nil } }
                    .id(newDraft.providerID)
            }
            ForEach(model.config.providers) { provider in
                if provider.providerID != model.config.providers.first?.providerID { HairlineDivider() }
                ProviderRow(model: model, provider: provider, open: editing == provider.providerID) {
                    editing = editing == provider.providerID ? nil : provider.providerID
                }
                if editing == provider.providerID {
                    InlinePanel { ProviderForm(model: model, initial: provider, isNew: false) { editing = nil } }
                        .id(provider.providerID)
                }
            }
        }
        .onChange(of: addingSource, initial: true) { _, add in
            if add {
                newDraft = ProviderPreset.all[0].makeProvider()
                editing = "new"
                addingSource = false
            }
        }
    }
}

/// The app-wide proxy choice. It sits with the model sources because that is where an unreachable endpoint shows up.
struct NetworkProxyRow: View {
    @AppStorage(NetworkProxy.defaultsKey) private var mode = NetworkProxy.system.rawValue

    var body: some View {
        SettingRow(title: L("网络")) {
            CapsuleChoice(options: [(NetworkProxy.system.rawValue, L("系统代理")), (NetworkProxy.direct.rawValue, L("不走代理"))], selection: $mode)
        }
    }
}

private struct ProviderRow: View {
    let model: ModelSettingsModel
    let provider: AIProvider
    let open: Bool
    let toggle: () -> Void

    private var state: ModelSettingsModel.ProbeState { model.probes[provider.providerID] ?? .idle }
    private var count: Int { model.config.models(of: provider.providerID).count }
    private var host: String {
        guard let url = URL(string: provider.baseURL), let host = url.host(percentEncoded: false) else { return provider.baseURL }
        return host + (url.port.map { ":\($0)" } ?? "")
    }

    var body: some View {
        HStack(spacing: Space.s) {
            SourceBadge(title: provider.name, destination: provider.destination)
            VStack(alignment: .leading, spacing: Space.xxs) {
                HStack(spacing: Space.xs) {
                    Text(provider.name).typography(.headline).lineLimit(1)
                    DestinationTag(destination: provider.destination)
                }
                Text(host + " · " + L("%@ 个模型", count))
                    .typography(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer(minLength: Space.xs)
            status
            IconChipButton(title: L("测试 ") + provider.name, symbol: "arrow.clockwise") { model.test(provider) }.disabled(state == .testing)
            IconChipButton(title: L("编辑 ") + provider.name, symbol: open ? "chevron.up" : "pencil", action: toggle)
            Toggle(L("启用来源"), isOn: Binding(get: { provider.enabled }, set: { model.setEnabled(provider, $0) }))
                .labelsHidden().toggleStyle(.switch).controlSize(.small)
        }
        .padding(.vertical, Space.row)
    }

    @ViewBuilder private var status: some View {
        if !provider.enabled {
            Text(L("已停用")).typography(.secondary).foregroundStyle(.secondary)
        } else {
            switch state {
            case .idle: Text(L("尚未测试")).typography(.secondary).foregroundStyle(.secondary)
            case .testing: ProgressView().controlSize(.small)
            case .ok(let ms, _): Text(L("在线 · %@ ms", ms)).typography(.secondary, digits: true).foregroundStyle(.secondary)
            case .failed(let message): Text(message).typography(.secondary).foregroundStyle(Palette.danger).lineLimit(1).help(message)
            }
        }
    }
}

/// Capsule choices in the page's own language, instead of system pickers.
private struct ProviderForm: View {
    let model: ModelSettingsModel
    let isNew: Bool
    let close: () -> Void
    @ViewState private var provider: AIProvider
    @ViewState private var key = ""
    @ViewState private var preset = "lmstudio"
    @ViewState private var trial: ProbeResult?
    @ViewState private var testing = false
    @ViewState private var selectedModels: Set<String> = []
    @ViewState private var assignSuggested = false

    init(model: ModelSettingsModel, initial: AIProvider, isNew: Bool, close: @escaping () -> Void) {
        self.model = model; self.isNew = isNew; self.close = close
        _provider = ViewState(initialValue: initial)
    }

    private var hasSavedKey: Bool { model.keyPresent.contains(provider.providerID) }
    private var needsKey: Bool { provider.destination == .cloud || provider.secretRef != nil || !key.isEmpty }
    private var isLocalHost: Bool {
        let h = URL(string: provider.baseURL)?.host(percentEncoded: false) ?? ""
        return ["localhost", "127.0.0.1", "::1"].contains(h) || h.hasSuffix(".localhost")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            if isNew {
                Text(L("添加模型来源")).typography(.section)
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 168), spacing: 8)], alignment: .leading, spacing: 8) {
                    ForEach(ProviderPreset.all) { p in PresetTile(preset: p, selected: p.id == preset) { applyPreset(p) } }
                }
            }
            HStack(alignment: .top, spacing: 14) {
                QuietField(title: L("名称"), text: $provider.name)
                QuietField(title: L("地址"), text: $provider.baseURL, placeholder: "http://localhost:1234/v1", mono: true)
            }
            HStack(alignment: .top, spacing: 22) {
                VStack(alignment: .leading, spacing: 6) {
                    Text(L("接口类型")).typography(.secondary, .medium).foregroundStyle(.secondary)
                    CapsuleChoice(options: [(ProviderAPI.openaiChat, L("OpenAI 兼容")), (.anthropicMessages, "Anthropic"), (.ollama, "Ollama")],
                                  selection: $provider.api)
                }
                VStack(alignment: .leading, spacing: 6) {
                    Text(L("内容发往")).typography(.secondary, .medium).foregroundStyle(.secondary)
                    CapsuleChoice(options: Destination.allCases.map { ($0, $0.label) }, selection: $provider.destination)
                }
            }
            if provider.destination == .onDevice && !isLocalHost {
                Text(L("地址不是本机；如果它在另一台机器上，请改选「自建」。")).typography(.secondary).foregroundStyle(.orange)
            }
            if provider.destination == .cloud { Text(L("选择云端后，用到这个来源的内容会发往第三方服务。")).typography(.secondary).foregroundStyle(.secondary) }
            QuietField(title: provider.destination == .onDevice ? L("API 令牌（服务开启鉴权时填写）") : L("API 密钥"), text: $key,
                       placeholder: hasSavedKey ? L("已保存在钥匙串；留空则保持不变") : L("仅保存在钥匙串"), secure: true)
            if let trial {
                HStack(spacing: 6) {
                    Image(systemName: trial.reachable ? "checkmark.circle.fill" : "exclamationmark.circle.fill")
                    Text(trial.reachable ? L("已连接 · %@ ms · 读到 %@ 个模型，%@ 个能看图", trial.latencyMs ?? 0, trial.models.count, trial.models.filter { $0.supportsImages == true }.count) : (trial.error ?? L("失败")))
                }
                .typography(.secondary).foregroundStyle(trial.reachable ? Color.green : Color.red)
                .transition(.opacity)
            }
            if isNew, let trial, trial.reachable {
                Text(L("确认模型")).typography(.headline)
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 190), spacing: Space.xs)], alignment: .leading) {
                    ForEach(trial.models) { item in
                        ToggleChip(title: item.displayName ?? item.modelID, isOn: Binding(get: { selectedModels.contains(item.modelID) }, set: { on in
                            if on { selectedModels.insert(item.modelID) } else { selectedModels.remove(item.modelID) }
                        }))
                    }
                }
                ToggleChip(title: L("为未配置的角色按能力分配模型"), isOn: $assignSuggested)
            }
            HStack(spacing: 8) {
                if !isNew {
                    ConfirmButton(title: L("删除…"), confirmTitle: L("删除来源和密钥")) { model.delete(provider); close() }
                        .help(L("使用它的角色会变成未选择；已记录的调用保留。"))
                }
                Spacer()
                if testing { ProgressView().controlSize(.small) }
                QuietButton(title: L("测试连接")) { runTrial() }.disabled(testing)
                QuietButton(title: L("取消"), action: close)
                PrimaryButton(title: isNew ? L("添加") : L("保存"), isDefault: true) { commit() }
                    .disabled(provider.name.isEmpty || provider.baseURL.count < 8 || (isNew && trial?.reachable != true))
            }
        }
        .animation(Motion.snappy, value: trial?.reachable)
        .onChange(of: provider.baseURL) { _, _ in trial = nil }
        .onChange(of: provider.api) { _, _ in trial = nil }
        .onChange(of: key) { _, _ in trial = nil }
    }

    private func applyPreset(_ p: ProviderPreset) {
        preset = p.id
        var next = p.makeProvider()
        next.providerID = provider.providerID
        if next.secretRef != nil { next.secretRef = "provider.\(provider.providerID)" }
        withAnimation(Motion.snappy) { provider = next; trial = nil }
    }

    private func runTrial() {
        testing = true; trial = nil
        let p = provider, k = key
        Task {
            let result = await model.trial(p, key: k)
            if provider.baseURL == p.baseURL && provider.api == p.api && key == k { trial = result; selectedModels = Set(result.models.map(\.modelID)) }
            testing = false
        }
    }

    private func commit() {
        var p = provider
        p.baseURL = p.baseURL.trimmingCharacters(in: .whitespaces)
        if needsKey, p.secretRef == nil { p.secretRef = "provider.\(p.providerID)" }
        model.save(p, key: key)
        if model.error == nil {
            if var trial, trial.reachable {
                if isNew { trial.models.removeAll { !selectedModels.contains($0.modelID) } }
                model.acceptProbe(trial, for: p)
                if isNew && assignSuggested { model.assignSuggestions(from: p) }
            }
            else { model.test(p) }
            close()
        }
    }
}

/// A selectable tile in a small grid of starting points.
struct PresetTile: View {
    let preset: ProviderPreset
    let selected: Bool
    let action: () -> Void
    @ViewState private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(alignment: .top, spacing: 10) {
                Image(systemName: preset.destination.symbol).typography(.body, .medium)
                    .foregroundStyle(preset.destination.tint)
                    .frame(width: 28, height: 28)
                    .background(Circle().fill(preset.destination.tint.opacity(0.12)))
                VStack(alignment: .leading, spacing: 2) {
                    Text(preset.name).typography(.body, .semibold).foregroundStyle(.primary)
                    Text(preset.hint).typography(.caption, .regular).foregroundStyle(.secondary).lineLimit(2).fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 0)
            }
            .padding(10)
            .frame(maxWidth: .infinity, minHeight: 70, alignment: .topLeading)
            .panelSurface(radius: Radius.medium, level: selected ? .hover : hovering ? .fill : .wash)
            .selectionEdge(selected, radius: Radius.medium)
            .contentShape(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .animation(Motion.snappy, value: hovering)
        .animation(Motion.snappy, value: selected)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}
