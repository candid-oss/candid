import AppKit
import AVFoundation
import CandidLocalization
import DesktopCore
import EventKit
import Foundation

extension Notification.Name {
    /// Shows the first-run guide again (from 关于 or the menu bar panel).
    static let candidShowOnboarding = Notification.Name("candid.onboarding.show")
}

/// One switch-level choice for what to record. Applied through the existing capture setters, never a parallel config.
enum CapturePreset: String, CaseIterable, Identifiable {
    case light, standard, full
    var id: String { rawValue }

    var title: String {
        switch self { case .light: L("轻量"); case .standard: L("标准"); case .full: L("全面") }
    }
    var detail: String {
        switch self {
        case .light: L("只记录用过哪些应用和窗口标题。不截图、不读正文。")
        case .standard: L("再读取你允许的应用里的文字，并自动截图和本机 OCR 识别。")
        case .full: L("在标准之外，通话时提醒录音，并读取日历和提醒事项。")
        }
    }
    var symbol: String {
        switch self { case .light: "macwindow"; case .standard: "text.viewfinder"; case .full: "waveform" }
    }
    /// Only what this level needs; the guide asks for nothing else.
    var permissions: [SystemPermission] {
        switch self {
        case .light: [.accessibility]
        case .standard: [.screen, .accessibility]
        case .full: [.screen, .accessibility, .microphone, .calendar, .reminders]
        }
    }
    var needsApps: Bool { self != .light }
}

/// A macOS privacy permission Candid may use, with its live state and the System Settings pane that grants it.
enum SystemPermission: String, CaseIterable, Identifiable {
    case screen, accessibility, microphone, calendar, reminders
    var id: String { rawValue }

    enum State { case granted, denied, notDetermined }

    var title: String {
        switch self {
        case .screen: L("屏幕录制"); case .accessibility: L("辅助功能"); case .microphone: L("麦克风")
        case .calendar: L("日历"); case .reminders: L("提醒事项")
        }
    }
    var detail: String {
        switch self {
        case .screen: L("截取你允许的应用窗口。")
        case .accessibility: L("识别当前窗口，读取允许的文字。")
        case .microphone: L("录下你开始的录音。")
        case .calendar: L("把会议放进时间线。")
        case .reminders: L("把待办放进时间线。")
        }
    }
    var symbol: String {
        switch self {
        case .screen: "rectangle.dashed.badge.record"; case .accessibility: "accessibility"; case .microphone: "mic"
        case .calendar: "calendar"; case .reminders: "checklist"
        }
    }
    private var pane: String {
        switch self {
        case .screen: "Privacy_ScreenCapture"; case .accessibility: "Privacy_Accessibility"; case .microphone: "Privacy_Microphone"
        case .calendar: "Privacy_Calendars"; case .reminders: "Privacy_Reminders"
        }
    }

    /// Cheap, synchronous reads of the system's own answer; safe to call every second.
    var state: State {
        switch self {
        case .screen: CGPreflightScreenCaptureAccess() ? .granted : .notDetermined
        case .accessibility: AXIsProcessTrusted() ? .granted : .notDetermined
        case .microphone:
            switch AVCaptureDevice.authorizationStatus(for: .audio) {
            case .authorized: .granted; case .notDetermined: .notDetermined; default: .denied
            }
        case .calendar, .reminders:
            switch EKEventStore.authorizationStatus(for: self == .calendar ? .event : .reminder) {
            case .fullAccess, .authorized: .granted; case .notDetermined: .notDetermined; default: .denied
            }
        }
    }

    func openSettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?\(pane)") { NSWorkspace.shared.open(url) }
    }

    /// Shows the system prompt where macOS still allows one, otherwise the matching System Settings pane.
    @MainActor func request(session: DesktopSession) async {
        switch self {
        case .screen: session.requestScreenPermission()
        case .accessibility: session.requestAccessibilityPermission()
        case .microphone:
            if state == .notDetermined { _ = await AVCaptureDevice.requestAccess(for: .audio) } else { openSettings() }
        case .calendar, .reminders:
            if state == .notDetermined {
                let store = EKEventStore()
                _ = try? await (self == .calendar ? store.requestFullAccessToEvents() : store.requestFullAccessToReminders())
            } else { openSettings() }
        }
    }
}

