import AIProviders
import CandidLocalization
import SwiftUI

struct ModelRoleEditor: View {
    let model: ModelSettingsModel
    let info: ModelRoleInfo
    @ViewState var promptOpen = false
    @ViewState private var draft = ""
    @ViewState private var choosing = false
    @ViewState private var fallbackOpen = false

    private var assignment: RoleAssignment? { model.config.assignment(info.id) }
    private var primary: AIModel? { assignment?.model.flatMap { model.config.model($0) } }
    private var engine: BuiltinEngine? { model.builtinEngine(for: info) }

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            Text(info.detail).typography(.secondary).foregroundStyle(.secondary)
            if !info.inUse { Tag(text: L("尚未使用")) }
            SettingRow(title: L("模型")) { picker }
            if info.hasPrompt || primary != nil {
                HStack(spacing: Space.xs) {
                    if info.hasPrompt {
                        ChipButton(title: L("提示词"), systemImage: "text.quote", emphasized: promptOpen) { promptOpen.toggle() }
                    }
                    if primary != nil {
                        ChipButton(title: L("回退模型"), systemImage: "arrow.triangle.branch", emphasized: fallbackOpen) { fallbackOpen.toggle() }
                    }
                }
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
                                 actions: [.init(title: L("保存"), primary: true) { text in
                                     model.setPrompt(text, for: info.id)
                                     if model.error == nil { promptOpen = false }
                                 }],
                                 onCancel: { promptOpen = false })
                }
                .padding(.top, 4)
            }
        }
        .padding(.vertical, 13)
        .animation(Motion.snappy, value: assignment)
        .onChange(of: promptOpen, initial: true) { _, open in
            if open { draft = model.prompt(for: info.id) }
        }
    }

    private func name(_ ref: ModelRef) -> String {
        let provider = model.config.provider(ref.providerID)?.name ?? "?"
        return "\(provider) · \(model.config.model(ref)?.displayName ?? ref.modelID)"
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
                        Text(engine?.title ?? L("未配置")).typography(.body, .medium)
                        if let engine { Text(engine.detail).typography(.caption).foregroundStyle(.secondary).lineLimit(1) }
                    }
                    Spacer(minLength: 0)
                }
                Image(systemName: "chevron.down").typography(.caption).foregroundStyle(.secondary)
            }
            .padding(Space.s).frame(width: 270, alignment: .leading)
            .background(Surface.fill.color, in: RoundedRectangle(cornerRadius: Radius.medium))
            .contentShape(RoundedRectangle(cornerRadius: Radius.medium))
        }.buttonStyle(.plain).accessibilityLabel(L("为") + info.title + L("选择模型"))
    }

    private func warning(for m: AIModel) -> String? {
        if info.needsTools, m.supportsTools == false { return L("这个模型没有声明工具调用，对话 Agent 会退化成“检索后单次回答”。") }
        if info.needsImages, m.supportsImages == false { return L("这个模型声明不接受图片，不适合此角色。") }
        if m.probedMs == nil { return L("尚未测试这个来源，能力未知。") }
        if info.needsImages, m.supportsImages == nil { return L("来源没有说明这个模型能否看图；能用就行，不能用时会报错。") }
        return nil
    }

    private func fallbackLine(_ primary: AIModel) -> some View {
        let candidates = model.config.fallbackCandidates(primary: primary.ref).filter { !(assignment?.fallback.contains($0.ref) ?? false) }
        return HStack(spacing: 6) {
            if let list = assignment?.fallback, !list.isEmpty {
                Text(L("回退")).typography(.secondary).foregroundStyle(.secondary)
                ForEach(list, id: \.self) { ref in
                    Button { model.removeFallback(role: info.id, ref) } label: {
                        HStack(spacing: 4) { Text(name(ref)); Image(systemName: "xmark").typography(.caption, .bold) }
                            .typography(.secondary, .medium).foregroundStyle(.secondary)
                            .padding(.horizontal, 9).frame(height: 22)
                            .background(Capsule().fill(Surface.hover.color))
                    }.buttonStyle(.plain).help(L("移除这个回退"))
                }
            }
            if !candidates.isEmpty {
                QuietMenu(title: L("＋ 回退")) {
                    ForEach(candidates) { c in Button(name(c.ref)) { model.addFallback(role: info.id, c.ref) } }
                }
            }
        }
    }
}
