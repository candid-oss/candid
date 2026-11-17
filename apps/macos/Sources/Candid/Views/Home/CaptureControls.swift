import SwiftUI
import CandidLocalization

/// Direct capture controls, always in reach: start/pause recording, one window
/// screenshot, one region screenshot, one microphone recording, a marker.
/// Each control is disabled with its reason instead of silently failing.
struct CaptureControls: View {
    @Bindable var session: DesktopSession
    let media: MediaWorkspaceStore
    let openSettings: () -> Void
    @ViewState private var showingSources = false

    var body: some View {
        HStack(spacing: 2) {
            recordToggle
            Rectangle().fill(Surface.hairline).frame(width: Surface.hairlineWidth, height: 16).padding(.horizontal, 3)
            windowShot
            ControlIcon(systemImage: "viewfinder", help: regionHelp, disabled: !media.canCapture) {
                Task { await media.captureRegion() }
            }
            microphone
            ControlIcon(systemImage: "bookmark", help: L("标记此刻（⇧⌘M）"), disabled: false, action: session.addMarker)
            ControlIcon(systemImage: "chevron.down", help: L("记录来源"), disabled: false, size: 10) { showingSources.toggle() }
                .popover(isPresented: $showingSources, arrowEdge: .bottom) { sources }
        }
        .padding(3)
        .background(.ultraThinMaterial, in: Capsule())
        .overlay(Capsule().strokeBorder(Surface.hover.color, lineWidth: 0.5))
        .animation(Motion.bouncy, value: session.isRecording)
        .animation(Motion.bouncy, value: media.isRecording)
    }

    // MARK: Pieces

