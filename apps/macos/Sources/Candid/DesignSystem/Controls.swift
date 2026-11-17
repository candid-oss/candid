import SwiftUI

// Shared controls. Each has one look; a page that needs something else adds a variant here, not a local copy.

/// The one filled button of a region: the action the panel exists for. At most one per region.
struct PrimaryButton: View {
    let title: String
    var isDefault = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(title).typography(.body, .semibold).foregroundStyle(.white)
                .padding(.horizontal, 14).frame(height: ControlHeight.button)
                .background(Capsule().fill(Palette.accent))
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .keyboardShortcut(isDefault ? .defaultAction : nil)
    }
}

/// Text button that only shows a backing on hover: every secondary action.
struct QuietButton: View {
    let title: String
    var role: ButtonRole?
    /// Accent text for the one action a row exists for (授权, 打开设置) without making it a filled button.
    var emphasized = false
    let action: () -> Void
    @ViewState private var hovering = false

    private var tint: Color { role == .destructive ? Palette.danger : emphasized ? Palette.accent : hovering ? .primary : .secondary }

    var body: some View {
        Button(action: action) {
            Text(title).typography(.secondary, emphasized ? .semibold : .medium)
                .foregroundStyle(tint)
                .padding(.horizontal, 9).frame(height: ControlHeight.button)
                .background(Capsule().fill(hovering ? (role == .destructive ? Palette.danger.opacity(0.1) : Surface.hover.color) : .clear))
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .animation(Motion.snappy, value: hovering)
    }
}

/// Delete and other irreversible actions: the first click turns the button into an inline confirmation.
struct ConfirmButton: View {
    let title: String
    let confirmTitle: String
    let action: () -> Void
    @ViewState private var asking = false

    var body: some View {
        HStack(spacing: Space.xxs) {
            if asking {
                QuietButton(title: "取消") { asking = false }
                QuietButton(title: confirmTitle, role: .destructive) { asking = false; action() }
                    .transition(.opacity.combined(with: .scale(scale: 0.96)))
            } else {
                QuietButton(title: title, role: .destructive) { asking = true }
            }
        }
        .animation(Motion.snappy, value: asking)
    }
}

/// Borderless round icon button used in the floating chrome.
struct ChromeButton: View {
    let systemImage: String
    let help: String
    var active = false
    let action: () -> Void
    @ViewState private var hovering = false

    var body: some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .typography(.headline, .medium)
                .foregroundStyle(active ? Color.primary : Color.secondary)
                .frame(width: ControlHeight.field, height: ControlHeight.field)
                .background(Circle().fill(active ? Surface.selected.color : hovering ? Surface.hover.color : .clear))
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .animation(Motion.snappy, value: hovering)
        .help(help).accessibilityLabel(help)
    }
}

/// Switches between views of the same content (tabs): a sliding pill, no box around the group.
struct PillSwitcher<Value: Hashable>: View {
    let items: [(Value, String)]
    @Binding var selection: Value
    var scale: TypeScale = .body
    @Namespace private var space

    var body: some View {
        HStack(spacing: 2) {
            ForEach(items, id: \.0) { value, title in
                let on = value == selection
                Button { withAnimation(Motion.bouncy) { selection = value } } label: {
                    Text(title)
                        .typography(scale, on ? .semibold : .medium)
                        .foregroundStyle(on ? Color.primary : Color.secondary)
                        .padding(.horizontal, 13).frame(height: scale.size + 15)
                        .background {
                            if on {
                                Capsule().fill(Surface.hover.color)
                                    .matchedGeometryEffect(id: "pill", in: space)
                            }
                        }
                        .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(on ? .isSelected : [])
            }
        }
    }
}

/// Picks one value of a setting from a few (up to about five) options; longer lists use a borderless `Menu`.
struct CapsuleChoice<Value: Hashable>: View {
    let options: [(Value, String)]
    @Binding var selection: Value

    var body: some View {
        HStack(spacing: Space.xxs) {
            ForEach(options, id: \.0) { option in
                let on = option.0 == selection
                Button { withAnimation(Motion.snappy) { selection = option.0 } } label: {
                    Text(option.1).typography(.secondary, on ? .semibold : .medium)
                        .foregroundStyle(on ? Color.primary : Color.secondary)
                        .padding(.horizontal, 11).frame(height: ControlHeight.button)
                        .background(Capsule().fill(on ? Surface.selected.color : Surface.wash.color))
                        .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(on ? .isSelected : [])
            }
        }
    }
}

/// Plain text field on a faint fill instead of a bordered box; the label sits above.
struct QuietField: View {
    let title: String
    @Binding var text: String
    var placeholder = ""
    var secure = false
    var mono = false

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(title).typography(.secondary, .medium).foregroundStyle(.secondary)
            Group {
                if secure { SecureField(placeholder, text: $text) } else { TextField(placeholder, text: $text) }
            }
            .textFieldStyle(.plain)
            .font(TypeScale.body.font(design: mono ? .monospaced : .default))
            .padding(.horizontal, 10).frame(height: ControlHeight.field)
            .background(RoundedRectangle(cornerRadius: Radius.small, style: .continuous).fill(Surface.fill.color))
        }
    }
}

