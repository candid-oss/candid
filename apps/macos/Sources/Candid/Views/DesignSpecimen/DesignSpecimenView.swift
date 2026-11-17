import AIProviders
import SwiftUI

/// A living specimen of the design system: every token and shared component, rendered by the real code.
/// Development reference only; reachable with `--design-specimen` or `page specimen` in the self-test copy.
struct DesignSpecimenView: View {
    /// Self-test only: `specimen <section>` scrolls to a section, `style <ambient|frosted|flat>` switches the style.
    @AppStorage(VisualStyle.key) private var styleRaw = ""
    @AppStorage(BackgroundStyleKey.mode) private var mode = "app"

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: Space.xl) {
                    HStack(alignment: .firstTextBaseline) {
                        Text("设计样张").typography(.title)
                        Spacer()
                        CapsuleChoice(options: VisualStyle.allCases.map { ($0, $0.title) },
                                      selection: Binding(get: { VisualStyle.stored(styleRaw, mode: mode) }, set: { styleRaw = $0.rawValue }))
                    }
                    .id("top")
                    HStack(alignment: .top, spacing: Space.xxl) {
                        VStack(alignment: .leading, spacing: Space.xl) {
                            SpecimenTypeSection().id("type")
                            SpecimenColorSection().id("color")
                            SpecimenSurfaceSection().id("surface")
                            SpecimenSpacingSection().id("space")
                            SpecimenMotionSection().id("motion")
                        }
                        .frame(width: 430, alignment: .leading)
                        VStack(alignment: .leading, spacing: Space.xl) {
                            SpecimenButtonSection().id("buttons")
                            SpecimenChoiceSection().id("choices")
                            SpecimenTagSection().id("tags")
                            SpecimenFieldSection().id("fields")
                            SpecimenRowSection().id("rows")
                            SpecimenFeedbackSection().id("feedback")
                            SpecimenCaptureSection().id("capture")
                        }
                        .frame(maxWidth: 560, alignment: .leading)
                    }
                }
                .frame(maxWidth: 1080, alignment: .leading)
                .padding(.horizontal, Space.page).padding(.top, 84).padding(.bottom, Space.xxl)
                .frame(maxWidth: .infinity)
            }
            .scrollIndicators(.hidden)
            .fadesUnderChrome()
            .onReceive(NotificationCenter.default.publisher(for: QASnapshot.command)) { note in
                guard let line = note.object as? String else { return }
                if line.hasPrefix("specimen ") { proxy.scrollTo(String(line.dropFirst(9)), anchor: .top) }
                if line.hasPrefix("style ") { styleRaw = String(line.dropFirst(6)) }
            }
        }
    }
}

/// One section of the specimen: the rule in one line, then the real thing.
private struct SpecimenSection<Content: View>: View {
    let title: String
    let rule: String
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: Space.m) {
            SectionHeader(title: title, note: rule)
            content
        }
    }
}

/// Token name and value, right-aligned beside a sample.
private struct TokenLabel: View {
    let text: String
    var body: some View { Text(text).typography(.caption, .regular).monospaced().foregroundStyle(.tertiary) }
}

// MARK: - Foundations

private struct SpecimenTypeSection: View {
    private func sample(_ scale: TypeScale) -> String {
        switch scale {
        case .display: "15:42"
        case .title: "设置"
        case .section: "图像理解"
        case .headline: "LM Studio · gemma-4-e4b"
        case .body: "截图整理成 Markdown，并按截图回答问题。"
        case .secondary: "来源没有说明这个模型能否看图。"
        case .caption: "已修改 · 3 分钟前"
        }
    }

    private func name(_ scale: TypeScale) -> String {
        switch scale {
        case .display: "display"
        case .title: "title"
        case .section: "section"
        case .headline: "headline"
        case .body: "body"
        case .secondary: "secondary"
        case .caption: "caption"
        }
    }