/// State of the first-run guide: the chosen level, live permission states, local model servers and agent hookup.
@MainActor @Observable final class OnboardingModel {
    static let doneKey = "candid.onboarding.v1.done"

    enum Step: Int, CaseIterable { case welcome, capture, permissions, ai, done }

    var step: Step = .welcome
    var preset: CapturePreset = .standard
    private(set) var states: [SystemPermission: SystemPermission.State] = [:]
    /// What applying the level could not switch on (missing permission or apps), shown on the last step.
    private(set) var skipped: [String] = []
    private(set) var servers: [LocalServer] = LocalServer.known
    private(set) var detecting = false
    private(set) var agentResult: [AgentTarget: AgentOutcome] = [:]
    @ObservationIgnored private var setups: [AgentTarget: TimelineService.ClientSetup] = [:]

    func refreshPermissions() {
        for permission in SystemPermission.allCases { states[permission] = permission.state }
    }

    func state(_ permission: SystemPermission) -> SystemPermission.State { states[permission] ?? permission.state }

    // MARK: Applying the level

    /// Writes the chosen level into the existing capture settings. Items whose permission is still missing stay off
    /// and are listed in `skipped`; the settings page can switch them on later.
    func apply(session: DesktopSession, media: MediaWorkspaceStore) {
        skipped = []
        guard !session.isDemo else { return }
        refreshPermissions()
        let ax = state(.accessibility) == .granted, screen = state(.screen) == .granted
        session.updateContentOption(\.windowTitleEnabled, value: true)
        if preset == .light {
            if session.screenshotsEnabled { session.setScreenshotsEnabled(false) }
            if session.textEnabled { session.setTextEnabled(false) }
            if !ax { skipped.append(L("窗口标题需要辅助功能授权")) }
        } else {
            let hasApps = !session.textAllowedBundleIDs.isEmpty || !session.allowedBundleIDs.isEmpty
            if !hasApps { skipped.append(L("还没有选择应用，文字和截图暂不开启")) }
            if ax, hasApps, !session.textEnabled { session.setTextEnabled(true) }
            if !ax { skipped.append(L("读取文字需要辅助功能授权")) }
            if ax, screen, hasApps {
                if !session.screenshotsEnabled { session.setScreenshotsEnabled(true) }
                if session.screenshotsEnabled, !session.automaticCaptureAvailable { session.setAutomaticCaptureEnabled(true) }
                var ocr = session.recognition.policy(.ocr)
                if ocr.mode == .manual { ocr.mode = .immediate; session.recognition.setPolicy(ocr, for: .ocr) }
            } else if !screen {
                skipped.append(L("截图需要屏幕录制授权"))
            }
        }
        if preset == .full {
            media.callRecordingReminders = true
            if state(.microphone) != .granted { skipped.append(L("录音需要麦克风授权")) }
        }
        // The setters report refusals through the session's notice line; the guide lists them instead.
        if session.error != nil { session.dismissError() }
    }

    // MARK: Local model servers

    struct LocalServer: Identifiable, Equatable {
        let name: String
        let url: String
        let path: String
        var models: Int?
        var found: Bool?
        var id: String { name }

        static let known = [LocalServer(name: "LM Studio", url: "http://localhost:1234/v1", path: "/models"),
                            LocalServer(name: "Ollama", url: "http://localhost:11434", path: "/api/tags")]
    }

    /// Asks the default local ports for their model lists; nothing is saved.
    func detectServers() async {
        detecting = true
        defer { detecting = false }
        var result: [LocalServer] = []
        for var server in LocalServer.known {
            let (found, count) = await Self.probe(server.url + server.path)
            server.found = found; server.models = count
            result.append(server)
        }
        servers = result
    }

