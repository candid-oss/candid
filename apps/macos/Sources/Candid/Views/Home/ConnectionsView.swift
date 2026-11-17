import ConsoleCore
import SwiftUI
import TimelineConnectors

/// Every source and destination in one place: a list grouped by kind on the
/// left, the selected source on the right. Live sources show their real state;
/// planned ones are folded at the bottom and say so.
struct ConnectionsView: View {
    let session: DesktopSession
    let media: MediaWorkspaceStore
    let deviceStore: ConsoleStore?
    let open: (HomePage) -> Void

    @AppStorage("connections.selected") private var selected = "mac"
    @AppStorage("connections.plannedOpen") private var plannedOpen = false
    @ViewState private var agents = AgentConnectionsModel()
    @ViewState private var connectors: ConnectorsModel?

    struct Source: Identifiable {
        enum Group: String, CaseIterable { case capture = "采集", agents = "AI 会话", apps = "应用数据", storage = "存储与访问", planned = "规划中" }
        let id: String
        let group: Group
        let title: String
        let symbol: String
        let tint: Color
        let state: StatusDot.State
        let status: String
    }

    private var sources: [Source] {
        var list: [Source] = [
            Source(id: "mac", group: .capture, title: "这台 Mac", symbol: "desktopcomputer", tint: .blue,
                   state: session.isDemo ? .off : session.isRecording ? .on : .off,
                   status: session.isDemo ? "合成样例" : session.isRecording ? "记录中" : "已暂停"),
            Source(id: "device", group: .capture, title: "设备服务", symbol: "waveform", tint: .purple,
                   state: deviceStore?.connection == .online && !session.isDemo ? .on : .off,
                   status: session.isDemo ? "合成样例不连接" : deviceStore.map { $0.connection == .online ? "已连接" : $0.connection.rawValue } ?? "未配置"),
        ]
        for row in agents.rows {
            let status = AgentConnectionsModel.statusText(row)
            list.append(Source(id: "agent:" + row.id, group: .agents, title: row.description.displayName, symbol: "terminal",
                               tint: row.id == "claude_code" ? .orange : .gray, state: status.on ? .on : .off, status: status.text))
        }
        list.append(Source(id: "windows", group: .agents, title: "窗口与会话", symbol: "link", tint: .indigo,
                           state: session.inputWindows.isEmpty ? .off : .attention,
                           status: session.inputWindows.isEmpty ? "没有待关联窗口" : "\(session.inputWindows.count) 个窗口可关联"))
        list += [
            Source(id: "mail", group: .apps, title: "邮件", symbol: "envelope", tint: .blue,
                   state: connectorModel.mailStatus.state, status: connectorModel.mailStatus.text),
            Source(id: "browser", group: .apps, title: "浏览器历史", symbol: "globe", tint: .teal,
                   state: connectorModel.browserStatus.state, status: connectorModel.browserStatus.text),
            Source(id: "library", group: .storage, title: "本机资料库", symbol: "internaldrive", tint: .gray,
                   state: media.connectionError == nil ? .on : .off, status: media.connectionError == nil ? "可用" : "未连接"),
            Source(id: "access", group: .storage, title: "外部访问（MCP）", symbol: "puzzlepiece.extension", tint: .green,
                   state: TimelineService.shared.endpoint == nil ? .off : .on,
                   status: TimelineService.shared.endpoint == nil ? "核心尚未就绪" : "本机只读服务"),
            Source(id: "calendar", group: .planned, title: "日历与提醒事项", symbol: "calendar", tint: .red, state: .planned, status: "规划中"),
            Source(id: "pendant", group: .planned, title: "挂件", symbol: "circle.hexagongrid", tint: .gray, state: .planned, status: "规划中"),
            Source(id: "phone", group: .planned, title: "手机", symbol: "iphone", tint: .gray, state: .planned, status: "规划中"),
            Source(id: "hub", group: .planned, title: "家庭 Hub / NAS", symbol: "server.rack", tint: .gray, state: .planned, status: "规划中"),
            Source(id: "notion", group: .planned, title: "Notion", symbol: "doc.text", tint: .gray, state: .planned, status: "规划中"),
        ]
        return list
    }

    /// Created on first use so the demo flag is known; synthetic sessions never reach the core's connectors.
    private var connectorModel: ConnectorsModel { connectors ?? ConnectorsModel(demo: true) }

    private var current: Source { sources.first { $0.id == selected } ?? sources[0] }