    private var recordToggle: some View {
        Button {
            session.isRecording ? session.pauseRecording() : session.startRecording()
        } label: {
            HStack(spacing: 7) {
                Circle().fill(session.isRecording ? Color.red : Color.secondary.opacity(0.6)).frame(width: 7, height: 7)
                    .phaseAnimator([false, true], trigger: session.isRecording) { dot, phase in
                        dot.opacity(session.isRecording && phase ? 0.45 : 1)
                    } animation: { _ in .easeInOut(duration: 1.6) }
                Text(session.isDemo ? L("合成样例") : session.isRecording ? L("记录中") : L("开始记录"))
                    .typography(.secondary, .semibold)
                    .fixedSize()
                    .contentTransition(.opacity)
            }
            .padding(.horizontal, 11).frame(height: 26)
            .background(Capsule().fill(session.isRecording ? Color.red.opacity(0.12) : Surface.hover.color))
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .disabled(session.isDemo)
        .help(session.isRecording ? L("暂停电脑记录") : L("开始电脑记录"))
        .accessibilityLabel(session.isRecording ? L("暂停电脑记录") : L("开始电脑记录"))
    }

    @ViewBuilder private var windowShot: some View {
        if session.captureBusy {
            ProgressView().controlSize(.small).frame(width: 28, height: 26).help(L("正在截图"))
        } else if !session.screenPermission || !session.screenshotsEnabled || session.allowedBundleIDs.isEmpty {
            // Not set up yet: the button leads to the one place that sets it up.
            ControlIcon(systemImage: "camera", help: L("设置窗口截图"), disabled: session.isDemo, dimmed: true, action: openSettings)
        } else {
            ControlIcon(systemImage: "camera", help: session.isRecording ? L("3 秒后截取允许的窗口") : L("开始记录后可以截取窗口"),
                        disabled: !session.isRecording || session.isDemo, action: session.captureNow)
        }
    }

    @ViewBuilder private var microphone: some View {
        if let started = media.recordingStartedAt {
            Button { Task { await media.stopRecording() } } label: {
                HStack(spacing: 7) {
                    Image(systemName: "stop.fill").typography(.caption, .bold)
                    LiveWaveform(levels: media.recordingLevels.suffix(12), color: .white, quiet: media.microphoneQuiet)
                        .frame(width: 34, height: 14)
                    Text(started, style: .timer).typography(.secondary, .semibold, digits: true)
                }
                .foregroundStyle(.white)
                .padding(.horizontal, 10).frame(height: 26)
                .background(Capsule().fill(Color.red))
                .contentShape(Capsule())
            }
            .buttonStyle(.plain)
            .disabled(media.recordingFinishing)
            .help(media.microphoneQuiet ? L("麦克风几秒没有收到声音。点按停止录音") : L("正在录制%@，点按停止", media.activeRecordingInput.label))
            .transition(.scale(scale: 0.7).combined(with: .opacity))
        } else {
            ControlIcon(systemImage: media.recordingInput == .microphone ? "mic" : "waveform.badge.mic", help: media.canCapture ? L("开始%@录音，每 %@ 秒自动保存到本机资料库", media.recordingInput.label, media.recordingSegmentSeconds) : L("先保存或丢弃上一份素材"), disabled: !media.canCapture) {
                Task { await media.startRecording() }
            }
            .transition(.scale(scale: 0.7).combined(with: .opacity))
        }
    }

    private var regionHelp: String { media.canCapture ? L("框选屏幕的一块") : L("先保存或丢弃上一份素材") }

    private var sources: some View {
        VStack(alignment: .leading, spacing: 4) {
            Toggle(isOn: Binding(get: { session.isRecording }, set: { $0 ? session.startRecording() : session.pauseRecording() })) {
                VStack(alignment: .leading, spacing: 1) {
                    Text(L("电脑活动"))
                    Text(session.statusText).typography(.caption).foregroundStyle(.secondary)
                }
            }
            .disabled(session.isDemo)
            Toggle(isOn: Binding(get: { session.screenshotsEnabled }, set: session.setScreenshotsEnabled)) {
                VStack(alignment: .leading, spacing: 1) {
                    Text(L("窗口截图"))
                    Text(session.screenPermission ? L("%@ 个应用", session.allowedBundleIDs.count) : L("需要屏幕录制授权"))
                        .typography(.caption).foregroundStyle(.secondary)
                }
            }
            .disabled(session.isDemo || !session.screenPermission)
            HStack {
                Text(L("录音来源")).typography(.secondary).foregroundStyle(.secondary)
                Spacer()
                MenuChoice(options: RecordingInput.allCases.filter { $0 == .microphone || RecordingInput.systemAvailable }.map { ($0, $0.label) },
                           selection: Binding(get: { media.recordingInput }, set: { media.recordingInput = $0 }))
            }
            .disabled(media.isRecording || media.busy)
            if RecordingInput.systemAvailable {
                Toggle(L("通话时提醒录音"), isOn: Binding(get: { media.callRecordingReminders }, set: { media.callRecordingReminders = $0 }))
                    .disabled(media.isDemo)
                Toggle(L("通话结束自动停止"), isOn: Binding(get: { media.autoStopCallRecording }, set: { media.autoStopCallRecording = $0 }))
                    .disabled(media.isDemo)
            }
            HStack {
                Text(L("录音分段保存")).typography(.secondary).foregroundStyle(.secondary)
                Spacer()
                MenuChoice(options: [(30, L("每 30 秒")), (60, L("每 60 秒"))],
                           selection: Binding(get: { media.recordingSegmentSeconds }, set: { media.recordingSegmentSeconds = $0 }))
            }
            .disabled(media.isRecording || media.busy)
            HairlineDivider().padding(.vertical, 6)
            HStack {
                Text(session.lastSaved.map { L("最近保存 ") + $0.clockText } ?? L("本次还没有保存")).typography(.caption).foregroundStyle(.secondary)
                Spacer()
                QuietButton(title: L("设置…")) { showingSources = false; openSettings() }
            }
        }
        .toggleStyle(.switch)
        .padding(16)
        .frame(width: 290)
    }
}

/// Live input level as mirrored bars, newest on the right. Flat and dimmed
/// when the microphone hears nothing, so a silent recording is obvious.
struct LiveWaveform<Levels: Collection>: View where Levels.Element == Float {
    let levels: Levels
    var color: Color = .accentColor
    var quiet = false

    var body: some View {
        GeometryReader { geo in
            let values = Array(levels)
            let count = max(values.count, 1)
            let spacing: CGFloat = 2
            let width = max(1.5, (geo.size.width - spacing * CGFloat(count - 1)) / CGFloat(count))
            HStack(alignment: .center, spacing: spacing) {
                ForEach(values.indices, id: \.self) { index in
                    Capsule()
                        .fill(color.opacity(quiet ? 0.45 : 0.55 + 0.45 * Double(index + 1) / Double(count)))
                        .frame(width: width, height: max(2, geo.size.height * CGFloat(min(1, values[index] * 1.6))))
                }
            }
            .frame(width: geo.size.width, height: geo.size.height)
            .animation(.linear(duration: 0.08), value: values)
        }
        .accessibilityHidden(true)
    }
}

/// A soft orb that breathes with the input level while recording.
struct RecordingOrb: View {
    let level: Float
    var quiet = false

