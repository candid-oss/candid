import SwiftUI
import TimelineConnectors
import CandidClient

/// Settings for Rust-owned agent sources (timeline-v1 §6.2).
/// Nothing here reads transcript content; only 读取 being on lets a connector open files.
@MainActor @Observable
final class AgentConnectionsModel {
    struct Row: Identifiable {
        var id: String { description.agent }
        var description: AgentDescription
        var consent: AgentConsent
        var detection: AgentDetection?
    }

    private(set) var rows: [Row] = []
    private(set) var saveError: String?
    private let store: AgentConsentStore?
    private let connectors: [any AgentTranscriptConnector]
    private let client: AgentCoreClient
    private var storedCounts: [String: Int] = [:]
    private(set) var saving = false
    private(set) var pendingRemoval: String?
    /// Agents with a save in flight, and those changed again meanwhile (saved once more with the latest consent).
    private var inFlight: Set<String> = []
    private var dirty: Set<String> = []

    init(store: AgentConsentStore? = nil, connectors: [any AgentTranscriptConnector]? = nil, client: AgentCoreClient? = nil) {
        self.store = store
        let remote = client ?? AgentCoreClient(transport: { path, body in
            let channel = await MainActor.run { TimelineService.shared.channel }
            guard let channel else { throw CoreError.unreachable("时间线核心尚未就绪") }
            return try await channel.send(body == nil ? "GET" : "POST", path, body: body).checked()
        })
        self.client = remote
        let list = connectors ?? AgentConnectors.all(client: remote)
        self.connectors = list
        rows = list.map { Row(description: $0.describe(), consent: store?.consent(for: $0.describe().agent) ?? AgentConsent(agent: $0.describe().agent), detection: nil) }
    }

    /// `detect: false` only rereads the stored consent and counts; detection probes files and is slow.
    func refresh(detect: Bool = true) async {
        if store == nil {
            do {
                let settings = try await client.settings()
                let consents = settings.consents; storedCounts = settings.stored_counts
                for i in rows.indices where !inFlight.contains(rows[i].id) {
                    if let c = consents.first(where: { $0.agent == rows[i].id }) { rows[i].consent = c }
                }
                saveError = nil
            } catch { saveError = "连接设置暂不可用"; return }
        }
        guard detect else { return }
        for (i, c) in connectors.enumerated() {
            let d = await c.detect()
            if i < rows.count { rows[i].detection = d }
        }
    }

    /// Switches answer at once; saves run in the background, one at a time per agent, always with the latest
    /// consent. Turning search or activity on turns reading on; turning reading off turns both off.
    func update(_ agent: String, _ change: (inout AgentConsent) -> Void) {
        guard let i = rows.firstIndex(where: { $0.id == agent }) else { return }
        let old = rows[i].consent
        var c = old; change(&c)
        if c.read == old.read, c.search || c.activity { c.read = true }
        if !c.read { c.search = false; c.activity = false }
        guard c != old else { return }
        let removal = (old.activity && !c.activity) || (old.read && !c.read && (storedCounts[agent] ?? 0) > 0)
        if let store {
            do { try store.save(c); saveError = nil; rows[i].consent = c; if removal { pendingRemoval = agent } }
            catch { saveError = "无法保存设置" }
            return
        }
        rows[i].consent = c
        if c.activity && pendingRemoval == agent { pendingRemoval = nil }
        if removal { pendingRemoval = agent }
        persist(agent)
    }

    private func persist(_ agent: String) {
        guard !inFlight.contains(agent) else { dirty.insert(agent); return }
        inFlight.insert(agent)
        Task {
            var failed = false
            repeat {
                dirty.remove(agent)
                guard let consent = rows.first(where: { $0.id == agent })?.consent else { break }
                do { try await client.save(consent); saveError = nil }
                catch { saveError = "无法保存设置"; failed = true; break }
            } while dirty.contains(agent)
            inFlight.remove(agent)
            if !failed { NotificationCenter.default.post(name: .candidAgentConsentChanged, object: self) }
            await refresh(detect: false)
        }
    }

    func keepStored() { pendingRemoval = nil }
    func deleteStored() async {
        guard !saving, let agent = pendingRemoval, let row = rows.first(where: { $0.id == agent }) else { return }
        saving = true
        do { try await client.save(row.consent, deleteStored: true); pendingRemoval = nil; saveError = nil; await refresh() }
        catch { saveError = "删除未完成，可重试" }
        saving = false
    }