    var body: some View {
        HStack(alignment: .top, spacing: Space.l) {
            list.frame(width: 290)
            ScrollView {
                detail.id(current.id)
                    .padding(.horizontal, 40).padding(.vertical, 28)
                    .frame(maxWidth: 820, alignment: .leading)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .transition(.opacity)
            }
            .scrollIndicators(.hidden)
            .panelSurface(radius: Radius.card)
            .padding(.bottom, Space.l)
        }
        .padding(.leading, 28).padding(.trailing, 28).padding(.top, 62)
        .animation(Motion.snappy, value: selected)
        .task {
            if connectors == nil { connectors = ConnectorsModel(demo: session.isDemo) }
            await agents.refresh()
            await connectors?.refresh()
        }
        .onChange(of: TimelineService.shared.dataVersion) { _, _ in Task { await connectors?.refresh() } }
        .onReceive(NotificationCenter.default.publisher(for: .candidAgentConsentChanged)) { note in
            // Its own saves already reread the settings; only changes made elsewhere need a reread here.
            if (note.object as AnyObject?) !== agents { Task { await agents.refresh(detect: false) } }
        }
    }

    // MARK: List

    private var list: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 2) {
                Text("连接").typography(.display).padding(.leading, 12).padding(.bottom, Space.m)
                ForEach(Source.Group.allCases, id: \.self) { group in
                    let items = sources.filter { $0.group == group }
                    if !items.isEmpty {
                        if group == .planned {
                            Button { withAnimation(Motion.spring) { plannedOpen.toggle() } } label: {
                                HStack(spacing: 5) {
                                    Image(systemName: "chevron.down").imageScale(.small).rotationEffect(.degrees(plannedOpen ? 0 : -90))
                                    Text("\(group.rawValue) · \(items.count)")
                                }
                                .typography(.caption).foregroundStyle(.tertiary).contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .padding(.leading, 12).padding(.top, Space.m).padding(.bottom, Space.xxs)
                        } else {
                            Text(group.rawValue).typography(.caption).foregroundStyle(.tertiary)
                                .padding(.leading, 12).padding(.top, Space.m).padding(.bottom, Space.xxs)
                        }
                        if group != .planned || plannedOpen {
                            ForEach(items) { source in SourceRow(source: source, selected: source.id == selected) { selected = source.id } }
                        }
                    }
                }
            }
            .padding(.bottom, Space.l)
        }
        .scrollIndicators(.hidden)
    }

    // MARK: Detail

    @ViewBuilder private var detail: some View {
        let source = current
        VStack(alignment: .leading, spacing: Space.l) {
            HStack(spacing: Space.m) {
                SourceIcon(symbol: source.symbol, tint: source.tint, planned: source.state == .planned, size: 44)
                VStack(alignment: .leading, spacing: 3) {
                    Text(source.title).typography(.title)
                    StatusDot(state: source.state, text: source.status)
                }
                Spacer()
            }
            switch source.id {
            case "mac": macDetail
            case "device": deviceDetail
            case "windows":
                lead("把屏幕上的输入窗口和获准的会话文件对上，同一条指令只显示一次。网页里的对话没有会话文件时，核对完整输入后手动确认。")
                WindowSourceConnections(session: session)
            case "mail":
                lead("只读本机「邮件」App 的数据，覆盖其中登录的全部账户；也可以直接用 IMAP 登录某个邮箱。正文入库前先过筛选规则，附件只记名称和大小。")
                MailConnectorPane(model: connectorModel)
            case "browser":
                lead("Safari 和 Chrome 的访问记录：网址、标题、时间和次数。这一版只连接和统计，不写入时间线。")
                BrowserConnectorPane(model: connectorModel)
            case "library": libraryDetail
            case "access":
                lead("外部 AI 经 MCP 或命令行读取你选定范围的时间线，每次读取都有记录，可随时撤销。")
                TimelineAccessView()
            default:
                if source.id.hasPrefix("agent:"), let row = agents.rows.first(where: { "agent:" + $0.id == source.id }) {
                    AgentDetailPane(row: row, model: agents)
                    if let error = agents.saveError { InlineMessage(kind: .error, text: error) }
                } else {
                    planned("还没有接入，不会读取任何数据。")
                }
            }
        }
    }

    private var macDetail: some View {
        VStack(alignment: .leading, spacing: Space.l) {
            lead("屏幕活动、窗口文字和截图，只在你允许的应用里记录。暂停后不再采集。")
            HStack(spacing: Space.s) {
                stat("\(session.allowedBundleIDs.count)", "允许截图的应用")
                stat("\(session.excludedBundleIDs.count)", "排除的应用")
                stat(session.isRecording ? "记录中" : "已暂停", "电脑记录")
            }
            SettingsGroup(title: "系统授权") {
                permissionRow("屏幕录制", detail: "窗口截图和框选需要", granted: session.screenPermission, request: session.requestScreenPermission)
                HairlineDivider()
                permissionRow("辅助功能", detail: "读取窗口标题、焦点和可读文字", granted: session.accessibilityPermission, request: session.requestAccessibilityPermission)
            }
            SettingsGroup(title: "记录") {
                SettingRow(title: "电脑记录") {
                    Toggle("电脑记录", isOn: Binding(get: { session.isRecording }, set: { $0 ? session.startRecording() : session.pauseRecording() }))
                        .toggleStyle(.switch).labelsHidden().controlSize(.small)
                }
                .disabled(session.isDemo)
                HairlineDivider()
                SettingRow(title: "允许与排除的应用") {
                    QuietButton(title: "管理应用", emphasized: true) { openSettings("scope") }
                }
                HairlineDivider()
                SettingRow(title: "截图、文字和采集频率") {
                    QuietButton(title: "记录设置", emphasized: true) { openSettings("capture") }
                }
            }
        }
    }

    private var deviceDetail: some View {
        VStack(alignment: .leading, spacing: Space.l) {
            lead("调试台和 USB 录音、拍照。设备服务在另一台电脑或另一个端口时，在设置里填写地址。")
            SettingsGroup(title: "连接") {
                SettingRow(title: "状态") {
                    StatusDot(state: current.state, text: current.status)
                }
                HairlineDivider()
                SettingRow(title: "调试台", detail: "录音、拍照、回听和本地转写") {
                    SecondaryButton(title: "打开") { open(.device) }.disabled(deviceStore == nil)
                }
                HairlineDivider()
                SettingRow(title: "服务地址") {
                    QuietButton(title: "修改", emphasized: true) { openSettings("console") }
                }
            }
        }
    }

    private var libraryDetail: some View {
        VStack(alignment: .leading, spacing: Space.l) {
            lead("录音、截图、转写和时间线都存在这台 Mac 上。")
            HStack(spacing: Space.s) {
                stat("\(media.records.count)", media.isDemo ? "资料 · 合成样例" : "资料")
                stat(media.connectionError == nil ? "可用" : "未连接", "资料库")
            }
            SettingsGroup(title: "管理") {
                SettingRow(title: "浏览资料") { SecondaryButton(title: "打开资料") { open(.library) } }
                HairlineDivider()
                SettingRow(title: "位置、导出与留存") { QuietButton(title: "数据设置", emphasized: true) { openSettings("storage") } }
            }
        }
    }

    // MARK: Pieces

    private func lead(_ text: String) -> some View {
        Text(text).typography(.body).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true).frame(maxWidth: 620, alignment: .leading)
    }

    private func planned(_ text: String) -> some View {
        HStack(spacing: Space.xs) { Tag(text: "规划中"); Text(text).typography(.secondary).foregroundStyle(.secondary) }
    }

    private func stat(_ value: String, _ label: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(value).typography(.section, digits: true)
            Text(label).typography(.secondary).foregroundStyle(.secondary)
        }
        .padding(.horizontal, Space.m).padding(.vertical, Space.s)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous).fill(Surface.wash.color))
    }

    private func permissionRow(_ title: String, detail: String, granted: Bool, request: @escaping () -> Void) -> some View {
        SettingRow(title: title, detail: detail) {
            HStack(spacing: Space.s) {
                StatusDot(state: granted ? .on : .attention, text: granted ? "已允许" : "未允许")
                if !granted { PrimaryButton(title: "授权", action: request).disabled(session.isDemo) }
            }
        }
    }

    private func openSettings(_ tab: String) {
        UserDefaults.standard.set(tab, forKey: "desktop.settings.tab")
        open(.settings)
    }
}

