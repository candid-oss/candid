import AIProviders
import DesktopCore
import DesktopPlatform
import SwiftUI

struct DesktopRecognitionView: View {
    let session: DesktopRecognitionSession
    let screenshot: DesktopRecord
    @ViewState private var results: [DesktopRecognitionKind: DesktopRecognitionResult] = [:]
    @ViewState private var loadError: String?
    @ViewState private var editingPrompt = false
    @ViewState private var draftPrompt = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: Space.xxs) {
                Group {
                    QuietButton(title: "模型分析", emphasized: true) { session.runManually(.vision, screenshot: screenshot) }
                        .help("用 设置 → 模型 里「图像理解」选择的模型分析这张截图")
                    QuietButton(title: "识别文字") { session.runManually(.ocr, screenshot: screenshot) }
                    QuietButton(title: "调整提示词") {
                        draftPrompt = session.prompt
                        withAnimation(Motion.spring) { editingPrompt.toggle() }
                    }
                }.disabled(session.activeKind != nil)
                if session.activeScreenshotID == screenshot.id {
                    ProgressView().controlSize(.small).accessibilityLabel("正在识别")
                    QuietButton(title: "取消", action: session.cancel)
                }
            }
            .offset(x: -9)
            if editingPrompt {
                InlinePanel {
                    PromptEditor(defaultText: PromptCatalog.defaultPrompt("vision"), presets: PromptCatalog.presets("vision"),
                                 text: $draftPrompt, minHeight: 150, maxHeight: 280,
                                 actions: [
                                    .init(title: "设为默认") { text in session.prompt = text },
                                    .init(title: "用这段分析", primary: true) { text in
                                        withAnimation(Motion.spring) { editingPrompt = false }
                                        session.runManually(.vision, screenshot: screenshot, promptOverride: text,
                                                            focusMainPane: text == DesktopVisionClient.chatPrompt)
                                    }
                                 ],
                                 onCancel: { withAnimation(Motion.spring) { editingPrompt = false } })
                }
            }
            if let feedback = session.manualFeedback, feedback.id == screenshot.id {
                message(feedback.message)
            }
            if let loadError { message(loadError) }
            if let analysis = results[.vision] {
                result(analysis, title: "窗口内容与布局", empty: "")
                if let ocr = results[.ocr] {
                    FoldRow(title: "查看原始 OCR") { result(ocr, title: "识别文字", empty: "没有识别到文字。") }
                }
                if let markdown = analysis.windowMarkdown {
                    FoldRow(title: "查看 Markdown 原文") {
                        Text(verbatim: markdown.body).typography(.caption, .regular).monospaced().textSelection(.enabled)
                    }
                    FoldRow(title: "查看提取数据") { DesktopRecognitionJSONView(value: markdown) }
                } else if let layout = analysis.windowLayout {
                    FoldRow(title: "查看 Markdown 原文") {
                        Text(verbatim: layout.markdown).typography(.caption, .regular).monospaced().textSelection(.enabled)
                    }
                    FoldRow(title: "查看结构化数据") { DesktopRecognitionJSONView(value: layout) }
                }
            } else if let ocr = results[.ocr] {
                result(ocr, title: "识别文字", empty: "没有识别到文字。")
            }
        }
        .typography(.secondary)
        .frame(maxWidth: .infinity, alignment: .leading)
        .task(id: session.resultRevision) {
            do {
                let saved = try await session.results(for: screenshot)
                try Task.checkCancellation()
                results = saved
                loadError = nil
            } catch {
                if !Task.isCancelled { loadError = error.localizedDescription }
            }
        }
    }

    private func message(_ text: String) -> some View {
        InlineMessage(kind: .note, text: text)
    }

    private func result(_ result: DesktopRecognitionResult, title: String, empty: String) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack {
                Text(title).fontWeight(.semibold)
                Text(result.source).typography(.caption).foregroundStyle(.secondary)
                if result.usedOCRReference { Text("参考了 OCR").typography(.caption).foregroundStyle(.secondary) }
            }
            if let markdown = result.windowMarkdown {
                if let missing = markdown.unplacedMainOCR, let total = markdown.mainOCRLineCount, total > 0 {
                    Text("主视图匹配 OCR \(total - missing.count) / \(total) 行")
                        .typography(.caption).foregroundStyle(.secondary)
                    if !missing.isEmpty {
                        FoldRow(title: "核对主视图未匹配文字（\(missing.count) 行）") {
                            Text(verbatim: missing.joined(separator: "\n")).textSelection(.enabled)
                        }
                    }
                } else if markdown.inputOCRLineCount > 0 {
                    Text("结果匹配 OCR \(markdown.matchedOCRLineCount) / \(markdown.inputOCRLineCount) 行")
                        .typography(.caption).foregroundStyle(.secondary)
                }
                DesktopMarkdownResultView(markdown: markdown.body)
                if !markdown.unplacedOCR.isEmpty {
                    FoldRow(title: "核对未匹配的 OCR（\(markdown.unplacedOCR.count) 行）") {
                        Text(verbatim: markdown.unplacedOCR.joined(separator: "\n")).textSelection(.enabled)
                    }
                }
            } else if let layout = result.windowLayout {
                DesktopLayoutMarkdownView(layout: layout)
                if !layout.excludedText.isEmpty {
                    FoldRow(title: "查看已过滤的界面文字") {
                        Text(verbatim: layout.excludedText.joined(separator: "\n"))
                            .textSelection(.enabled).foregroundStyle(.secondary)
                    }
                }
            } else {
                Text(verbatim: result.text.isEmpty ? empty : result.text)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

// Render the supported Markdown blocks from their typed source. OCR and
// model strings remain literal, including any HTML or link-like text.
private struct DesktopLayoutMarkdownView: View {
    let layout: DesktopWindowLayout

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            if let focus = layout.focusLabel {
                Label("窗口焦点 · \(focus)", systemImage: "scope").fontWeight(.medium)
            }
            ForEach(Array(layout.regions.enumerated()), id: \.offset) { _, region in
                VStack(alignment: .leading, spacing: 5) {
                    Text(verbatim: region.name).typography(.headline)
                    if !region.visibleState.isEmpty {
                        Text(verbatim: region.visibleState).foregroundStyle(.secondary)
                    }
                    if let rows = region.tableRows {
                        Grid(alignment: .leading, horizontalSpacing: 14, verticalSpacing: 6) {
                            ForEach(Array(rows.enumerated()), id: \.offset) { index, row in
                                if index > 0 { HairlineDivider() }
                                GridRow {
                                    ForEach(Array(row.enumerated()), id: \.offset) { _, cell in
                                        Text(verbatim: cell).frame(maxWidth: .infinity, alignment: .leading)
                                    }
                                }
                            }
                        }.padding(8).background(.background.secondary, in: RoundedRectangle(cornerRadius: Radius.small))
                    } else {
                        bullets(region.text)
                    }
                    if !region.visualSupplement.isEmpty {
                        Text("画面补充 · \(region.visualSupplement)").typography(.caption).foregroundStyle(.secondary)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            if !layout.unclassifiedText.isEmpty {
                VStack(alignment: .leading, spacing: 5) {
                    Text("未归类文字").typography(.headline)
                    bullets(layout.unclassifiedText)
                }
            }
        }
        .textSelection(.enabled)
        .fixedSize(horizontal: false, vertical: true)
    }

    private func bullets(_ lines: [String]) -> some View {
        ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text("•").foregroundStyle(.secondary).accessibilityHidden(true)
                Text(verbatim: line)
            }
        }
    }
}

