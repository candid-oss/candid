import AppKit
import CandidLocalization
import DesktopCore
import SwiftUI

/// Something in the menu bar panel that needs the user's attention.
struct StatusAlert: Identifiable, Equatable {
    enum Action: Equatable { case grantScreen, grantAccessibility, saveDraft, discardDraft, dismissError, openApp, raiseStorageLimit }
    let id: String
    let symbol: String
    let text: String
    var actions: [(title: String, action: Action)] = []
    static func == (lhs: StatusAlert, rhs: StatusAlert) -> Bool { lhs.id == rhs.id && lhs.text == rhs.text }
}

@MainActor enum StatusAlerts {
    static func current(session: DesktopSession, media: MediaWorkspaceStore) -> [StatusAlert] {
        var alerts: [StatusAlert] = []
        if let error = session.error { alerts.append(.init(id: "error", symbol: "exclamationmark.triangle.fill", text: error, actions: [(L("知道了"), .dismissError)])) }
        if let error = media.error { alerts.append(.init(id: "media-error", symbol: "exclamationmark.triangle.fill", text: error, actions: [(L("知道了"), .dismissError)])) }
        if let draft = media.draft {
            alerts.append(.init(id: "draft", symbol: draft.kind == "audio" ? "waveform" : "photo",
                                text: draft.kind == "audio" ? L("录音已就绪，等你保存") : L("截图已就绪，等你保存"),
                                actions: [(L("保存"), .saveDraft), (L("丢弃"), .discardDraft)]))
        }
        if !session.isDemo, let alert = storage(session.storageBudget) { alerts.append(alert) }
        if !session.isDemo, session.screenshotsEnabled, !session.screenPermission {
            alerts.append(.init(id: "screen", symbol: "rectangle.dashed.badge.record", text: L("窗口截图需要屏幕录制授权"), actions: [(L("授权"), .grantScreen)]))
        }
        if !session.isDemo, session.textEnabled, !session.accessibilityPermission {
            alerts.append(.init(id: "ax", symbol: "accessibility", text: L("读取文字需要辅助功能授权"), actions: [(L("授权"), .grantAccessibility)]))
        }
        return alerts
    }

    /// The screenshot storage limit at ≥90% or full, with a way to raise it.
    static func storage(_ budget: DesktopStorageBudget) -> StatusAlert? {
        guard let text = budget.message else { return nil }
        let symbol = budget.level == .full ? "externaldrive.fill.badge.exclamationmark" : "externaldrive.badge.exclamationmark"
        let actions: [(title: String, action: StatusAlert.Action)] = budget.raisedLimit.map {
            [(L("上限提高到 %@", ByteCountFormatter.string(fromByteCount: $0, countStyle: .file)), .raiseStorageLimit)]
        } ?? []
        return StatusAlert(id: "storage", symbol: symbol, text: text, actions: actions)
    }
}

/// Menu bar icon: state at a glance, with a live timer while the microphone records.
struct MenuBarLabel: View {
    let session: DesktopSession
    let media: MediaWorkspaceStore

    var body: some View {
        let alerts = StatusAlerts.current(session: session, media: media)
        HStack(spacing: 3) {
            Image(systemName: symbol(alerting: !alerts.isEmpty))
            // Plain text only: a self-updating timer here leaks (see recordingClock).
            if let clock = media.recordingClock {
                Text(clock).monospacedDigit()
            }
        }
        .accessibilityLabel(media.isRecording ? L("Candid 正在录音") : session.isRecording ? L("Candid 正在记录") : L("Candid 已暂停"))
    }

    private func symbol(alerting: Bool) -> String { Self.symbol(session: session, media: media, alerting: alerting) }

    static func symbol(session: DesktopSession, media: MediaWorkspaceStore, alerting: Bool) -> String {
        if media.isRecording { return "mic.fill" }
        if alerting { return session.isRecording ? "record.circle.fill" : "exclamationmark.circle" }
        return session.isRecording ? "record.circle" : "pause.circle"
    }
}

/// The panel under the menu bar icon: what needs attention, one-tap capture,
/// and the last few things recorded today.
struct MenuBarPanel: View {
    @Bindable var session: DesktopSession
    let media: MediaWorkspaceStore
    /// Shows the main window and closes the panel (the panel lives in a popover, outside any SwiftUI scene).
    var openApp: () -> Void = {}

    var body: some View {
        let alerts = StatusAlerts.current(session: session, media: media)
        VStack(alignment: .leading, spacing: 14) {
            header
            if !alerts.isEmpty {
                VStack(spacing: 6) { ForEach(alerts) { alert($0) } }
            }
            HealthProblemsCard(excluding: Set(alerts.map(\.id)), openApp: openMain)
            if media.isRecording { recording.transition(.move(edge: .top).combined(with: .opacity)) }
            controls
            today
            HairlineDivider()
            HStack {
                Button(L("打开 Candid")) { openMain() }.buttonStyle(.borderless)
                Spacer()
                Button { openMain(); OnboardingLinks.showGuide() } label: { Image(systemName: "questionmark.circle") }.buttonStyle(.borderless).help(L("打开引导"))
                SettingsLink { Image(systemName: "gearshape") }.buttonStyle(.borderless).help(L("设置"))
                Button { NSApp.terminate(nil) } label: { Image(systemName: "power") }.buttonStyle(.borderless).help(L("退出 Candid"))
            }
            .typography(.secondary)
        }
        .padding(16)
        .frame(width: 330)
        .animation(Motion.snappy, value: alerts)
        .animation(Motion.snappy, value: media.isRecording)
    }