/// Separates rows. Groups are separated by space, not lines.
struct HairlineDivider: View {
    var body: some View { Rectangle().fill(Surface.hairline).frame(height: Surface.hairlineWidth) }
}

/// Picks one value from a longer list (more than about five options): the current value and a small chevron
/// on a faint capsule. The list itself is the system menu, so keyboard and accessibility behave natively.
struct MenuChoice<Value: Hashable>: View {
    let options: [(Value, String)]
    @Binding var selection: Value
    /// Shown when the selection matches no option.
    var placeholder = "选择"

    private var current: String { options.first { $0.0 == selection }?.1 ?? placeholder }

    var body: some View {
        Menu {
            ForEach(options, id: \.0) { option in
                Button { selection = option.0 } label: {
                    if option.0 == selection { Label(option.1, systemImage: "checkmark") } else { Text(option.1) }
                }
            }
        } label: {
            MenuCapsuleLabel(title: current)
        }
        .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
        .accessibilityValue(current)
    }
}

/// A button that opens a short list of actions (生成草稿 → 标题 / 摘要 …), drawn like `MenuChoice`.
struct MenuButton<Content: View>: View {
    let title: String
    @ViewBuilder let content: () -> Content

    var body: some View {
        Menu { content() } label: { MenuCapsuleLabel(title: title) }
            .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
    }
}

/// The trigger shared by `MenuChoice` and `MenuButton`.
struct MenuCapsuleLabel: View {
    let title: String
    @ViewState private var hovering = false
    var body: some View {
        HStack(spacing: 5) {
            Text(title).typography(.body, .medium).foregroundStyle(.primary).lineLimit(1)
            Image(systemName: "chevron.down").typography(.caption, .semibold).imageScale(.small).foregroundStyle(.secondary)
        }
        .padding(.leading, 11).padding(.trailing, 9).frame(height: ControlHeight.button)
        .background(Capsule().fill(hovering ? Surface.hover.color : Surface.fill.color))
        .contentShape(Capsule())
        .onHover { hovering = $0 }
    }
}

/// A secondary action that should read as a button at rest (区域里的几个并列操作): faint filled capsule.
/// Use `QuietButton` when the action can stay text-only, `PrimaryButton` for the one main action.
struct SecondaryButton: View {
    let title: String
    var role: ButtonRole?
    let action: () -> Void
    @ViewState private var hovering = false
    @Environment(\.isEnabled) private var enabled

    var body: some View {
        Button(action: action) {
            Text(title).typography(.body, .medium)
                .foregroundStyle(role == .destructive ? Palette.danger : Color.primary)
                .padding(.horizontal, 12).frame(height: ControlHeight.button)
                .background(Capsule().fill(role == .destructive ? Palette.danger.opacity(hovering ? 0.16 : 0.1) : hovering ? Surface.hover.color : Surface.fill.color))
                .contentShape(Capsule())
                .opacity(enabled ? 1 : 0.45)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .animation(Motion.snappy, value: hovering)
    }
}

/// One of several independent switches about the same object (截图 / 文字 / 排除 for an app).
/// On reads as a tinted capsule, off as a faint one.
struct ToggleChip: View {
    let title: String
    @Binding var isOn: Bool
    var tint: Color = Palette.accent
    @ViewState private var hovering = false

    var body: some View {
        Button { withAnimation(Motion.snappy) { isOn.toggle() } } label: {
            Text(title).typography(.secondary, isOn ? .semibold : .medium)
                .foregroundStyle(isOn ? tint : Color.secondary)
                .padding(.horizontal, 10).frame(height: ControlHeight.chip + 2)
                .background(Capsule().fill(isOn ? tint.opacity(0.14) : hovering ? Surface.hover.color : Surface.wash.color))
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .accessibilityAddTraits(isOn ? .isSelected : [])
    }
}

/// Search within the current page: a capsule on a faint fill with a clear button.
struct SearchField: View {
    let prompt: String
    @Binding var text: String
    var focus: FocusState<Bool>.Binding?

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "magnifyingglass").typography(.secondary, .medium).foregroundStyle(.secondary)
            field
            if !text.isEmpty {
                Button { text = "" } label: { Image(systemName: "xmark.circle.fill").foregroundStyle(.tertiary) }
                    .buttonStyle(.plain).accessibilityLabel("清空搜索")
            }
        }
        .padding(.horizontal, Space.s).frame(height: ControlHeight.field)
        .background(Capsule().fill(Surface.fill.color))
    }

    @ViewBuilder private var field: some View {
        let base = TextField(prompt, text: $text).textFieldStyle(.plain).typography(.body)
        if let focus { base.focused(focus) } else { base }
    }
}

/// A whole-number setting adjusted in steps: the value as text, then a small stepper.
struct StepperControl: View {
    let text: String
    @Binding var value: Int
    let range: ClosedRange<Int>
    var step = 1

