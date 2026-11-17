import AppKit
import SwiftUI
import CandidClient

/// 设置 → 外部访问: which AI tools may read the timeline, how to connect them, and what they did recently.
struct TimelineAccessView: View {
    private var service: TimelineService { .shared }
    @ViewState private var adding = false
    @ViewState private var name = "Claude Code"
    @ViewState private var setup: TimelineService.ClientSetup?
    @ViewState private var error: String?

    private var active: [ClientInfo] { service.clients.filter { $0.revoked_ms == nil } }
    private var revoked: [ClientInfo] { service.clients.filter { $0.revoked_ms != nil } }

    var body: some View {
        SettingsPage(title: "外部访问") {
            SettingsGroup(title: "服务") {
                SettingRow(title: "状态", detail: service.endpoint.map { "只在这台电脑的 \($0) 上提供，只读" }) {
                    StatusDot(state: service.endpoint == nil ? .off : .on, text: service.endpoint == nil ? "未启动" : "运行中")
                }
                HairlineDivider()
                SettingRow(title: "命令行", detail: "在终端里查询时间线，例如 candid search --q 预算 --start -3d") {
                    CopyButton(title: "复制路径", text: TimelineService.cliPath)
                }
            }
            SettingsGroup(title: "已授权") {
                if active.isEmpty {
                    Text("还没有授权任何 AI 工具。").typography(.secondary).foregroundStyle(.secondary).padding(.vertical, Space.row)
                }
                ForEach(active) { client in
                    ClientRow(client: client) { Task { await service.revoke(client.client_id) } }
                    HairlineDivider()
                }
                if let setup { SetupPanel(setup: setup) { self.setup = nil }.padding(.top, Space.s) }
                if adding { addPanel.padding(.top, Space.s) }
                if !adding && setup == nil {
                    QuietButton(title: "＋ 授权一个 AI 工具") { name = "Claude Code"; adding = true }.offset(x: -9).padding(.top, Space.xs)
                }
            }
            if !revoked.isEmpty {
                FoldRow(title: "已撤销（\(revoked.count)）") {
                    ForEach(revoked) { client in ClientRow(client: client, onRevoke: nil); HairlineDivider() }
                }
            }
            Text("每个工具有自己的令牌和范围：最近 90 天的文字与整理结果，不含截图和录音，不能修改任何内容。每次查询都会记录。返回给 AI 的内容会发往它所用的模型服务。")
                .typography(.secondary).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
        .animation(Motion.spring, value: adding)
        .animation(Motion.spring, value: setup)
        .animation(Motion.snappy, value: service.clients)
        .task { await service.refreshClients() }
    }

    private var addPanel: some View {
        InlinePanel {
            VStack(alignment: .leading, spacing: Space.s) {
                Text("名称").typography(.secondary, .medium).foregroundStyle(.secondary)
                CapsuleChoice(options: ["Claude Code", "Codex", "Claude Desktop", "Cursor"].map { ($0, $0) }, selection: $name)
                QuietField(title: "或自定义", text: $name)
                if let error { InlineMessage(kind: .error, text: error) }
                HStack(spacing: 6) {
                    Spacer()
                    QuietButton(title: "取消") { adding = false }
                    PrimaryButton(title: "生成令牌", isDefault: true) {
                        Task {
                            do { setup = try await service.addClient(name: name.trimmingCharacters(in: .whitespaces)); adding = false; error = nil }
                            catch { self.error = error.localizedDescription }
                        }
                    }
                    .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
        }
    }
}

private struct ClientRow: View {
    let client: ClientInfo
    let onRevoke: (() -> Void)?

    private var detail: String {
        var parts = client.transport == "cli" ? [] : ["MCP"]
        if let last = client.last_access_ms {
            parts.append("上次查询 " + Date(timeIntervalSince1970: Double(last) / 1000).formatted(.relative(presentation: .named)))
        } else { parts.append("还没有查询") }
        if client.calls_7d > 0 { parts.append("7 天 \(client.calls_7d) 次" + (client.denied_7d > 0 ? "，拒绝 \(client.denied_7d) 次" : "")) }
        return parts.joined(separator: " · ")
    }

    var body: some View {
        SettingRow(title: client.name, detail: detail) {
            if let onRevoke, client.transport != "cli" {
                ConfirmButton(title: "撤销", confirmTitle: "撤销访问", action: onRevoke)
            } else if client.revoked_ms != nil {
                Tag(text: "已撤销")
            } else {
                Tag(text: "本机")
            }
        }
    }
}

/// Shown once after a client is registered: the token never appears again.
private struct SetupPanel: View {
    let setup: TimelineService.ClientSetup
    let done: () -> Void
    @ViewState private var format = "claude"

    private var snippet: String {
        switch format { case "codex": setup.codex; case "json": setup.json; default: setup.claudeCode }
    }

    var body: some View {
        InlinePanel {
            VStack(alignment: .leading, spacing: Space.s) {
                HStack {
                    Text("「\(setup.name)」的接入方式").typography(.headline)
                    Spacer()
                    Tag(text: "只显示这一次", tint: Palette.warning)
                }
                CapsuleChoice(options: [("claude", "Claude Code 命令"), ("codex", "Codex 配置"), ("json", "通用 JSON")], selection: $format)
                Text(snippet)
                    .typography(.secondary).monospaced().textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(Space.s).frame(maxWidth: .infinity, alignment: .leading)
                    .background(RoundedRectangle(cornerRadius: Radius.small, style: .continuous).fill(Surface.fill.color))
                Text(format == "codex" ? "加到 ~/.codex/config.toml。" : format == "json" ? "用于 Claude Desktop、Cursor 等读取 mcpServers 的工具。" : "在终端运行一次即可，之后在 Claude Code 里直接问。")
                    .typography(.secondary).foregroundStyle(.secondary)
                HStack(spacing: 6) {
                    CopyButton(title: "复制", text: snippet)
                    Spacer()
                    PrimaryButton(title: "完成", action: done)
                }
            }
        }
    }
}

/// Copies text and confirms in place for a moment.
struct CopyButton: View {
    let title: String
    let text: String
    @ViewState private var copied = false

    var body: some View {
        QuietButton(title: copied ? "已复制" : title, emphasized: true) {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(text, forType: .string)
            copied = true
            Task { try? await Task.sleep(for: .seconds(1.5)); copied = false }
        }
    }
}

/// 设置 → 数据 → 时间线库: where the library lives, whether it keeps up with capture, and the last comparison.
struct TimelineLibraryGroup: View {
    private var service: TimelineService { .shared }

    private var state: (StatusDot.State, String) {
        switch service.phase {
        case .off: (.off, "未启动")
        case .opening: (.attention, "正在打开")
        case .syncing: (.attention, "首次同步中")
        case .ready: (.on, "正常")
        case .failed: (.attention, "出错")
        }
    }

    var body: some View {
        SettingsGroup(title: "时间线库") {
            SettingRow(title: "状态", detail: service.lastSync.map { "上次同步 " + $0.formatted(date: .omitted, time: .standard) + (service.lastChange.map { "，\($0)" } ?? "") }) {
                StatusDot(state: state.0, text: state.1)
            }
            if case .failed(let message) = service.phase { InlineMessage(kind: .error, text: message).padding(.bottom, Space.xs) }
            ForEach(problems, id: \.self) { InlineMessage(kind: .warning, text: $0).padding(.bottom, Space.xxs) }
            HairlineDivider()
            SettingRow(title: "事件", detail: service.captureDirect
                       ? "电脑活动、截图和识别直接写入这里；资料库内容每 30 秒同步。"
                       : "电脑活动等原账本核对通过后直接写入这里，在此之前先存在本机待写队列；资料库内容每 30 秒同步。") {
                Text("\(service.events)").typography(.body, .medium, digits: true)
            }
            HairlineDivider()
            SettingRow(title: "核对", detail: verificationDetail) {
                if let v = service.verification { StatusDot(state: v.ok ? .on : .attention, text: v.ok ? "一致" : "有差异") }
            }
            HairlineDivider()
            SettingRow(title: "位置", detail: service.rootPath) {
                QuietButton(title: "在 Finder 中显示") {
                    NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: service.rootPath)])
                }
            }
        } accessory: {
            HStack(spacing: Space.xxs) {
                QuietButton(title: "立即同步") { service.syncNow() }
                QuietButton(title: "重新核对") { Task { await service.verify() } }
            }
        }
    }

    /// Mirroring problems, why the switch to direct capture has not happened, and capture waiting to be written.
    private var problems: [String] {
        var lines = service.syncProblems
        if let reason = service.cutoverError { lines.insert(reason, at: 0) }
        if let store = service.desktopStore, store.pending > 0, let problem = store.deliveryProblem {
            lines.append("\(store.pending) 组电脑记录等待写入：\(problem)")
        }
        return lines
    }

    private var verificationDetail: String {
        if service.cuttingOver { return "正在导入原账本剩余的记录并逐项核对，通过后电脑采集改为直接写入。" }
        guard let v = service.verification else {
            if service.captureDirect, let cutover = service.cutover, cutover.ledger == nil { return "没有原桌面账本，电脑采集从一开始直接写入。" }
            return "启动时和原账本逐项比对，通过后电脑采集直接写入，原账本只读保留。"
        }
        if v.ok {
            let files = v.hashed ? "\(v.blobsChecked) 张截图的文件 hash 一致" : "\(v.blobsChecked) 张截图的文件都在"
            return "各类记录数一致，\(files)（\(v.at.formatted(date: .omitted, time: .shortened))）。"
        }
        return v.differences.prefix(4).joined(separator: "；")
    }
}