    var body: some View {
        let energy = CGFloat(min(1, level * 1.8))
        ZStack {
            Circle().fill(Color.red.opacity(quiet ? 0.08 : 0.18)).scaleEffect(1 + energy * 0.55)
            Circle().fill(Color.red.opacity(quiet ? 0.14 : 0.28)).scaleEffect(1 + energy * 0.28)
            Circle()
                .fill(RadialGradient(colors: [Color(red: 1, green: 0.55, blue: 0.5), .red], center: .topLeading, startRadius: 1, endRadius: 30))
                .overlay(Image(systemName: quiet ? "mic.slash.fill" : "mic.fill").typography(.section).foregroundStyle(.white))
                .saturation(quiet ? 0.4 : 1)
        }
        .animation(.interpolatingSpring(stiffness: 260, damping: 18), value: energy)
        .animation(Motion.snappy, value: quiet)
        .accessibilityHidden(true)
    }
}

/// Icon-only capsule button with hover backing.
private struct ControlIcon: View {
    let systemImage: String
    let help: String
    let disabled: Bool
    var dimmed = false
    var size: CGFloat = 13
    let action: () -> Void
    @ViewState private var hovering = false

    var body: some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.system(size: size, weight: .medium))  // style-exempt: glyph scales with its container
                .foregroundStyle(disabled || dimmed ? Color.secondary : Color.primary)
                .frame(width: 28, height: 26)
                .background(Capsule().fill(hovering && !disabled ? Surface.hover.color : .clear))
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .disabled(disabled)
        .opacity(disabled ? 0.45 : 1)
        .onHover { hovering = $0 }
        .animation(Motion.snappy, value: hovering)
        .help(help).accessibilityLabel(help)
    }
}

/// Screenshots and legacy recordings wait for review here. New microphone
/// recordings show their automatic save result wherever the user is.
struct DraftBar: View {
    let media: MediaWorkspaceStore
    let openLibrary: () -> Void

    var body: some View {
        if let draft = media.draft {
            HStack(spacing: 12) {
                Image(systemName: draft.kind == "audio" ? "waveform" : "photo")
                    .typography(.headline).foregroundStyle(.tint)
                VStack(alignment: .leading, spacing: 1) {
                    Text(draft.kind == "audio" ? L("录音已就绪") : L("截图已就绪")).typography(.body, .semibold)
                    Text(draft.filename).typography(.secondary).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer(minLength: 12)
                QuietButton(title: L("查看"), emphasized: true, action: openLibrary)
                QuietButton(title: L("丢弃"), role: .destructive) { media.discardDraft() }
                Button { Task { await media.saveDraft() } } label: {
                    Text(L("保存")).typography(.secondary, .semibold).padding(.horizontal, 12).frame(height: 26)
                        .background(Capsule().fill(Color.accentColor)).foregroundStyle(.white)
                }
                .buttonStyle(.plain).disabled(media.busy)
            }
            .padding(.leading, 14).padding(.trailing, 8).frame(height: 44)
            .frame(maxWidth: 460)
            .background(.regularMaterial, in: Capsule())
            .overlay(Capsule().strokeBorder(Surface.hover.color, lineWidth: 0.5))
            .shadow(color: .black.opacity(0.16), radius: 18, y: 8)
            .transition(.move(edge: .top).combined(with: .opacity))
        } else if let status = media.recordingNotice, !media.isRecording {
            HStack(spacing: Space.s) {
                if media.recordingFinishing { ProgressView().controlSize(.small) }
                else { Image(systemName: media.error == nil ? "checkmark.circle" : "exclamationmark.triangle").foregroundStyle(media.error == nil ? Palette.success : Palette.warning) }
                Text(status).typography(.secondary, .medium)
                QuietButton(title: L("查看资料"), emphasized: true, action: openLibrary)
                if !media.recordingFinishing {
                    QuietButton(title: L("关闭")) { media.recordingNotice = nil }
                }
            }
            .padding(Space.s).floatingSurface(radius: Radius.large)
            .transition(.move(edge: .top).combined(with: .opacity))
        }
    }
}