    nonisolated static func probe(_ address: String, timeout: TimeInterval = 1.5) async -> (Bool, Int?) {
        guard let url = URL(string: address) else { return (false, nil) }
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = timeout; config.connectionProxyDictionary = [:]
        var request = URLRequest(url: url); request.timeoutInterval = timeout
        guard let (data, response) = try? await URLSession(configuration: config).data(for: request),
              response is HTTPURLResponse else { return (false, nil) }
        let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        let list = (object?["data"] ?? object?["models"]) as? [Any]
        return (true, list?.count)
    }

    // MARK: Claude Code and Codex

    enum AgentTarget: String, CaseIterable, Identifiable {
        case claudeCode, codex
        var id: String { rawValue }
        var name: String { self == .claudeCode ? "Claude Code" : "Codex" }
    }
    enum AgentOutcome: Equatable { case copied, written(String), failed(String) }

    /// One token per tool for this run of the guide, registered like 设置 → 外部访问 does.
    private func setup(_ target: AgentTarget) async throws -> TimelineService.ClientSetup {
        if let cached = setups[target] { return cached }
        let setup = try await TimelineService.shared.addClient(name: target.name)
        setups[target] = setup
        return setup
    }

    func copy(_ target: AgentTarget) async {
        do {
            let setup = try await setup(target)
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(target == .claudeCode ? setup.claudeCode : setup.codex, forType: .string)
            agentResult[target] = .copied
        } catch { agentResult[target] = .failed(error.localizedDescription) }
    }

    func write(_ target: AgentTarget) async {
        do {
            let setup = try await setup(target)
            switch target {
            case .claudeCode:
                // Replace an earlier entry of ours, then add; a login shell finds `claude` wherever it was installed.
                let script = "claude mcp remove candid --scope user >/dev/null 2>&1; " + setup.claudeCode
                let (status, output) = await Self.run(script)
                agentResult[target] = status == 0 ? .written(L("已加入 Claude Code（用户级）"))
                    : .failed(output.contains("not found") ? L("没有找到 claude 命令，请复制命令后在终端运行。") : output)
            case .codex:
                let path = try Self.writeCodexConfig(setup.codex)
                agentResult[target] = .written(L("已写入 %@", path))
            }
        } catch { agentResult[target] = .failed(error.localizedDescription) }
    }

    nonisolated static func run(_ script: String) async -> (Int32, String) {
        await Task.detached {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/bin/zsh")
            process.arguments = ["-lc", script]
            let pipe = Pipe()
            process.standardOutput = pipe; process.standardError = pipe
            do { try process.run() } catch { return (-1, error.localizedDescription) }
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            let text = String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            return (process.terminationStatus, String(text.suffix(300)))
        }.value
    }

    /// Puts the `[mcp_servers.candid]` table into `~/.codex/config.toml`, replacing an earlier one and keeping a
    /// `.bak` copy of the previous file.
    nonisolated static func writeCodexConfig(_ snippet: String, home: URL = FileManager.default.homeDirectoryForCurrentUser) throws -> String {
        let directory = home.appendingPathComponent(".codex", isDirectory: true)
        let file = directory.appendingPathComponent("config.toml")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let existing = (try? String(contentsOf: file, encoding: .utf8)) ?? ""
        if !existing.isEmpty { try existing.write(to: file.appendingPathExtension("bak"), atomically: true, encoding: .utf8) }
        try mergeCodexConfig(existing, snippet).write(to: file, atomically: true, encoding: .utf8)
        return "~/.codex/config.toml"
    }

    nonisolated static func mergeCodexConfig(_ existing: String, _ snippet: String) -> String {
        var kept: [Substring] = []
        var skipping = false
        for line in existing.split(separator: "\n", omittingEmptySubsequences: false) {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("[") {
                skipping = trimmed == "[mcp_servers.candid]" || trimmed.hasPrefix("[mcp_servers.candid.")
            }
            if !skipping { kept.append(line) }
        }
        var text = kept.joined(separator: "\n").trimmingCharacters(in: .newlines)
        if !text.isEmpty { text += "\n\n" }
        return text + snippet + "\n"
    }
}