    var body: some View {
        HStack(spacing: Space.xs) {
            Text(text).typography(.body, .medium, digits: true).contentTransition(.numericText())
            Stepper(text, value: $value, in: range, step: step).labelsHidden()
        }
    }
}

/// A menu that looks like `ChromeButton`: an icon in a circle (more actions in a header).
struct ChromeMenu<Content: View>: View {
    let systemImage: String
    let help: String
    @ViewBuilder let content: () -> Content
    @ViewState private var hovering = false

    var body: some View {
        Menu { content() } label: {
            Image(systemName: systemImage)
                .typography(.headline, .medium).foregroundStyle(.secondary)
                .frame(width: ControlHeight.field, height: ControlHeight.field)
                .background(Circle().fill(hovering ? Surface.hover.color : .clear))
                .contentShape(Circle())
        }
        .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
        .onHover { hovering = $0 }
        .animation(Motion.snappy, value: hovering)
        .help(help).accessibilityLabel(help)
    }
}

/// A menu that looks like `QuietButton`: secondary text, a capsule on hover (speed, add a fallback).
struct QuietMenu<Content: View>: View {
    let title: String
    @ViewBuilder let content: () -> Content
    @ViewState private var hovering = false

    var body: some View {
        Menu { content() } label: {
            Text(title).typography(.secondary, .medium).foregroundStyle(hovering ? .primary : .secondary)
                .padding(.horizontal, 9).frame(height: ControlHeight.button)
                .background(Capsule().fill(hovering ? Surface.hover.color : .clear))
                .contentShape(Capsule())
        }
        .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
        .onHover { hovering = $0 }
        .animation(Motion.snappy, value: hovering)
    }
}

/// A small action that must read as clickable at rest (问 AI, 展开, 撤销): a faint capsule, optional symbol.
/// Use it where a `QuietButton` would look like plain text among other text.
struct ChipButton: View {
    let title: String
    var systemImage: String?
    var role: ButtonRole?
    var emphasized = false
    let action: () -> Void
    @ViewState private var hovering = false
    @Environment(\.isEnabled) private var enabled
    private var tint: Color { role == .destructive ? Palette.danger : emphasized ? Palette.accent : .primary }

    var body: some View {
        Button(action: action) {
            HStack(spacing: 3) {
                if let systemImage { Image(systemName: systemImage).imageScale(.small) }
                Text(title)
            }
            .typography(.caption, .semibold)
            .foregroundStyle(tint.opacity(enabled ? 1 : 0.45))
            .padding(.horizontal, 9).frame(height: ControlHeight.chip)
            .background(Capsule().fill(role == .destructive ? Palette.danger.opacity(hovering ? 0.16 : 0.1)
                                       : emphasized ? Palette.accent.opacity(hovering ? 0.18 : 0.12)
                                       : hovering ? Surface.hover.color : Surface.fill.color))
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .animation(Motion.snappy, value: hovering)
        .accessibilityLabel(title)
    }
}

/// An action that may take a while (回顾今天, 会前要点, running a skill). Idle it is a chip (or the region's primary
/// button); once started it becomes a spinner with what is happening and 取消, which really stops the run.
struct RunButton: View {
    let title: String
    var systemImage: String?
    /// Shown while running (「正在回顾…」).
    let runningTitle: String
    let running: Bool
    var primary = false
    let start: () -> Void
    let cancel: () -> Void

    var body: some View {
        Group {
            if running {
                HStack(spacing: Space.xxs) {
                    ProgressView().controlSize(.mini)
                    Text(runningTitle).typography(.caption, .semibold).foregroundStyle(.secondary).lineLimit(1)
                    Button(action: cancel) {
                        Text("取消").typography(.caption, .semibold).foregroundStyle(Palette.accent)
                            .padding(.horizontal, 7).frame(height: ControlHeight.chip - 4)
                            .background(Capsule().fill(Surface.selected.color)).contentShape(Capsule())
                    }
                    .buttonStyle(.plain).accessibilityLabel("取消" + title)
                }
                .padding(.leading, 8).padding(.trailing, 2).frame(height: ControlHeight.chip)
                .background(Capsule().fill(Surface.fill.color))
                .transition(.opacity.combined(with: .scale(scale: 0.96)))
            } else if primary {
                PrimaryButton(title: title, action: start)
            } else {
                ChipButton(title: title, systemImage: systemImage, emphasized: true, action: start)
            }
        }
        .animation(Motion.snappy, value: running)
    }
}

/// Small visible icon action for source and role rows, outside the window chrome.
struct IconChipButton: View {
    let title: String
    let symbol: String
    let action: () -> Void
    var body: some View {
        Button(action: action) {
            Image(systemName: symbol).typography(.body)
                .frame(width: ControlHeight.button, height: ControlHeight.button)
                .background(Surface.fill.color, in: Circle())
        }.buttonStyle(.plain).help(title).accessibilityLabel(title)
    }
}
