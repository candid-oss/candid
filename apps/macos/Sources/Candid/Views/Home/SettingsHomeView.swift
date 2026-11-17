import SwiftUI

/// Settings inside the immersive window: a quiet section list on the left,
/// one section on the right. No box around either; the page floats on the
/// same background as everything else.
struct SettingsHomeView: View {
    @Bindable var session: DesktopSession
    var deviceSettings: AnyView?
    @AppStorage("desktop.settings.tab") private var selection = "capture"
    @Namespace private var marker

    private var sections: [(id: String, title: String, symbol: String)] {
        var items: [(String, String, String)] = [
            ("capture", "记录", "record.circle"), ("scope", "应用", "square.grid.2x2"),
            ("appearance", "外观", "paintpalette"), ("storage", "数据", "internaldrive"),
            ("recognition", "识别", "text.viewfinder"), ("shortcuts", "快捷操作", "keyboard"),
            ("models", "模型", "sparkles"), ("ai-functions", "AI 功能", "wand.and.stars"),
            ("components", "组件", "shippingbox"),
            ("people", "人物", "person.2"), ("access", "外部访问", "key.horizontal")
        ]
        if deviceSettings != nil { items.append(("console", "设备", "waveform")) }
        items.append(("diagnostics", "诊断", "stethoscope"))
        items.append(("about", "关于", "info.circle"))
        return items.map { (id: $0.0, title: $0.1, symbol: $0.2) }
    }

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            VStack(alignment: .leading, spacing: 2) {
                Text("设置").typography(.title).padding(.horizontal, 12).padding(.bottom, 18)
                ForEach(sections, id: \.id) { item in
                    let on = item.id == current
                    Button { withAnimation(Motion.bouncy) { selection = item.id } } label: {
                        HStack(spacing: 10) {
                            Image(systemName: item.symbol).typography(.headline, .medium)
                                .foregroundStyle(on ? Color.accentColor : Color.secondary).frame(width: 20)
                            Text(item.title).typography(.headline, on ? .semibold : .regular)
                                .foregroundStyle(on ? Color.primary : Color.secondary)
                            Spacer()
                        }
                        .padding(.horizontal, 12).frame(height: 34)
                        .background {
                            if on {
                                RoundedRectangle(cornerRadius: Radius.medium, style: .continuous).fill(Surface.hover.color)
                                    .matchedGeometryEffect(id: "selection", in: marker)
                            }
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(on ? .isSelected : [])
                }
                Spacer()
            }
            .frame(width: 200)
            .padding(.top, 70).padding(.leading, 28)

            Group {
                if current == "shortcuts" { HotkeySettingsView() }
                else if current == "models" { ModelSettingsView() }
                else if current == "ai-functions" { AIFunctionSettingsView() }
                else if current == "components" { ComponentsSettingsView() }
                else if current == "about" { AboutSettingsView() }
                else if current == "people" { SettingsPage(title: "人物") { VoiceLibraryView() } }
                else { DesktopSettingsView(session: session, deviceSettings: deviceSettings, section: current) }
            }
                .id(current)
                .transition(.opacity.combined(with: .offset(y: 8)))
                .scrollContentBackground(.hidden)
                .frame(maxWidth: 760)
                .padding(.top, 58)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .animation(Motion.spring, value: current)
        .onReceive(NotificationCenter.default.publisher(for: QASnapshot.command)) { note in
            // Self-test only: `settings <section>`.
            if let line = note.object as? String, line.hasPrefix("settings ") { selection = String(line.dropFirst(9)) }
        }
    }

    /// A remembered device section disappears when no device store exists.
    private var current: String { sections.contains { $0.id == selection } ? selection : "capture" }
}

/// Look of the whole window. Changes apply live to the background behind it.
struct AppearanceSettings: View {
    @AppStorage("console.appearance") private var appearance = "system"
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
        SettingsPage(title: "外观") {
            SettingRow(title: "明暗") {
                CapsuleChoice(options: [("system", "跟随系统"), ("light", "浅色"), ("dark", "深色")], selection: $appearance)
            }
            VStack(alignment: .leading, spacing: Space.s) {
                Text("风格").typography(.headline)
                HStack(spacing: Space.s) {
                    ForEach(VisualStyle.allCases) { item in
                        StyleTile(style: item, selected: item == style) { withAnimation(Motion.snappy) { styleBinding.wrappedValue = item } }
                    }
                }
            }
            VStack(alignment: .leading, spacing: 0) {
                switch style {
                case .ambient: ambientOptions
                case .frosted: tintRow; HairlineDivider(); sliderRow("浓度", value: $frostTint, range: 0...1)
                case .flat: tintRow
                }
            }
            .id(style).transition(Motion.enter)
            QuietButton(title: "恢复默认") {
                styleRaw = VisualStyle.ambient.rawValue; mode = "app"; hex = "#5B6CFF"; intensity = 1.0; glass = 0.55
                flow = false; styleTint = ""; frostTint = 0.35
            }
            .offset(x: -9)
        }
        .animation(Motion.spring, value: style)
        .animation(Motion.snappy, value: mode)
    }

    @ViewBuilder private var ambientOptions: some View {
        SettingRow(title: "颜色") {
            HStack(spacing: Space.xs) {
                if mode == "solid" { ColorDot(hex: $hex) }
                CapsuleChoice(options: [("app", "跟随应用"), ("aurora", "极光"), ("solid", "单色")], selection: $mode)
            }
        }
        HairlineDivider()
        sliderRow("浓度", value: $intensity, range: 0.3...1.8)
        HairlineDivider()
        sliderRow("毛玻璃", value: $glass, range: 0...1)
        HairlineDivider()
        SettingRow(title: "缓慢流动", detail: "背景持续重绘，会多耗一些电。") {
            Toggle("", isOn: $flow).toggleStyle(.switch).labelsHidden().controlSize(.small)
        }
    }

    private var tintRow: some View {
        SettingRow(title: "底色") {
            HStack(spacing: Space.xs) {
                if !styleTint.isEmpty { ColorDot(hex: $styleTint) }
                CapsuleChoice(options: [(false, "中性"), (true, "自选")],
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
