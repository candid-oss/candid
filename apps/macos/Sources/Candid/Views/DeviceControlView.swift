import SwiftUI
import ConsoleCore

struct DeviceControlView: View {
    @Bindable var store: ConsoleStore
    var body: some View {
        VStack(alignment: .leading, spacing: Space.l) {
            SettingsGroup(title: "设备") {
                HStack(spacing: Space.l) {
                    metric("设备", value: store.device?.status.device_id ?? "未报告")
                    metric("固件", value: store.device?.status.firmware_version ?? "未报告")
                    metric("录音", value: recordingLabel)
                }
                .padding(.vertical, Space.row)
                if let error = store.device?.error { InlineMessage(kind: .error, text: error).padding(.bottom, Space.xs) }
                if let reason = store.device?.status.unavailable?["audio"] { InlineMessage(kind: .note, text: reason).padding(.bottom, Space.xs) }
                if let recording = store.device?.status.recording, let codec = recording.codec {
                    Text("设备报告格式：\(codec.uppercased())\(codec == "opus" ? " · \((recording.bitrate ?? 0) / 1000) kbps" : "")")
                        .typography(.caption, .regular).foregroundStyle(.secondary)
                }
            } accessory: {
                StatusDot(state: store.connection != .online ? .attention : store.device?.connected == true ? .on : .off,
                          text: store.connection != .online ? "状态已过期" : store.device?.connected == true ? "设备已连接" : "等待服务连接设备")
            }
            SettingsGroup(title: "实时采样") {
                TimelineView(.periodic(from: .now, by: 0.25)) { context in
                    telemetry(at: context.date)
                }
                .padding(.top, Space.xs)
            }
            SettingsGroup(title: "录音") {
                SettingRow(title: "连接方式") {
                    MenuChoice(options: Transport.allCases.map { ($0, $0.title) }, selection: $store.transport)
                }
                .disabled(store.settingsLocked)
                HairlineDivider()
                SettingRow(title: "格式") {
                    HStack(spacing: Space.s) {
                        if store.codec == "opus" {
                            MenuChoice(options: [16000, 24000, 32000, 48000].map { ($0, "\($0 / 1000) kbps") }, selection: $store.bitrate)
                        }
                        CapsuleChoice(options: store.device?.status.capabilities?["opus"] == true ? [("wav", "WAV 原音"), ("opus", "Opus 压缩")] : [("wav", "WAV 原音")],
                                      selection: $store.codec)
                    }
                }
                .disabled(store.settingsLocked)
                HairlineDivider()
                SettingRow(title: "时长上限") {
                    MenuChoice(options: Array(Set([3, 5, 10, store.maxSeconds].filter { $0 <= store.maxSeconds })).sorted().map { ($0, "\($0) 秒") },
                               selection: $store.seconds)
                }
                .disabled(store.settingsLocked)
                HStack(spacing: 6) {
                    PrimaryButton(title: "开始录音") { Task { await store.command("record.start") } }
                        .disabled(!store.canStart).opacity(store.canStart ? 1 : 0.45)
                        .accessibilityIdentifier("record.start")
                    QuietButton(title: "停止并保存") { Task { await store.command("record.stop") } }
                        .disabled(!store.canStop).accessibilityIdentifier("record.stop")
                    Spacer()
                    QuietButton(title: store.device?.status.vad?.enabled == true ? "停止语音监听" : "开启语音监听") {
                        Task { await store.toggleMonitor() }
                    }
                    .disabled(!monitorAvailable).help("VAD 监听与手动录音独立；开启后持续监看，须主动停用")
                    .accessibilityIdentifier("vad.toggle")
                }
                .padding(.top, Space.s)
                if let busy = store.busy {
                    HStack(spacing: Space.xs) { ProgressView().controlSize(.small); Text(busy).typography(.secondary) }.padding(.top, Space.xs)
                } else if let notice = store.notice {
                    InlineMessage(kind: .note, text: notice).padding(.top, Space.xs)
                }
                if let transfer = store.device?.transfer, transfer.state == "receiving" {
                    ProgressView("正在接收 \(transfer.received) / \(transfer.total) 字节", value: Double(transfer.received), total: Double(max(1, transfer.total)))
                        .typography(.secondary).padding(.top, Space.xs)
                }
                if store.device?.status.vad?.enabled == true {
                    InlineMessage(kind: .note, text: "退出应用不会停止设备的语音监听。").padding(.top, Space.xs)
                }
            }
        }
    }
    private var monitorAvailable: Bool {
        guard store.connection == .online, store.device?.connected == true, store.busy == nil else { return false }
        if store.device?.status.vad?.enabled == true {
            return store.device?.status.vad?.mode == "monitor" && (store.transport == .serial || !store.receiving)
        }
        return store.device?.status.capabilities?["vad"] == true && !store.settingsLocked
    }
    private var recordingLabel: String {
        if store.connection != .online { return "状态已过期" }
        if store.receiving { return "正在接收文件" }
        switch store.recordingState {
        case "recording": return "正在录音"
        case "encoding": return "正在编码"
        case "ready": return "采样结束 · 媒体就绪"
        case "idle": return "空闲"
        default: return store.recordingState == "unknown" ? "未报告" : store.recordingState
        }
    }
    private func metric(_ title: String, value: String) -> some View {
        VStack(alignment: .leading, spacing: Space.xxs) {
            Text(title).typography(.secondary).foregroundStyle(.secondary)
            Text(value).typography(.body, .medium).textSelection(.enabled)
        }.frame(maxWidth: .infinity, alignment: .leading)
    }
    private func telemetry(at now: Date) -> some View {
        let frame = store.freshFrame(at: now)
        return VStack(alignment: .leading, spacing: 10) {
            HStack {
                StatusDot(state: frame?.vad_state == "speech" ? .on : .off, text: store.vadLabel(at: now))
                Spacer()
                Text(frame.map { String(format: "%.1f dBFS · 削波 %d", $0.rms_dbfs, $0.clipped_samples) } ?? "无新鲜采样")
                    .typography(.caption, .regular, digits: true).foregroundStyle(.secondary)
            }
            if let vad = store.device?.status.vad {
                Text("\(vad.engine ?? "设备 VAD") · \(vad.mode == "auto" ? "自动触发模式，请在原验收台管理" : "monitor，手动控制录音")")
                    .typography(.caption).foregroundStyle(.secondary)
            }
            WaveformView(frames: store.transport == .serial ? store.snapshot?.live_audio?.frames ?? [] : [], fresh: frame != nil)
                .frame(height: 100)
            if store.recordingState == "recording" {
                if let start = store.device?.status.recording?.started_monotonic_ms, let frame {
                    let maximum = Double(store.device?.status.recording?.max_seconds ?? store.seconds)
                    let elapsed = min(maximum, max(0, (frame.monotonic_ms - start) / 1000))
                    ProgressView(value: elapsed, total: maximum).tint(maximum - elapsed <= 3 ? Palette.warning : Palette.accent)
                    Text(String(format: "已录 %.1f 秒 / %.0f 秒 · 剩余约 %.1f 秒", elapsed, maximum, maximum - elapsed))
                        .typography(.secondary, digits: true)
                } else {
                    InlineMessage(kind: .warning, text: "设备正在录音，当前无新鲜计时信息；固件按时长上限停止。")
                }
            }
            if store.device?.status.recording?.stop_reason == "max_duration" {
                InlineMessage(kind: .warning, text: "本段达到时长上限，句尾可能截断，请回听核对。")
            }
        }
    }
}