    var body: some View {
        SpecimenSection(title: "文字", rule: "七级，只用整数字号。每页一个 title；数字变化处用等宽数字。") {
            VStack(alignment: .leading, spacing: Space.s) {
                ForEach(TypeScale.allCases, id: \.self) { scale in
                    HStack(alignment: .firstTextBaseline) {
                        Text(sample(scale)).typography(scale, digits: scale == .display).lineLimit(1)
                        Spacer(minLength: Space.s)
                        TokenLabel(text: "\(name(scale)) \(Int(scale.size))")
                    }
                }
            }
        }
    }
}

private struct SpecimenColorSection: View {
    var body: some View {
        SpecimenSection(title: "颜色", rule: "颜色只表达含义：交互、去向、状态、AI。装饰交给背景和应用色。") {
            VStack(alignment: .leading, spacing: Space.l) {
                VStack(alignment: .leading, spacing: Space.s) {
                    Text("交互色：跟随系统强调色").typography(.secondary, .semibold).foregroundStyle(.secondary)
                    AccentSample(color: Palette.accent)
                }
                VStack(alignment: .leading, spacing: Space.s) {
                    Text("状态与去向").typography(.secondary, .semibold).foregroundStyle(.secondary)
                    HStack(spacing: Space.m) {
                        Swatch(color: Palette.success, name: "完成")
                        Swatch(color: Palette.warning, name: "警告")
                        Swatch(color: Palette.danger, name: "错误")
                        Swatch(color: Palette.recording, name: "录制中")
                    }
                    HStack(spacing: Space.m) {
                        ForEach([Destination.onDevice, .selfHosted, .cloud], id: \.self) { Swatch(color: $0.tint, name: $0.label) }
                    }
                }
                VStack(alignment: .leading, spacing: Space.s) {
                    Text("AI 与应用").typography(.secondary, .semibold).foregroundStyle(.secondary)
                    HStack(spacing: Space.s) {
                        Capsule().fill(Palette.ai).frame(width: 120, height: 10)
                        Text("只标记 AI 生成的内容和 AI 页").typography(.secondary).foregroundStyle(.secondary)
                    }
                    HStack(spacing: Space.m) {
                        ForEach([("com.apple.Safari", "Safari"), ("com.apple.Notes", "备忘录"), ("com.apple.mail", "邮件"),
                                 ("com.apple.finder", "访达")], id: \.0) { id, name in
                            Swatch(color: AppTint.color(for: id), name: name)
                        }
                    }
                }
            }
        }
    }
}

/// The interaction color on a primary button, an "edited" tag and a switch.
private struct AccentSample: View {
    let color: Color
    var body: some View {
        VStack(alignment: .leading, spacing: Space.xs) {
            HStack(spacing: Space.xs) {
                Text("保存").typography(.body, .semibold).foregroundStyle(.white)
                    .padding(.horizontal, 14).frame(height: ControlHeight.button).background(Capsule().fill(color))
                Text("已修改").typography(.caption, .semibold).foregroundStyle(color)
                    .padding(.horizontal, 7).padding(.vertical, 2).background(Capsule().fill(color.opacity(0.13)))
                Toggle("", isOn: .constant(true)).toggleStyle(.switch).labelsHidden().controlSize(.mini).tint(color)
            }
        }
    }
}

private struct Swatch: View {
    let color: Color
    let name: String
    var body: some View {
        HStack(spacing: 6) {
            Circle().fill(color).frame(width: 12, height: 12)
            Text(name).typography(.secondary).foregroundStyle(.secondary)
        }
    }
}

private struct SpecimenSurfaceSection: View {
    @ViewState private var hovering = false