    private var header: some View {
        HStack(spacing: 10) {
            Image(nsImage: NSApp.applicationIconImage).resizable().frame(width: 30, height: 30)
            VStack(alignment: .leading, spacing: 2) {
                Text("Candid").typography(.headline)
                HStack(spacing: 5) {
                    Circle().fill(session.isRecording ? Color.red : Color.secondary).frame(width: 6, height: 6)
                    Text(statusLine).typography(.secondary).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            Spacer()
        }
    }

    /// Live proof the microphone is picking something up.
    private var recording: some View {
        HStack(spacing: 14) {
            RecordingOrb(level: media.recordingLevels.last ?? 0, quiet: media.microphoneQuiet).frame(width: 40, height: 40)
            VStack(alignment: .leading, spacing: 6) {
                HStack {
                    Text(media.microphoneQuiet ? L("没有听到声音") : media.activeRecordingInput.label).typography(.body, .semibold)
                    Spacer()
                    if let started = media.recordingStartedAt {
                        Text(started, style: .timer).typography(.secondary, .medium, digits: true).foregroundStyle(.secondary)
                    }
                }
                LiveWaveform(levels: media.recordingLevels, color: .red, quiet: media.microphoneQuiet).frame(height: 18)
                    .onAppear { media.levelViewers += 1 }.onDisappear { media.levelViewers -= 1 }
                if media.microphoneQuiet {
                    Text(L("检查麦克风是否静音，或在系统设置的声音里选对输入设备。")).typography(.caption).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
        .padding(12)
        .background(RoundedRectangle(cornerRadius: Radius.large, style: .continuous).fill(Color.red.opacity(0.07)))
        .animation(Motion.snappy, value: media.microphoneQuiet)
    }

    private var statusLine: String {
        if session.isDemo { return L("合成样例") }
        var parts = [session.isRecording ? L("正在记录") : L("已暂停")]
        if !session.isRecording, let resume = RecordingControl.shared.resumeAt { parts.append(L("%@ 自动继续", resume.clockText)) }
        if let saved = session.lastSaved { parts.append(L("最近保存 ") + saved.clockText) }
        return parts.joined(separator: " · ")
    }

    private func alert(_ alert: StatusAlert) -> some View {
        HStack(alignment: .top, spacing: 9) {
            Image(systemName: alert.symbol).typography(.secondary, .semibold).foregroundStyle(.orange).frame(width: 16)
            Text(alert.text).typography(.secondary).lineLimit(3).frame(maxWidth: .infinity, alignment: .leading)
            ForEach(alert.actions, id: \.title) { item in
                Button(item.title) { perform(item.action) }.buttonStyle(.borderless).typography(.secondary, .semibold)
            }
        }
        .padding(10)
        .background(Color.orange.opacity(0.1), in: RoundedRectangle(cornerRadius: Radius.medium, style: .continuous))
        .transition(.opacity.combined(with: .move(edge: .top)))
    }

    private var controls: some View {
        HStack(spacing: 0) {
            control(session.isRecording ? "pause.fill" : "record.circle", session.isRecording ? L("暂停") : L("记录"),
                    tint: session.isRecording ? .red : .accentColor, disabled: session.isDemo) {
                session.isRecording ? session.pauseRecording() : session.startRecording()
            }
            control("camera", L("截窗"), disabled: !session.isRecording || !session.screenshotsEnabled || session.captureBusy || session.isDemo,
                    action: session.captureNow)
            control("viewfinder", L("框选"), disabled: !media.canCapture) { Task { await media.captureRegion() } }
            if media.isRecording {
                control("stop.fill", L("停止"), tint: .red, disabled: media.recordingFinishing) { Task { await media.stopRecording() } }
            } else {
                control("mic", L("录音"), disabled: !media.canCapture) { Task { await media.startRecording() } }
            }
            control("bookmark", L("标记"), disabled: false, action: session.addMarker)
        }
    }

    private func control(_ symbol: String, _ title: String, tint: Color = .primary, disabled: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            VStack(spacing: 5) {
                Image(systemName: symbol).typography(.section, .medium).foregroundStyle(tint)
                    .frame(width: 42, height: 42)
                    .background(Circle().fill(tint == .primary ? Surface.hover.color : tint.opacity(0.14)))
                Text(title).typography(.caption)
            }
            .frame(maxWidth: .infinity)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(disabled)
        .opacity(disabled ? 0.4 : 1)
    }

    @ViewBuilder private var today: some View {
        let recent = Array(session.timelineEvents.filter { $0.record.kind != .system && $0.record.kind != .gap }.prefix(3))
        if !recent.isEmpty {
            VStack(alignment: .leading, spacing: 8) {
                Text(L("最近")).typography(.caption, .semibold).foregroundStyle(.secondary)
                ForEach(recent) { event in
                    Button { openMain() } label: {
                        HStack(spacing: 9) {
                            DesktopApplicationIcon(bundleID: event.record.bundleID, fallback: event.record.kind.symbol, size: 20)
                            Text(event.latestRecord.displayTitle).typography(.secondary).lineLimit(1)
                            Spacer()
                            Text(event.startDate.clockText).typography(.caption, digits: true).foregroundStyle(.secondary)
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
        }
    }

    private func perform(_ action: StatusAlert.Action) {
        switch action {
        case .grantScreen: session.requestScreenPermission()
        case .grantAccessibility: session.requestAccessibilityPermission()
        case .saveDraft: Task { await media.saveDraft() }
        case .discardDraft: media.discardDraft()
        case .dismissError: session.dismissError(); media.error = nil
        case .openApp: openMain()
        case .raiseStorageLimit: session.raiseStorageLimit()
        }
    }

    private func openMain() { openApp() }
}