    func setPaths(_ agent: String, _ text: String) {
        let items = Self.split(text)
        update(agent) { $0.paths = items }
    }
    func setExcludedSessions(_ agent: String, _ text: String) {
        let items = Self.split(text)
        update(agent) { $0.exclude.sessions = items }
    }
    func setExcludedProjects(_ agent: String, _ text: String) {
        let items = Self.split(text)
        update(agent) { $0.exclude.projects = items }
    }
    static func split(_ text: String) -> [String] {
        text.split(whereSeparator: { $0 == "," || $0 == "\n" || $0 == "，" })
            .map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
    }

    static func statusText(_ r: Row) -> (text: String, on: Bool) {
        guard let d = r.detection else { return ("检测中…", false) }
        switch d.state {
        case .notFound: return ("未发现", false)
        case .unknownFormat: return ("格式未知，已停止读取", false)
        case .found:
            if r.consent.read { return (r.consent.activity ? "已授权：纳入活动" : (r.consent.search ? "已授权：读取、搜索" : "已授权：读取"), true) }
            return ("已发现，未授权", false)
        }
    }
}

struct AgentConnectionsGroup: View {
    @ViewState private var model: AgentConnectionsModel
    init(model: AgentConnectionsModel = AgentConnectionsModel()) { _model = ViewState(initialValue: model) }

    var body: some View {
        VStack(alignment: .leading, spacing: Space.l) {
            ForEach(model.rows) { row in AgentDetailPane(row: row, model: model) }
        }
        .task { await model.refresh() }
    }
}

/// One AI agent source on the connections page: what it reads, three switches, and its folders.
struct AgentDetailPane: View {
    let row: AgentConnectionsModel.Row
    let model: AgentConnectionsModel
    @ViewState private var pathText = ""
    @ViewState private var excludeText = ""
    @ViewState private var sessionText = ""
    @ViewState private var loaded = false

    private var available: Bool { row.detection?.state == .found }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("读取 \(row.description.displayName) 的本机会话文件。纳入活动后保存你发出的指令和工作摘要；回复正文不存，用时从文件现读。")
                .typography(.body).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                .padding(.bottom, Space.m)
            SettingsGroup(title: "授权") {
                switchRow("读取", detail: "允许 Candid 打开这些文件", isOn: row.consent.read, enabled: available || row.consent.read) { v in model.update(row.id) { $0.read = v } }
                HairlineDivider()
                switchRow("纳入搜索", detail: "指令可以被搜索到；会同时打开读取", isOn: row.consent.search, enabled: available || row.consent.read) { v in model.update(row.id) { $0.search = v } }
                HairlineDivider()
                switchRow("纳入活动", detail: "指令和工作出现在时间线的 AI 轨；会同时打开读取", isOn: row.consent.activity, enabled: available || row.consent.read) { v in model.update(row.id) { $0.activity = v } }
            }
            if model.pendingRemoval == row.id {
                HStack(spacing: Space.xs) {
                    Text("已停止同步。删除已保存的消息和工作摘要？").typography(.secondary)
                    Spacer()
                    QuietButton(title: "保留") { model.keepStored() }
                    QuietButton(title: "删除", role: .destructive) { Task { await model.deleteStored() } }
                }
                .disabled(model.saving)
                .padding(.top, Space.s)
            }
            SettingsGroup(title: "范围") {
                VStack(alignment: .leading, spacing: Space.s) {
                    QuietField(title: "对话文件夹（留空用默认）", text: $pathText, placeholder: row.description.defaultPaths.joined(separator: ", "), mono: true)
                    QuietField(title: "排除的项目目录（逗号分隔）", text: $excludeText, placeholder: "/path/to/project", mono: true)
                    QuietField(title: "排除的会话（逗号分隔）", text: $sessionText, placeholder: "会话来源引用", mono: true)
                    HStack {
                        Spacer()
                        SecondaryButton(title: "保存范围") {
                            model.update(row.id) { consent in
                                consent.paths = AgentConnectionsModel.split(pathText)
                                consent.exclude.projects = AgentConnectionsModel.split(excludeText)
                                consent.exclude.sessions = AgentConnectionsModel.split(sessionText)
                            }
                        }
                        .disabled(model.saving)
                    }
                }
                .padding(.vertical, Space.xs)
            }
            .padding(.top, Space.l)
        }
        .onAppear {
            guard !loaded else { return }
            loaded = true
            pathText = row.consent.paths.joined(separator: ", ")
            sessionText = row.consent.exclude.sessions.joined(separator: ", ")
            excludeText = row.consent.exclude.projects.joined(separator: ", ")
        }
    }

    private func switchRow(_ title: String, detail: String, isOn: Bool, enabled: Bool, set: @escaping (Bool) -> Void) -> some View {
        SettingRow(title: title, detail: detail) {
            Toggle(title, isOn: Binding(get: { isOn }, set: { set($0) })).toggleStyle(.switch).labelsHidden().controlSize(.small)
        }
        .disabled(!enabled)
    }
}