    var body: some View {
        SpecimenSection(title: "表面与圆角", rule: "层次靠底色深浅，不靠边框。窗口毛玻璃之上最多再叠一层；只有选中和聚焦才描边。") {
            VStack(alignment: .leading, spacing: Space.l) {
                HStack(spacing: Space.s) {
                    ForEach(Surface.allCases, id: \.self) { level in
                        VStack(alignment: .leading, spacing: 6) {
                            RoundedRectangle(cornerRadius: Radius.medium, style: .continuous).fill(level.color).frame(width: 92, height: 56)
                            TokenLabel(text: "\(level) \(String(format: "%.3g", level.opacity))")
                        }
                    }
                }
                HStack(alignment: .top, spacing: Space.l) {
                    VStack(alignment: .leading, spacing: 6) {
                        HStack(spacing: Space.s) {
                            Image(systemName: "doc.text").foregroundStyle(.secondary)
                            VStack(alignment: .leading, spacing: 2) {
                                Text("14:20 · Notion").typography(.body, .semibold)
                                Text("整理周报，补了三条待办").typography(.secondary).foregroundStyle(.secondary)
                            }
                        }
                        .padding(Space.s).floatingSurface()
                        TokenLabel(text: "floating · material")
                    }
                    VStack(alignment: .leading, spacing: 6) {
                        Text("选中的选项").typography(.body, .medium).padding(.horizontal, Space.s).frame(height: 40)
                            .background(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous).fill(Surface.hover.color))
                            .selectionEdge(true)
                        TokenLabel(text: "selection edge")
                    }
                }
                HStack(alignment: .bottom, spacing: Space.m) {
                    ForEach(Radius.all, id: \.0) { name, r in
                        VStack(alignment: .leading, spacing: 6) {
                            RoundedRectangle(cornerRadius: r, style: .continuous).fill(Surface.selected.color).frame(width: 64, height: 48)
                            TokenLabel(text: "\(name) \(Int(r))")
                        }
                    }
                    VStack(alignment: .leading, spacing: 6) {
                        Capsule().fill(Surface.selected.color).frame(width: 64, height: 26)
                        TokenLabel(text: "capsule")
                    }
                }
                VStack(alignment: .leading, spacing: 6) {
                    HairlineDivider().frame(width: 200)
                    TokenLabel(text: "hairline 0.5 · 只分隔行")
                }
            }
        }
    }
}

private struct SpecimenSpacingSection: View {
    var body: some View {
        SpecimenSection(title: "间距", rule: "4 的倍数。行内 8，行与行 12，组与组 24，页面左右 28。") {
            VStack(alignment: .leading, spacing: 6) {
                ForEach(Space.all, id: \.0) { name, value in
                    HStack(spacing: Space.s) {
                        Capsule().fill(Palette.accent.opacity(0.35)).frame(width: value, height: 10)
                        TokenLabel(text: "\(name) \(Int(value))")
                    }
                }
            }
        }
    }
}

private struct SpecimenMotionSection: View {
    @ViewState private var flipped = false

    var body: some View {
        SpecimenSection(title: "动效", rule: "全部用可打断的弹簧；进场是淡入加上移 6pt；只有跟随实时信号的才匀速。") {
            VStack(alignment: .leading, spacing: Space.s) {
                ForEach(Motion.all, id: \.0) { name, animation, use in
                    HStack(spacing: Space.s) {
                        ZStack(alignment: .leading) {
                            Capsule().fill(Surface.fill.color).frame(width: 160, height: 6)
                            Circle().fill(Palette.accent).frame(width: 14, height: 14)
                                .offset(x: flipped ? 146 : 0)
                                .animation(animation, value: flipped)
                        }
                        TokenLabel(text: name)
                        Text(use).typography(.secondary).foregroundStyle(.secondary)
                    }
                }
                QuietButton(title: "播放") { flipped.toggle() }.offset(x: -9)
            }
        }
    }
}

// MARK: - Components

