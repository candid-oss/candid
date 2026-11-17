import AIProviders
import SwiftUI

/// Roles grouped by what they do. Each row picks a built-in engine or a model, may add fallbacks (only to the
/// same or a more local destination), and unfolds its prompt in place.
struct ModelRolesView: View {
    let model: ModelSettingsModel
    @Binding var qaRequest: String?
    @ViewState private var editingPrompt: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 26) {
            ForEach([ModelRoleInfo.Group.perception, .understanding], id: \.self) { group in
                VStack(alignment: .leading, spacing: 0) {
                    Text(group.rawValue).typography(.secondary, .semibold).foregroundStyle(.secondary).padding(.bottom, 4)
                    ForEach(ModelRoleInfo.catalog.filter { $0.group == group }) { info in
                        RoleRow(model: model, info: info, editingPrompt: $editingPrompt)
                        HairlineDivider()
                    }
                }
            }
            Text("回退只会换到同样或更本地的来源：本机角色不会悄悄改用云端。")
                .typography(.secondary).foregroundStyle(.secondary)
        }
        .animation(Motion.spring, value: editingPrompt)
        .onChange(of: qaRequest) { _, request in
            if let request, request.hasPrefix("modelprompt ") { editingPrompt = String(request.dropFirst(12)); qaRequest = nil }
        }
    }
}

private struct RoleRow: View {
    let model: ModelSettingsModel
    let info: ModelRoleInfo
    @Binding var editingPrompt: String?
    @ViewState private var draft = ""
    @ViewState private var choosing = false
    @ViewState private var fallbackOpen = false

    private var assignment: RoleAssignment? { model.config.assignment(info.id) }
    private var primary: AIModel? { assignment?.model.flatMap { model.config.model($0) } }
    /// The engine in effect: the chosen one, or the role's default when no model is chosen.
    private var engine: BuiltinEngine? {
        guard assignment?.model == nil else { return nil }
        return info.engines.first { $0.id == assignment?.engine } ?? info.defaultEngine
    }
    private var promptOpen: Bool { editingPrompt == info.id }

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            HStack(alignment: .center, spacing: Space.s) {
                VStack(alignment: .leading, spacing: Space.xxs) {
                    HStack(spacing: Space.xs) {
                        Text(info.title).typography(.headline)
                        if !info.inUse { Tag(text: "尚未使用") }
                    }
                    Text(info.detail).typography(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                }.frame(maxWidth: .infinity, alignment: .leading)
                picker
                HStack(spacing: Space.xs) {
                    if info.hasPrompt {
                        IconChipButton(title: "编辑" + info.title + "提示词", symbol: "text.quote") {
                            if promptOpen { editingPrompt = nil } else { draft = model.prompt(for: info.id); editingPrompt = info.id }
                        }
                    } else { Color.clear.frame(width: ControlHeight.button, height: ControlHeight.button) }
                    if primary != nil {
                        IconChipButton(title: "设置" + info.title + "回退", symbol: "arrow.triangle.branch") { fallbackOpen.toggle() }
                    } else { Color.clear.frame(width: ControlHeight.button, height: ControlHeight.button) }
                }.frame(width: ControlHeight.button * 2 + Space.xs)

            }
            if choosing {
                RoleModelChooser(model: model, info: info) { choosing = false }
            }
            if let primary, fallbackOpen {
                if let warning = warning(for: primary) { InlineMessage(kind: .warning, text: warning) }
                fallbackLine(primary)
            }
            if promptOpen {
                InlinePanel {
                    PromptEditor(defaultText: PromptCatalog.defaultPrompt(info.id), presets: PromptCatalog.presets(info.id),
                                 text: $draft,
                                 actions: [.init(title: "保存", primary: true) { text in
                                     model.setPrompt(text, for: info.id); editingPrompt = nil
                                 }],
                                 onCancel: { editingPrompt = nil })
                }
                .padding(.top, 4)
            }
        }
        .padding(.vertical, 13)
        .animation(Motion.snappy, value: assignment)
        .onChange(of: editingPrompt) { _, open in if open == info.id && draft.isEmpty { draft = model.prompt(for: info.id) } }
    }

    private func name(_ ref: ModelRef) -> String {
        let provider = model.config.provider(ref.providerID)?.name ?? "?"
        return "\(provider) · \(model.config.model(ref)?.displayName ?? ref.modelID)"
    }

    private var current: String {
        if let ref = assignment?.model { return name(ref) }
        if let engine { return engine.title + (assignment?.engine == nil ? "（默认）" : "") }
        return "未选择"
    }

    private var picker: some View {
        Button { choosing.toggle() } label: {
            HStack(spacing: Space.s) {
                if let ref = assignment?.model, let provider = model.config.provider(ref.providerID) {
                    SourceBadge(title: provider.name, destination: provider.destination)
                    VStack(alignment: .leading, spacing: Space.xxs) {
                        Text(model.config.model(ref)?.displayName ?? ref.modelID).typography(.body, .medium).lineLimit(1)
                        Text(provider.name).typography(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }
                    Spacer(minLength: 0)
                    DestinationTag(destination: provider.destination)
                } else {
                    VStack(alignment: .leading, spacing: Space.xxs) {
                        Text(engine?.title ?? "未配置").typography(.body, .medium)
                        if let engine { Text(engine.detail).typography(.caption).foregroundStyle(.secondary).lineLimit(1) }
                    }
                    Spacer(minLength: 0)
                }
                Image(systemName: "chevron.down").typography(.caption).foregroundStyle(.secondary)
            }
            .padding(Space.s).frame(width: 270, alignment: .leading)
            .background(Surface.fill.color, in: RoundedRectangle(cornerRadius: Radius.medium))
            .contentShape(RoundedRectangle(cornerRadius: Radius.medium))
        }.buttonStyle(.plain).accessibilityLabel("为" + info.title + "选择模型")
    }

    private func warning(for m: AIModel) -> String? {
        if info.needsTools, m.supportsTools == false { return "这个模型没有声明工具调用，对话 Agent 会退化成“检索后单次回答”。" }
        if info.needsImages, m.supportsImages == false { return "这个模型声明不接受图片，不适合此角色。" }
        if m.probedMs == nil { return "尚未测试这个来源，能力未知。" }
        if info.needsImages, m.supportsImages == nil { return "来源没有说明这个模型能否看图；能用就行，不能用时会报错。" }
        return nil
    }

    private func fallbackLine(_ primary: AIModel) -> some View {
        let candidates = model.config.fallbackCandidates(primary: primary.ref).filter { !(assignment?.fallback.contains($0.ref) ?? false) }
        return HStack(spacing: 6) {
            if let list = assignment?.fallback, !list.isEmpty {
                Text("回退").typography(.secondary).foregroundStyle(.secondary)
                ForEach(list, id: \.self) { ref in
                    Button { model.removeFallback(role: info.id, ref) } label: {
                        HStack(spacing: 4) { Text(name(ref)); Image(systemName: "xmark").typography(.caption, .bold) }
                            .typography(.secondary, .medium).foregroundStyle(.secondary)
                            .padding(.horizontal, 9).frame(height: 22)
                            .background(Capsule().fill(Surface.hover.color))
                    }.buttonStyle(.plain).help("移除这个回退")
                }
            }
            if !candidates.isEmpty {
                QuietMenu(title: "＋ 回退") {
                    ForEach(candidates) { c in Button(name(c.ref)) { model.addFallback(role: info.id, c.ref) } }
                }
            }
        }
    }
}
