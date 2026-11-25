import SwiftUI
import CandidLocalization

struct SettingsHomeView: View {
    @Bindable var session: DesktopSession
    var deviceSettings: AnyView?
    @AppStorage("desktop.settings.tab") private var selection = "capture"
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private typealias Item = (id: String, title: String, symbol: String)

    private var groups: [(title: String, items: [Item])] {
        var system: [Item] = [
            ("storage", L("数据"), "internaldrive"), ("components", L("组件"), "shippingbox"),
            ("access", L("外部访问"), "key.horizontal")
        ]
        if deviceSettings != nil { system.append(("console", L("设备"), "waveform")) }
        system += [("diagnostics", L("诊断"), "stethoscope"), ("about", L("关于"), "info.circle")]
        return [
            (L("常规"), [("appearance", L("外观"), "paintpalette"), ("shortcuts", L("快捷操作"), "keyboard")]),
            (L("采集"), [("capture", L("记录"), "record.circle"), ("scope", L("应用"), "square.grid.2x2")]),
            (L("智能处理"), [("ai-functions", L("AI 功能"), "sparkles"), ("people", L("人物"), "person.2")]),
            (L("系统"), system)
        ]
    }

    private var current: String {
        switch selection {
        case "models", "recognition": "ai-functions"
        default: groups.contains { $0.items.contains { $0.id == selection } } ? selection : "capture"
        }
    }

    var body: some View {
        HStack(alignment: .top, spacing: Space.xs) {
            sidebar
            detail
                .id(selection == "models" ? "models" : current)
                .scrollContentBackground(.hidden)
                .frame(maxWidth: 760)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.top, 58)
        .onReceive(NotificationCenter.default.publisher(for: QASnapshot.command)) { note in
            if let line = note.object as? String, line.hasPrefix("settings ") { selection = String(line.dropFirst(9)) }
        }
    }

    private var sidebar: some View {
        VStack(alignment: .leading, spacing: Space.m) {
            Text(L("设置")).typography(.section).accessibilityAddTraits(.isHeader)
                .padding(.horizontal, Space.s)
            ScrollView {
                VStack(alignment: .leading, spacing: Space.m) {
                    ForEach(groups, id: \.title) { group in
                        VStack(alignment: .leading, spacing: Space.xxs) {
                            Text(group.title).typography(.caption, .semibold).foregroundStyle(.secondary)
                                .accessibilityAddTraits(.isHeader).padding(.horizontal, Space.s)
                            ForEach(group.items, id: \.id) { item in
                                let selected = item.id == current
                                Button {
                                    withAnimation(reduceMotion ? nil : Motion.snappy) { selection = item.id }
                                } label: {
                                    HStack(spacing: Space.xs) {
                                        Image(systemName: item.symbol).frame(width: 18).accessibilityHidden(true)
                                        Text(item.title)
                                        Spacer(minLength: 0)
                                    }
                                    .typography(.body, selected ? .semibold : .regular)
                                    .foregroundStyle(selected ? Color.primary : Color.secondary)
                                    .padding(.horizontal, Space.s).frame(height: ControlHeight.button + 4)
                                    .background(selected ? Surface.selected.color : .clear, in: RoundedRectangle(cornerRadius: Radius.medium))
                                    .contentShape(Rectangle())
                                }
                                .buttonStyle(.plain)
                                .accessibilityAddTraits(selected ? .isSelected : [])
                            }
                        }
                    }
                }
                .padding(.bottom, Space.m)
            }
            .scrollIndicators(.hidden)
        }
        .frame(width: 166)
        .padding(.leading, Space.page)
    }

    @ViewBuilder private var detail: some View {
        switch current {
        case "shortcuts": HotkeySettingsView()
        case "ai-functions":
            AIFunctionSettingsView(recognition: session.recognition, initialTab: selection == "models" ? .sources : .features)
        case "components": ComponentsSettingsView()
        case "about": AboutSettingsView()
        case "people": SettingsPage(title: L("人物")) { VoiceLibraryView() }
        default: DesktopSettingsView(session: session, deviceSettings: deviceSettings, section: current)
        }
    }
}

/// Look of the whole window. Changes apply live to the background behind it.
struct AppearanceSettings: View {
    @AppStorage("console.appearance") private var appearance = "system"
    @AppStorage(Localization.languageKey) private var language = "system"
    @AppStorage(VisualStyle.key) private var styleRaw = ""
    @AppStorage(BackgroundStyleKey.mode) private var mode = "app"
    @AppStorage(BackgroundStyleKey.color) private var hex = "#5B6CFF"
    @AppStorage(BackgroundStyleKey.intensity) private var intensity = 1.0
    @AppStorage(BackgroundStyleKey.glass) private var glass = 0.55
    @AppStorage(BackgroundStyleKey.flow) private var flow = false
    @AppStorage(BackgroundStyleKey.styleTint) private var styleTint = ""
    @AppStorage(BackgroundStyleKey.frostTint) private var frostTint = 0.35

    private var style: VisualStyle { VisualStyle.stored(styleRaw, mode: mode) }
    private var styleBinding: Binding<VisualStyle> {
        Binding(get: { style }, set: { styleRaw = $0.rawValue; if mode == "plain" { mode = "app" } })
    }

