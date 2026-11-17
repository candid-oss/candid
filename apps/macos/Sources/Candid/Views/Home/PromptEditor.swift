import SwiftUI

/// Inline prompt editor shared by 设置 → 模型 and per-screenshot recognition. It unfolds in place (no sheet),
/// sits on a faint tint instead of a bordered box, and lights its edge softly while focused.
struct PromptEditor: View {
    struct Action {
        let title: String
        var primary = false
        let run: (String) -> Void
    }

    let defaultText: String
    let presets: [PromptCatalog.Preset]
    @Binding var text: String
    var minHeight: CGFloat = 170
    var maxHeight: CGFloat = 340
    /// Shown right-aligned; the last primary action is the default button.
    let actions: [Action]
    var onCancel: (() -> Void)?

    @FocusState private var focused: Bool

    private var isDefault: Bool { text.trimmingCharacters(in: .whitespacesAndNewlines) == defaultText.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var activePreset: String? { presets.first { $0.text.trimmingCharacters(in: .whitespacesAndNewlines) == text.trimmingCharacters(in: .whitespacesAndNewlines) }?.id }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: Space.xs) {
                Text("提示词").typography(.secondary, .semibold).foregroundStyle(.secondary)
                StateTag(isDefault: isDefault)
                Spacer(minLength: Space.xs)
                if presets.count > 1 {
                    HStack(spacing: Space.xxs) {
                        Text("起点").typography(.secondary).foregroundStyle(.tertiary)
                        CapsuleChoice(options: presets.map { ($0.id, $0.title) },
                                      selection: Binding(get: { activePreset ?? "" },
                                                         set: { id in if let p = presets.first(where: { $0.id == id }) { text = p.text } }))
                    }
                }
            }

            TextEditor(text: $text)
                .font(TypeScale.body.font())
                .lineSpacing(4)
                .scrollContentBackground(.hidden)
                .scrollIndicators(.never)
                .focused($focused)
                .padding(.horizontal, Space.s).padding(.vertical, 10)
                .frame(minHeight: minHeight, maxHeight: maxHeight)
                .background(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous).fill(focused ? Surface.fill.color : Surface.wash.color))
                .selectionEdge(focused, radius: Radius.medium, glow: true)
                .animation(Motion.snappy, value: focused)
                .accessibilityLabel("提示词")

            HStack(spacing: 6) {
                Text("\(text.count) 字").typography(.secondary, digits: true).foregroundStyle(.tertiary)
                Spacer()
                QuietButton(title: "恢复默认") { withAnimation(Motion.snappy) { text = defaultText } }
                    .disabled(isDefault).opacity(isDefault ? 0.4 : 1)
                if let onCancel { QuietButton(title: "取消", action: onCancel) }
                ForEach(actions.indices, id: \.self) { i in
                    let action = actions[i]
                    if action.primary {
                        PrimaryButton(title: action.title, isDefault: i == actions.lastIndex(where: \.primary)) { action.run(text) }
                    } else {
                        QuietButton(title: action.title) { action.run(text) }
                    }
                }
            }
        }
        .onAppear { focused = true }
    }
}