private struct SpecimenButtonSection: View {
    var body: some View {
        SpecimenSection(title: "按钮", rule: "一个区域最多一个实心按钮。其余是轻按钮，悬停才出底色。删除在原地二次确认，不弹系统对话框。") {
            VStack(alignment: .leading, spacing: Space.m) {
                HStack(spacing: 6) {
                    PrimaryButton(title: "保存") {}
                    QuietButton(title: "取消") {}
                    QuietButton(title: "恢复默认") {}.disabled(true).opacity(0.4)
                    Spacer()
                    ConfirmButton(title: "删除来源", confirmTitle: "确认删除") {}
                }
                HStack(spacing: Space.xs) {
                    ChipButton(title: "问 AI", systemImage: "sparkle") {}
                    ChipButton(title: "回顾今天", emphasized: true) {}
                    RunButton(title: "会前要点", runningTitle: "正在整理会前要点…", running: true, start: {}, cancel: {})
                    Text("小操作用浅底胶囊，耗时的 AI 操作点后变成进度和取消").typography(.secondary).foregroundStyle(.secondary)
                }
                HStack(spacing: Space.xs) {
                    ChromeButton(systemImage: "gearshape", help: "设置") {}
                    ChromeButton(systemImage: "camera", help: "截图", active: true) {}
                    Text("顶栏圆形按钮：选中时有底色").typography(.secondary).foregroundStyle(.secondary)
                }
            }
        }
    }
}

private struct SpecimenChoiceSection: View {
    @ViewState private var tab = "roles"
    @ViewState private var place = Destination.onDevice
    @ViewState private var on = true
    @ViewState private var model = "gemma-4-e4b"

    var body: some View {
        SpecimenSection(title: "选择", rule: "切换视图用滑块标签；五项以内的设置用胶囊，更多用菜单；开关一律用 switch，不用复选框。") {
            VStack(alignment: .leading, spacing: Space.m) {
                PillSwitcher(items: [("sources", "来源"), ("roles", "角色"), ("usage", "用量")], selection: $tab).offset(x: -13)
                CapsuleChoice(options: [(Destination.onDevice, "本机"), (.selfHosted, "自建"), (.cloud, "云端")], selection: $place)
                HStack(spacing: Space.l) {
                    Toggle("记录屏幕文字", isOn: $on).toggleStyle(.switch).controlSize(.small).typography(.body)
                    MenuButton(title: model) {
                        ForEach(["系统文字识别", "gemma-4-e4b", "qwen3-vl-8b"], id: \.self) { m in Button(m) { model = m } }
                    }
                }
            }
        }
    }
}

private struct SpecimenTagSection: View {
    var body: some View {
        SpecimenSection(title: "标签与状态", rule: "固定词表：同一种状态永远同一个样子。灰色陈述事实，有色表达含义。") {
            VStack(alignment: .leading, spacing: Space.s) {
                HStack(spacing: 6) {
                    StateTag(isDefault: true)
                    StateTag(isDefault: false)
                    Tag(text: "尚未使用")
                    ForEach([Destination.onDevice, .selfHosted, .cloud], id: \.self) { DestinationTag(destination: $0) }
                    Tag(text: "规划中", tint: .purple)
                }
                HStack(spacing: Space.m) {
                    StatusDot(state: .on, text: "已连接")
                    StatusDot(state: .off, text: "未开启")
                    StatusDot(state: .attention, text: "需要授权")
                    StatusDot(state: .planned, text: "规划中")
                }
                HStack(spacing: Space.s) {
                    SourceBadge(title: "LM", destination: .onDevice)
                    IconChipButton(title: "测试来源", symbol: "arrow.clockwise") {}
                    ProportionMeter(segments: [(0.6, Palette.accent)]).frame(width: 100)
                    ActivityBars(values: [1, 3, 0, 4, 2, 6, 3]).frame(width: 100)
                }
            }
        }
    }
}

private struct SpecimenFieldSection: View {
    @ViewState private var url = "http://localhost:1234/v1"
    @ViewState private var key = "synthetic-key"
    @ViewState private var prompt = PromptCatalog.defaultPrompt("summary")