private struct DesktopRecognitionJSONView<Value: Encodable>: View {
    let value: Value

    var body: some View {
        let encoder = JSONEncoder()
        let _ = encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        if let data = try? encoder.encode(value) {
            Text(verbatim: String(decoding: data, as: UTF8.self))
                .typography(.caption, .regular).monospaced()
                .textSelection(.enabled)
        }
    }
}

struct DesktopVisionSettings: View {
    @Bindable var session: DesktopRecognitionSession
    let isDemo: Bool
    @ViewState private var selectedKind: DesktopRecognitionKind = .ocr
    @AppStorage("desktop.settings.tab") private var settingsTab = "capture"

    private func roleSummary(_ role: String) -> String {
        let config = AIRuntime.shared.config()
        guard let a = config.assignment(role), a.model != nil || a.engine != nil else {
            return role == "ocr" ? "系统文字识别（默认）" : "未选择模型"
        }
        if let ref = a.model {
            return "\(config.provider(ref.providerID)?.name ?? "?") · \(config.model(ref)?.displayName ?? ref.modelID)"
        }
        return ModelRoleInfo.info(role)?.engines.first { $0.id == a.engine }?.title ?? "内置引擎"
    }

    var body: some View {
        SettingsPage(title: "识别") {
            PillSwitcher(items: DesktopRecognitionKind.allCases.map { ($0, $0.title) }, selection: $selectedKind).offset(x: -13)
            timingGroup(selectedKind)
            powerGroup(selectedKind)
            SettingsGroup(title: "模型与提示词") {
                SettingRow(title: selectedKind == .vision ? "图像理解" : "文字识别", detail: "模型、密钥和提示词统一在「模型」里管理。") {
                    HStack(spacing: Space.xs) {
                        Text(roleSummary(selectedKind == .vision ? "vision" : "ocr")).typography(.body).foregroundStyle(.secondary)
                        QuietButton(title: "打开", emphasized: true) { settingsTab = "models" }
                    }
                }
            }
            if selectedKind == .vision {
                SettingsGroup(title: "描述方式") {
                    SwitchRow(title: "参考这张图已有的 OCR 文字", detail: "有 OCR 结果时随图提供，模型仍需与原图核对；没有时直接看图。",
                              isOn: $session.useOCRReference)
                    HairlineDivider()
                    SettingRow(title: "输出上限") {
                        StepperControl(text: "\(session.maximumTokens) token", value: $session.maximumTokens, range: 256...8_192, step: 256)
                    }
                }
            }
        }
        .animation(Motion.spring, value: selectedKind)
        .onAppear(perform: session.refreshPower)
    }