struct WindowSourceConnections: View {
    let session: DesktopSession
    @ViewState private var windowID = ""
    @ViewState private var reference = ""
    @ViewState private var result = ""
    @ViewState private var choices: [AgentSessionInfo] = []
    @ViewState private var sourcePreview = ""
    @ViewState private var sourceGeneration = 0
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("窗口与会话").typography(.headline)
            Text("选择已记录的输入窗口，填入获准会话的来源引用。关联仅在本次采集有效；切换会话时请清除并重新关联。").typography(.secondary).foregroundStyle(.secondary)
            SettingRow(title: "输入窗口") {
                MenuChoice(options: [("", "选择窗口")] + session.inputWindows.map { ($0.id, $0.label) }, selection: $windowID)
            }
            SettingRow(title: "获准的会话") {
                HStack(spacing: Space.xs) {
                    MenuChoice(options: [("", "选择会话")] + choices.map { item in
                        (item.ref, (item.title ?? item.agent) + " · " + (item.project.map { URL(fileURLWithPath: $0).lastPathComponent } ?? "") + " · " + Date(timeIntervalSince1970: Double(item.startMs ?? 0) / 1000).formatted())
                    }, selection: $reference)
                    QuietButton(title: "刷新") { Task { await refreshChoices() } }
                }
            }
            if !sourcePreview.isEmpty { Text(sourcePreview).typography(.secondary).textSelection(.enabled) }
            HStack(spacing: Space.xs) {
                QuietButton(title: "清除关联") { session.clearWindowLinks(); result = "已清除" }
                QuietButton(title: "确认这段输入已发出") { result = session.confirmScreenInput(windowID) ? "已确认，正在按屏幕来源保存" : "输入已失效，请重新采集后核对" }
                Spacer()
                PrimaryButton(title: "关联会话") { Task { result = await session.associateWindow(windowID, with: reference) ? "已关联" : "关联未完成，请检查授权与引用" } }
                    .disabled(reference.isEmpty)
            }.disabled(windowID.isEmpty)
            Text("云端 AI 应用或网页没有会话文件时，可核对已保存的完整输入后确认；回车、点击和清空本身不会确认发出。").typography(.secondary).foregroundStyle(.secondary)
            if let window = session.inputWindows.first(where: { $0.id == windowID }), let text = window.record.context?.recordEvidence?.input_value { Text(text).typography(.secondary).textSelection(.enabled) }
            if !result.isEmpty { Text(result).typography(.secondary) }
        }
        .task { await refreshChoices() }
        .onReceive(NotificationCenter.default.publisher(for: .candidAgentConsentChanged)) { _ in
            sourceGeneration += 1; sourcePreview = ""; choices = []; reference = ""
            Task { await refreshChoices() }
        }
        .task(id: reference) {
            sourcePreview = ""
            guard !reference.isEmpty, let channel = TimelineService.shared.channel,
                  let connector = AgentConnectors.connector(agent: reference.split(separator: ":").dropFirst().first.map(String.init) ?? "", client: AgentCoreClient(channel: channel)) else { return }
            let ticket = sourceGeneration, selected = reference
            if let page = try? await connector.read(ref: reference, cursor: nil, limit: 5), !Task.isCancelled,
               let consents = try? await AgentCoreClient(channel: channel).consents(),
               consents.contains(where: { $0.read && $0.activity && selected.hasPrefix("agent:" + $0.agent + ":") }),
               ticket == sourceGeneration, reference == selected {
                sourcePreview = page.messages.filter { $0.role == "user" }.compactMap(\.text).joined(separator: "\n\n")
            }
        }
    }
    private func refreshChoices() async {
        sourceGeneration += 1; let ticket = sourceGeneration
        guard let channel = TimelineService.shared.channel else { choices = []; return }
        let client = AgentCoreClient(channel: channel)
        do {
            let consents = try await client.consents()
            var found: [AgentSessionInfo] = []
            for connector in AgentConnectors.all(client: client) where consents.contains(where: { $0.agent == connector.describe().agent && $0.read && $0.activity }) {
                let page = try await connector.sessions(range: DateInterval(start: Date().addingTimeInterval(-7 * 86400), end: Date().addingTimeInterval(1)), project: nil)
                found += page.sessions
                if page.limited == true { result = "仅显示已核对的部分会话，可缩小来源目录后重试" }
            }
            guard ticket == sourceGeneration else { return }
            choices = found
            if !choices.contains(where: { $0.ref == reference }) { reference = ""; sourcePreview = "" }
        } catch { if ticket == sourceGeneration { choices = []; reference = ""; sourcePreview = ""; result = "会话列表未能读取，可重试" } }
    }
}