    var body: some View {
        SettingsPage(title: L("外观")) {
            SettingRow(title: L("明暗")) {
                CapsuleChoice(options: [("system", L("跟随系统")), ("light", L("浅色")), ("dark", L("深色"))], selection: $appearance)
            }
            SettingRow(title: L("语言"), detail: language == Localization.language.rawValue || (language == "system") ? nil : L("重启 Candid 后生效。")) {
                CapsuleChoice(options: [("system", L("跟随系统")), ("zh", "\u{4E2D}\u{6587}"), ("en", "English")], selection: $language) // language names stay in their own language
            }
            VStack(alignment: .leading, spacing: Space.s) {
                Text(L("风格")).typography(.headline)
                HStack(spacing: Space.s) {
                    ForEach(VisualStyle.allCases) { item in
                        StyleTile(style: item, selected: item == style) { withAnimation(Motion.snappy) { styleBinding.wrappedValue = item } }
                    }
                }
            }
            VStack(alignment: .leading, spacing: 0) {
                switch style {
                case .ambient: ambientOptions
                case .frosted: tintRow; HairlineDivider(); sliderRow(L("浓度"), value: $frostTint, range: 0...1)
                case .flat: tintRow
                }
            }
            .id(style).transition(Motion.enter)
            QuietButton(title: L("恢复默认")) {
                styleRaw = VisualStyle.ambient.rawValue; mode = "app"; hex = "#5B6CFF"; intensity = 1.0; glass = 0.55
                flow = false; styleTint = ""; frostTint = 0.35
            }
            .offset(x: -9)
        }
        .animation(Motion.spring, value: style)
        .animation(Motion.snappy, value: mode)
    }

    @ViewBuilder private var ambientOptions: some View {
        SettingRow(title: L("颜色")) {
            HStack(spacing: Space.xs) {
                if mode == "solid" { ColorDot(hex: $hex) }
                CapsuleChoice(options: [("app", L("跟随应用")), ("aurora", L("极光")), ("solid", L("单色"))], selection: $mode)
            }
        }
        HairlineDivider()
        sliderRow(L("浓度"), value: $intensity, range: 0.3...1.8)
        HairlineDivider()
        sliderRow(L("毛玻璃"), value: $glass, range: 0...1)
        HairlineDivider()
        SettingRow(title: L("缓慢流动"), detail: L("背景持续重绘，会多耗一些电。")) {
            Toggle("", isOn: $flow).toggleStyle(.switch).labelsHidden().controlSize(.small)
        }
    }

    private var tintRow: some View {
        SettingRow(title: L("底色")) {
            HStack(spacing: Space.xs) {
                if !styleTint.isEmpty { ColorDot(hex: $styleTint) }
                CapsuleChoice(options: [(false, L("中性")), (true, L("自选"))],
                              selection: Binding(get: { !styleTint.isEmpty }, set: { styleTint = $0 ? hex : "" }))
            }
        }
    }

    private func sliderRow(_ title: String, value: Binding<Double>, range: ClosedRange<Double>) -> some View {
        SettingRow(title: title) { Slider(value: value, in: range).controlSize(.small).frame(width: 220) }
    }
}

/// A color well drawn as a dot; clicking opens the system color panel.
private struct ColorDot: View {
    @Binding var hex: String
    var body: some View {
        ColorPicker("", selection: Binding(get: { Color(hex: hex) ?? .accentColor }, set: { hex = $0.hexString }), supportsOpacity: false)
            .labelsHidden().fixedSize()
    }
}

/// A miniature of each style, drawn with the same pieces the window uses.
private struct StyleTile: View {
    let style: VisualStyle
    let selected: Bool
    let action: () -> Void
    @ViewState private var hovering = false

    var body: some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: Space.xs) {
                preview
                    .frame(height: 92)
                    .clipShape(RoundedRectangle(cornerRadius: Radius.small, style: .continuous))
                Text(style.title).typography(.body, selected ? .semibold : .medium)
                    .foregroundStyle(selected ? Color.primary : Color.secondary)
            }
            .padding(Space.xs)
            .panelSurface(radius: Radius.medium, level: selected ? .hover : hovering ? .fill : .wash)
            .selectionEdge(selected)
            .contentShape(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous))
        }
        .buttonStyle(.plain)
        .frame(maxWidth: 190)
        .onHover { hovering = $0 }
        .animation(Motion.snappy, value: hovering)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    @ViewBuilder private var preview: some View {
        ZStack(alignment: .topLeading) {
            switch style {
            case .ambient:
                ZStack {
                    Color(nsColor: .windowBackgroundColor)
                    RadialGradient(colors: [Color(red: 0.36, green: 0.42, blue: 1).opacity(0.45), .clear], center: .topLeading, startRadius: 0, endRadius: 120)
                    RadialGradient(colors: [Color(red: 0.86, green: 0.36, blue: 0.78).opacity(0.3), .clear], center: .bottomTrailing, startRadius: 0, endRadius: 110)
                }
            case .frosted:
                ZStack {
                    LinearGradient(colors: [Color(red: 0.55, green: 0.7, blue: 0.85), Color(red: 0.85, green: 0.75, blue: 0.6)], startPoint: .topLeading, endPoint: .bottomTrailing)
                    Rectangle().fill(.ultraThinMaterial)
                }
            case .flat:
                Palette.flatBackground(nil)
            }
            VStack(alignment: .leading, spacing: 5) {
                Capsule().fill(.tertiary).frame(width: 46, height: 6)
                VStack(alignment: .leading, spacing: 4) {
                    Capsule().fill(.quaternary).frame(width: 70, height: 4)
                    Capsule().fill(.quinary).frame(width: 52, height: 4)
                }
                .padding(8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .environment(\.visualStyle, style)
                .environment(\.insideRaisedPanel, false)
                .panelSurface(radius: 6)
            }
            .padding(10)
        }
    }
}