    @ViewBuilder private func timingGroup(_ kind: DesktopRecognitionKind) -> some View {
        let policy = session.policy(kind)
        SettingsGroup(title: "处理时机") {
            SettingRow(title: "模式", detail: kind == .vision && policy.mode != .manual ? "自动分析会把符合范围的截图交给配置的模型。" : nil) {
                CapsuleChoice(options: DesktopRecognitionMode.allCases.map { ($0, $0.title) }, selection: binding(\.mode, for: kind))
            }
            if policy.mode != .manual {
                HairlineDivider()
                SwitchRow(title: "也处理已有截图", isOn: binding(\.includeHistory, for: kind))
            }
            if policy.mode == .batch {
                HairlineDivider()
                SettingRow(title: "间隔") {
                    StepperControl(text: "每 \(policy.batchIntervalMinutes) 分钟", value: binding(\.batchIntervalMinutes, for: kind), range: 1...720)
                }
                HairlineDivider()
                SettingRow(title: "每批最多") {
                    StepperControl(text: "\(policy.batchLimit) 张", value: binding(\.batchLimit, for: kind), range: 1...100)
                }
                QuietButton(title: "现在运行一批", emphasized: true) { session.runBatchNow(kind) }
                    .disabled(isDemo || session.activeKind != nil).offset(x: -9)
            }
            if let status = session.status[kind], policy.mode != .manual || session.activeKind == kind {
                InlineMessage(kind: .note, text: status).padding(.vertical, Space.xs)
            }
            if isDemo { InlineMessage(kind: .note, text: "合成演示只运行手动识别。").padding(.vertical, Space.xs) }
        }
    }

    @ViewBuilder private func powerGroup(_ kind: DesktopRecognitionKind) -> some View {
        let policy = session.policy(kind)
        SettingsGroup(title: "省电条件") {
            SwitchRow(title: "仅接入外部电源时处理", detail: "这些条件也适用于手动识别。", isOn: binding(\.requireExternalPower, for: kind))
            if policy.requireExternalPower {
                HairlineDivider()
                SettingRow(title: "插电后等待") {
                    StepperControl(text: policy.pluggedMinutes == 0 ? "不等待" : "\(policy.pluggedMinutes) 分钟",
                                   value: binding(\.pluggedMinutes, for: kind), range: 0...120)
                }
            }
            HairlineDivider()
            SettingRow(title: "最低电量", detail: "没有内置电池的 Mac 跳过这项。") {
                StepperControl(text: policy.minimumBatteryPercent == 0 ? "不限" : "\(policy.minimumBatteryPercent)%",
                               value: binding(\.minimumBatteryPercent, for: kind), range: 0...100, step: 5)
            }
            HairlineDivider()
            SwitchRow(title: "低电量模式时暂停", isOn: binding(\.pauseInLowPower, for: kind))
            if let block = session.powerBlocks[kind] { InlineMessage(kind: .note, text: block).padding(.vertical, Space.xs) }
        }
    }

    private func binding<Value>(_ path: WritableKeyPath<DesktopRecognitionPolicy, Value>, for kind: DesktopRecognitionKind) -> Binding<Value> {
        Binding(get: { session.policy(kind)[keyPath: path] }, set: { value in
            var policy = session.policy(kind)
            policy[keyPath: path] = value
            session.setPolicy(policy, for: kind)
        })
    }
}