    var body: some View {
        SpecimenSection(title: "输入", rule: "字段是淡底，没有边框；标题在上方。地址和代码用等宽字。长文本在原地展开编辑。") {
            VStack(alignment: .leading, spacing: Space.m) {
                HStack(spacing: Space.s) {
                    QuietField(title: "地址", text: $url, mono: true)
                    QuietField(title: "密钥", text: $key, secure: true)
                }
                InlinePanel {
                    PromptEditor(defaultText: PromptCatalog.defaultPrompt("summary"), presets: PromptCatalog.presets("summary"),
                                 text: $prompt, minHeight: 84, maxHeight: 120,
                                 actions: [.init(title: "保存", primary: true) { _ in }], onCancel: {})
                }
            }
        }
    }
}

private struct SpecimenRowSection: View {
    @ViewState private var preset = "lmstudio"

    var body: some View {
        SpecimenSection(title: "行与面板", rule: "列表是行加细线，不是卡片。点开的内容在行下原地展开成淡底面板。") {
            VStack(alignment: .leading, spacing: 0) {
                SpecimenRow(title: "图像理解", detail: "截图整理成 Markdown；需要图片输入", destination: .onDevice, value: "LM Studio · gemma-4-e4b")
                HairlineDivider()
                SpecimenRow(title: "摘要", detail: "片段与每日回顾的文字整理", destination: .cloud, value: "OpenRouter · claude-sonnet", unused: true)
                HairlineDivider()
                InlinePanel {
                    LazyVGrid(columns: [GridItem(.flexible(), spacing: 10), GridItem(.flexible(), spacing: 10), GridItem(.flexible())], spacing: 10) {
                        ForEach(ProviderPreset.all.prefix(3)) { p in
                            PresetTile(preset: p, selected: p.id == preset) { withAnimation(Motion.snappy) { preset = p.id } }
                        }
                    }
                }
                .padding(.top, Space.s)
            }
        }
    }
}

private struct SpecimenRow: View {
    let title: String
    let detail: String
    let destination: Destination
    let value: String
    var unused = false

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 14) {
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: Space.xs) {
                    Text(title).typography(.headline)
                    if unused { Tag(text: "尚未使用") }
                }
                Text(detail).typography(.secondary).foregroundStyle(.secondary)
            }
            Spacer()
            DestinationTag(destination: destination)
            Text(value).typography(.body, .medium).lineLimit(1)
        }
        .padding(.vertical, Space.row)
    }
}

private struct SpecimenFeedbackSection: View {
    var body: some View {
        SpecimenSection(title: "反馈与空状态", rule: "问题出在哪就写在哪；别处发生的事用顶部浮动提示。空状态说清缺什么，并直接给出下一步。") {
            VStack(alignment: .leading, spacing: Space.m) {
                VStack(alignment: .leading, spacing: 6) {
                    InlineMessage(kind: .error, text: "连接失败：服务返回 401，需要访问令牌。")
                    InlineMessage(kind: .warning, text: "这个模型没有声明工具调用，对话会退化成单次回答。")
                    InlineMessage(kind: .note, text: "来源没有说明这个模型能否看图。")
                }
                NoticeCapsule(text: "截图已保存到 14:20 的片段", onClose: {})
                VStack(alignment: .leading, spacing: 6) {
                    Text("还没有模型来源").typography(.headline)
                    Text("接入本机的 LM Studio、Ollama，或云端接口。密钥只保存在钥匙串里。").typography(.secondary).foregroundStyle(.secondary)
                    QuietButton(title: "＋ 添加来源") {}.offset(x: -9)
                }
            }
        }
    }
}

private struct SpecimenCaptureSection: View {
    private let levels: [Float] = (0..<36).map { i in Float(0.12 + 0.3 * abs(sin(Double(i) * 0.55)) * (0.6 + 0.4 * cos(Double(i) * 0.21))) }

    var body: some View {
        SpecimenSection(title: "采集反馈", rule: "录制时一定有可见的动态反馈；红色只留给正在录制。") {
            HStack(spacing: Space.l) {
                RecordingOrb(level: 0.3).frame(width: 34, height: 34)
                LiveWaveform(levels: levels, color: Palette.recording).frame(width: 180, height: 28)
                Text("00:42").typography(.body, .medium, digits: true)
            }
        }
    }
}
