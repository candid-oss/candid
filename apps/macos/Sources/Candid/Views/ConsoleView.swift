import SwiftUI
import UniformTypeIdentifiers
import ConsoleCore

private enum DevicePanel: String, CaseIterable, Identifiable {
    case workspace = "设备", library = "录音库"
    var id: Self { self }
}

/// Embedded in the main workspace. Observation exists only while this view is visible.
struct ConsoleView: View {
    @Bindable var store: ConsoleStore
    var openConnectionSettings: () -> Void = {}
    @ViewState private var panel: DevicePanel = .workspace
    @ViewState private var importing = false
    @ViewState private var observationReady = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: Space.s) {
                PillSwitcher(items: DevicePanel.allCases.map { ($0, $0.rawValue) }, selection: $panel).offset(x: -13)
                Spacer()
                ConnectionIndicator(store: store)
                if panel == .library {
                    QuietButton(title: "导入录音") { importing = true }
                        .disabled(!canImport).keyboardShortcut("o", modifiers: [.command])
                        .help("导入 WAV，最多 20 MiB")
                }
                if store.connection != .online { QuietButton(title: "连接设置", action: openConnectionSettings) }
                QuietButton(title: "刷新") { Task { await store.refresh() } }.disabled(store.busy != nil)
            }
            .padding(.horizontal, Space.page).padding(.top, Space.s).padding(.bottom, Space.s)
            if let error = store.error, store.snapshot != nil {
                HStack(alignment: .firstTextBaseline) {
                    InlineMessage(kind: .error, text: error)
                    Spacer()
                    QuietButton(title: "关闭", action: store.dismissError)
                }
                .padding(.horizontal, Space.page).padding(.bottom, Space.s)
            }
            Group {
                switch panel {
                case .workspace:
                    if store.snapshot == nil { connectionEmptyState }
                    else { deviceWorkspace }
                case .library:
                    RecordingLibraryView(store: store, importAction: { importing = true })
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
        .task(id: store.connectionRevision) {
            observationReady = false
            await store.run()
        }
        .onChange(of: store.lastUpdated) { _, updated in
            observationReady = updated != nil && store.connection == .online
        }
        .onChange(of: store.connection) { _, connection in
            if connection != .online { observationReady = false }
            else if store.lastUpdated != nil { observationReady = true }
        }
        .fileImporter(isPresented: $importing, allowedContentTypes: [.wav]) { result in
            switch result {
            case let .success(url):
                panel = .library
                Task { await store.importAudio(from: url) }
            case let .failure(error): store.reportError(error.localizedDescription)
            }
        }
    }

    private var connectionEmptyState: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(store.connection == .connecting ? "正在连接设备服务" : "设备服务未连接").typography(.headline)
            Text(store.service.endpoint).typography(.secondary).monospaced().foregroundStyle(.secondary).textSelection(.enabled)
            if let error = store.error { InlineMessage(kind: .error, text: error) }
            HStack(spacing: Space.xxs) {
                QuietButton(title: "连接设置", emphasized: true, action: openConnectionSettings)
                QuietButton(title: "重试") { Task { await store.refresh() } }
            }
            .offset(x: -9)
        }
        .frame(maxWidth: 520, alignment: .leading)
        .padding(.horizontal, Space.page).padding(.top, Space.l)
    }

    private var deviceWorkspace: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Space.l) {
                DeviceControlView(store: store)
                SettingsGroup(title: "连接") {
                    SettingRow(title: "本机服务") { Text(store.service.endpoint).typography(.body).monospaced().foregroundStyle(.secondary) }
                    HairlineDivider()
                    SettingRow(title: "USB 端口") { Text(store.snapshot?.serial?.path ?? "未连接").typography(.body).foregroundStyle(.secondary) }
                    HairlineDivider()
                    SettingRow(title: "最近更新") {
                        Text(store.lastUpdated?.formatted(date: .abbreviated, time: .standard) ?? "尚无状态").typography(.body, digits: true).foregroundStyle(.secondary)
                    }
                }
                .textSelection(.enabled)
            }
            .frame(maxWidth: 900, alignment: .leading)
            .padding(.horizontal, Space.page).padding(.bottom, Space.xl)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .scrollIndicators(.hidden)
    }

    private var canImport: Bool {
        observationReady && store.connection == .online && store.busy == nil && !store.service.isMock
    }
}

struct ConnectionIndicator: View {
    let store: ConsoleStore
    var body: some View {
        StatusDot(state: store.connection == .online ? .on : .off, text: store.service.isMock ? "合成样例" : store.connection.rawValue)
            .help(store.service.endpoint)
    }
}

struct ConsoleConnectionSettings: View {
    @Bindable var store: ConsoleStore
    @ViewState private var endpoint = ""
    @ViewState private var demo = false
    @ViewState private var validationError: String?

    var body: some View {
        SettingsPage(title: "设备") {
            SettingsGroup(title: "设备服务") {
                SwitchRow(title: "合成样例", detail: "不连接真实设备，用合成数据演示。", isOn: $demo)
                HairlineDivider()
                QuietField(title: "服务地址", text: $endpoint, placeholder: "http://127.0.0.1:8766", mono: true)
                    .disabled(demo).opacity(demo ? 0.5 : 1).padding(.vertical, Space.row)
                if let validationError { InlineMessage(kind: .error, text: validationError).padding(.bottom, Space.xs) }
                if !store.canChangeConnection { InlineMessage(kind: .note, text: "请先结束录音、文件接收或 VAD 监听。").padding(.bottom, Space.xs) }
                HStack(spacing: 6) {
                    QuietButton(title: "恢复默认地址") { endpoint = "http://127.0.0.1:8766" }.offset(x: -9)
                    Spacer()
                    PrimaryButton(title: "应用连接", action: apply)
                        .disabled(!store.canChangeConnection || !hasChanges).opacity(!store.canChangeConnection || !hasChanges ? 0.45 : 1)
                }
            }
            SettingsGroup(title: "当前") {
                SettingRow(title: "地址") {
                    Text(store.service.isMock ? "合成样例" : store.service.endpoint).typography(.body).monospaced().foregroundStyle(.secondary)
                }
            }
        }
        .onAppear {
            endpoint = store.service.isMock ? UserDefaults.standard.string(forKey: "console.endpoint") ?? "http://127.0.0.1:8766" : store.service.endpoint
            demo = store.service.isMock
        }
        .onChange(of: endpoint) { _, _ in validationError = nil }
        .onChange(of: demo) { _, _ in validationError = nil }
    }

    private var hasChanges: Bool {
        demo != store.service.isMock || (!demo && endpoint.trimmingCharacters(in: .whitespacesAndNewlines) != store.service.endpoint)
    }
    private func apply() {
        do {
            try store.changeConnection(endpoint: endpoint, mock: demo)
            if !demo { UserDefaults.standard.set(store.service.endpoint, forKey: "console.endpoint") }
            UserDefaults.standard.set(demo, forKey: "console.demo")
            validationError = nil
        } catch { validationError = error.localizedDescription }
    }
}