/// One row of the connections list: icon, name, state.
private struct SourceRow: View {
    let source: ConnectionsView.Source
    let selected: Bool
    let action: () -> Void
    @ViewState private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: Space.s) {
                SourceIcon(symbol: source.symbol, tint: source.tint, planned: source.state == .planned, size: source.group == .planned ? 24 : 30)
                Text(source.title).typography(.body, .medium).lineLimit(1)
                Spacer(minLength: Space.xs)
                if source.group != .planned { StatusDot(state: source.state, text: source.status).lineLimit(1) }
            }
            .padding(.horizontal, Space.s).frame(height: source.group == .planned ? 36 : 44)
            .background(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous)
                .fill(selected ? Surface.selected.color : hovering ? Surface.hover.color : .clear))
            .opacity(source.group == .planned && !selected ? 0.6 : 1)
            .contentShape(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

private struct SourceIcon: View {
    let symbol: String
    let tint: Color
    let planned: Bool
    let size: CGFloat
    var body: some View {
        Image(systemName: symbol)
            .font(.system(size: size * 0.48, weight: .medium))  // style-exempt: glyph scales with its container
            .foregroundStyle(.white)
            .frame(width: size, height: size)
            .background(RoundedRectangle(cornerRadius: size * 0.26, style: .continuous)
                .fill(LinearGradient(colors: [tint.opacity(planned ? 0.45 : 0.95), tint.opacity(planned ? 0.35 : 0.75)], startPoint: .top, endPoint: .bottom)))
            .accessibilityHidden(true)
    }
}
