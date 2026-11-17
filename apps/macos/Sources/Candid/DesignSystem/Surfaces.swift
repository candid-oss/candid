import SwiftUI

// The two surface levels above the window's frosted background: a faint wash that unfolds in place,
// and a floating material for things that hover over content. Nothing gets a border except focus and selection.

/// A faint, borderless panel that unfolds under a row: the in-place alternative to a sheet.
struct InlinePanel<Content: View>: View {
    @ViewBuilder let content: Content
    var body: some View {
        content
            .padding(Space.m)
            .panelSurface(radius: Radius.large)
            .transition(Motion.unfold)
    }
}

extension View {
    /// Hover cards, peeks and popovers drawn by the app: material in the glass styles, a raised solid in the
    /// flat style; always a hairline edge and a soft drop shadow.
    func floatingSurface(radius: CGFloat = Radius.large) -> some View {
        modifier(FloatingSurface(shape: RoundedRectangle(cornerRadius: radius, style: .continuous)))
    }

    /// A resting panel: a faint wash over glass, or a white (dark: lifted gray) panel with a soft shadow on flat.
    /// `level` is the wash used over glass, so hover and selection stay visible in every style.
    func panelSurface(radius: CGFloat = Radius.large, level: Surface = .wash) -> some View {
        modifier(PanelSurface(radius: radius, level: level))
    }

    /// Selection and focus are the only outlines: an accent edge, plus a soft glow while focused.
    func selectionEdge(_ on: Bool, radius: CGFloat = Radius.medium, glow: Bool = false) -> some View {
        overlay(RoundedRectangle(cornerRadius: radius, style: .continuous).strokeBorder(Palette.accent.opacity(on ? 0.5 : 0), lineWidth: 1))
            .shadow(color: Palette.accent.opacity(on && glow ? 0.12 : 0), radius: 8)
    }
}

extension View {
    /// Scrolling content fades out before it reaches the floating chrome instead of sliding under it.
    func fadesUnderChrome(top: CGFloat = 52, fade: CGFloat = 28) -> some View {
        mask {
            VStack(spacing: 0) {
                Color.clear.frame(height: top)
                LinearGradient(colors: [.clear, .black], startPoint: .top, endPoint: .bottom).frame(height: fade)
                Color.black
            }
            .ignoresSafeArea()
        }
    }
}

private struct InsideRaisedKey: EnvironmentKey { static let defaultValue = false }

extension EnvironmentValues {
    /// True inside a raised panel: nested panels use a tint instead of raising again.
    var insideRaisedPanel: Bool {
        get { self[InsideRaisedKey.self] }
        set { self[InsideRaisedKey.self] = newValue }
    }
}

private struct PanelSurface: ViewModifier {
    let radius: CGFloat
    let level: Surface
    @Environment(\.visualStyle) private var style
    @Environment(\.insideRaisedPanel) private var nested

    func body(content: Content) -> some View {
        let shape = RoundedRectangle(cornerRadius: radius, style: .continuous)
        let raised = style == .flat && !nested
        content
            .environment(\.insideRaisedPanel, nested || raised)
            .background {
            if raised {
                ZStack {
                    shape.fill(Palette.raised).shadow(color: .black.opacity(0.07), radius: 6, y: 2)
                    if level != .wash { shape.fill(level.color) }
                }
            } else {
                shape.fill(level.color)
            }
        }
    }
}

struct FloatingSurface<S: InsettableShape>: ViewModifier {
    let shape: S
    @Environment(\.visualStyle) private var style

    func body(content: Content) -> some View {
        content
            .background {
                if style == .flat { shape.fill(Palette.raised) } else { shape.fill(.regularMaterial) }
            }
            .overlay(shape.strokeBorder(Surface.hairline, lineWidth: Surface.hairlineWidth))
            .shadow(color: .black.opacity(style == .flat ? 0.12 : 0.18), radius: 18, y: 10)
    }
}

/// Section heading inside a page: one level only, no subtitle unless it carries a decision.
struct SectionHeader: View {
    let title: String
    var note: String?

    var body: some View {
        VStack(alignment: .leading, spacing: Space.xxs) {
            Text(title).typography(.section)
            if let note { Text(note).typography(.secondary).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true) }
        }
    }
}

/// A settings section page: one heading, then groups. Scrolls under the chrome without touching it.
struct SettingsPage<Content: View>: View {
    let title: String
    var accessory: AnyView? = nil
    @ViewBuilder let content: Content

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Space.l) {
                HStack {
                    Text(title).typography(.section)
                    Spacer()
                    accessory
                }
                content
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, Space.page).padding(.bottom, Space.xl)
        }
        .scrollIndicators(.hidden)
    }
}

/// A named group of settings rows. Rows inside are separated by `HairlineDivider`; groups by space.
struct SettingsGroup<Content: View, Accessory: View>: View {
    let title: String
    @ViewBuilder let content: Content
    @ViewBuilder var accessory: Accessory

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline) {
                Text(title).typography(.headline)
                Spacer()
                accessory
            }
            .padding(.bottom, Space.xxs)
            content
        }
    }
}

extension SettingsGroup where Accessory == EmptyView {
    init(title: String, @ViewBuilder content: () -> Content) {
        self.init(title: title, content: content, accessory: { EmptyView() })
    }
}

/// Advanced settings folded under one row; unfolds in place.
struct FoldRow<Content: View>: View {
    let title: String
    /// What is inside while folded (「已选 2 / 4」); optional.
    var detail: String? = nil
    @ViewBuilder let content: Content
    @ViewState private var open = false

    init(title: String, detail: String? = nil, @ViewBuilder content: () -> Content) {
        self.title = title; self.detail = detail; self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button { withAnimation(Motion.spring) { open.toggle() } } label: {
                HStack {
                    Text(title).typography(.body, .medium)
                    Spacer()
                    if let detail { Text(detail).typography(.secondary).foregroundStyle(.secondary).lineLimit(1) }
                    Image(systemName: "chevron.down").typography(.caption, .bold).foregroundStyle(.secondary)
                        .rotationEffect(.degrees(open ? 180 : 0))
                }
                .padding(.vertical, Space.row)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityAddTraits(open ? .isSelected : [])
            if open {
                InlinePanel { VStack(alignment: .leading, spacing: 0) { content } }
                    .padding(.bottom, Space.s)
            }
        }
    }
}

/// A setting: title (and a consequence, when there is one) on the left, its control on the right.
struct SettingRow<Control: View>: View {
    let title: String
    var detail: String?
    @ViewBuilder let control: Control

    var body: some View {
        HStack(alignment: .center, spacing: Space.m) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title).typography(.body, .medium)
                if let detail { Text(detail).typography(.secondary).foregroundStyle(.secondary) }
            }
            Spacer(minLength: Space.m)
            control
        }
        .padding(.vertical, Space.row)
    }
}
