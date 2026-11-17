import Foundation
import CandidLocalization
import Observation

@MainActor @Observable
public final class ConsoleStore {
    public enum Connection: String, Sendable { case connecting = "正在连接", online = "服务在线", offline = "服务离线", reconnecting = "连接中断，正在重试" }
    public private(set) var connection: Connection = .connecting
    public private(set) var snapshot: BenchState?
    public private(set) var lastUpdated: Date?
    public private(set) var error: String?
    public private(set) var busy: String?
    public private(set) var notice: String?
    public private(set) var maxSeconds = 15
    public var selectedAssetID: String?
    public var transport: Transport = .serial
    public var codec = "wav"
    public var bitrate = 24000
    public var seconds = 15
    public private(set) var service: any ConsoleService
    public private(set) var connectionRevision = UUID()
    @ObservationIgnored private var lastEventRefresh = Date.distantPast
    @ObservationIgnored private var observationID: UUID?
    public init(service: any ConsoleService) { self.service = service }
    public var device: DeviceSnapshot? { snapshot?.device(for: transport) }
    public var recordings: [AudioAsset] { snapshot?.audioAssets ?? [] }
    public var selectedAsset: AudioAsset? { recordings.first { $0.id == selectedAssetID } ?? recordings.first }
    public var recordingState: String { device?.status.recording?.state ?? "unknown" }
    public var receiving: Bool { device?.transfer?.state == "receiving" }
    public var settingsLocked: Bool { busy != nil || receiving || ["recording", "encoding"].contains(recordingState) || device?.status.vad?.auto_armed == true }
    public var canStart: Bool {
        connection == .online && device?.connected == true && device?.status.capabilities?["audio"] == true && !settingsLocked && (codec != "opus" || device?.status.capabilities?["opus"] == true)
    }
    public var canStop: Bool {
        connection == .online && device?.connected == true && busy == nil && !receiving && ["recording", "ready"].contains(recordingState)
    }
    public var canTranscribe: Bool { connection == .online && selectedAsset != nil && snapshot?.asr.busy == false && busy == nil }
    public func dismissError() { error = nil }
    public func dismissNotice() { notice = nil }
    public func refresh() async {
        let generation = connectionRevision
        let service = self.service
        do {
            let config = try await service.configure()
            let state = try await service.state()
            guard generation == connectionRevision else { return }
            maxSeconds = max(1, min(15, config.max_record_seconds)); seconds = min(seconds, maxSeconds)
            apply(state); connection = .online; error = nil
        }
        catch {
            guard generation == connectionRevision else { return }
            connection = .offline; self.error = error.localizedDescription
        }
    }
    public func run() async {
        let generation = connectionRevision
        let observation = UUID(); observationID = observation
        connection = .connecting
        defer {
            if observationID == observation, generation == connectionRevision {
                observationID = nil; connection = .offline
            }
        }
        let service = self.service
        var delay: UInt64 = 1
        while !Task.isCancelled {
            do {
                let config = try await service.configure()
                let state = try await service.state()
                guard generation == connectionRevision, !Task.isCancelled else { return }
                maxSeconds = max(1, min(15, config.max_record_seconds)); seconds = min(seconds, maxSeconds)
                apply(state); connection = .online; error = nil
                let messages = await service.messages()
                delay = 1
                for try await message in messages {
                    try Task.checkCancellation()
                    guard generation == connectionRevision else { return }
                    if let state = message.state { apply(state) }
                    if message.type == "audio.levels", let frame = message.frame, var state = snapshot {
                        var frames = state.live_audio?.frames ?? []
                        if frames.last?.boot_id != frame.boot_id || (frames.last?.start_sample ?? -1) >= frame.start_sample { frames = [] }
                        frames.append(frame)
                        state.live_audio = LiveAudio(frames: Array(frames.suffix(150)), gap_samples: message.gap_samples ?? 0)
                        if let device = message.serial_device { state.serial_device = device }
                        apply(state)
                    }
                    if message.type == "event", Date().timeIntervalSince(lastEventRefresh) > 0.75 {
                        lastEventRefresh = Date()
                        let state = try await service.state()
                        guard generation == connectionRevision else { return }
                        apply(state)
                    }
                    connection = .online
                }
                if !Task.isCancelled { connection = .reconnecting }
            } catch {
                guard !Task.isCancelled, generation == connectionRevision else { break }
                connection = snapshot == nil ? .offline : .reconnecting
                self.error = L("本机服务连接失败：%@", error.localizedDescription)
            }
            do { try await Task.sleep(nanoseconds: delay * 1_000_000_000) } catch { break }
            delay = min(delay * 2, 8)
        }
    }
    public var canChangeConnection: Bool {
        busy == nil && !receiving && !["recording", "encoding"].contains(recordingState) && device?.status.vad?.enabled != true
    }
    public func changeConnection(endpoint: String, mock: Bool) throws {
        guard canChangeConnection else { throw ConsoleError.rejected(L("请先结束当前录音、接收或 VAD 监听，再切换连接。")) }
        let next: any ConsoleService = mock ? MockService() : try DebugClient(endpoint: endpoint.trimmingCharacters(in: .whitespacesAndNewlines))
        service = next; snapshot = nil; selectedAssetID = nil; lastUpdated = nil
        connection = .connecting; error = nil; notice = nil; lastEventRefresh = .distantPast
        connectionRevision = UUID()
    }
    public func reportError(_ message: String) { error = message }
    public func importAudio(from url: URL) async {
        guard connection == .online, busy == nil else { return }
        await perform(L("正在导入录音")) {
            let granted = url.startAccessingSecurityScopedResource()
            defer { if granted { url.stopAccessingSecurityScopedResource() } }
            let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
            guard size > 0, size <= 20 * 1024 * 1024, url.pathExtension.lowercased() == "wav" else {
                throw ConsoleError.rejected(L("请选择不超过 20 MiB 的 WAV 文件"))
            }
            let data = try await Task.detached(priority: .userInitiated) { try Data(contentsOf: url) }.value
            let asset = try await self.service.importAudio(data: data, filename: url.lastPathComponent)
            self.apply(try await self.service.state()); self.selectedAssetID = asset.id
            self.notice = L("已导入 %@，可在录音库中查看", asset.filename)
        }
    }
    private func apply(_ state: BenchState) {
        snapshot = state; lastUpdated = Date()
        if !recordings.contains(where: { $0.id == selectedAssetID }) { selectedAssetID = recordings.first?.id }
    }
    public func command(_ name: String) async {
        guard ["record.start", "record.stop"].contains(name) else { return }
        guard name == "record.start" ? canStart : canStop else { return }
        await perform(name == "record.start" ? L("正在请求录音") : L("正在停止并接收录音")) {
            let result = try await self.service.command(CommandRequest(transport: self.transport, command: name, codec: self.codec, bitrate: self.bitrate, seconds: self.seconds))
            self.notice = result.state == "verified" ? L("电脑已接收并校验录音") : result.state == "encoding" ? L("采样已停止，设备正在编码") : L("设备已确认开始录音")
            let state = try await self.service.state(); self.apply(state)
            if let asset = result.asset { self.selectedAssetID = asset.id }
        }
    }
    public func toggleMonitor() async {
        guard connection == .online, device?.connected == true, busy == nil else { return }
        let enabled = device?.status.vad?.enabled == true
        guard device?.status.vad?.mode != "auto" || !enabled else {
            error = L("服务当前处于自动触发模式；请在原验收台停止并保存，原生控制台仅控制 monitor。"); return
        }
        guard enabled || (!settingsLocked && device?.status.capabilities?["vad"] == true) else { return }
        await perform(enabled ? L("正在停用 VAD") : L("正在开启 VAD monitor")) {
            if enabled && self.transport == .serial { try await self.service.pauseMonitor() }
            else {
                _ = try await self.service.command(CommandRequest(transport: self.transport, command: enabled ? "vad.stop" : "vad.start", codec: self.codec, bitrate: self.bitrate, seconds: self.seconds))
            }
            self.apply(try await self.service.state())
            self.notice = enabled ? L("VAD 已停用；录音状态独立显示") : L("VAD monitor 已启用；手动录音由你控制")
        }
    }
    public func transcribe() async {
        guard canTranscribe, let asset = selectedAsset else { return }
        await perform(L("正在请求本机转写")) {
            try await self.service.transcribe(assetID: asset.id)
            self.apply(try await self.service.state()); self.notice = L("已提交本机转写，请等待结果")
        }
    }
    private func perform(_ label: String, action: () async throws -> Void) async {
        busy = label; error = nil; notice = nil
        defer { busy = nil }
        do { try await action() }
        catch {
            let message = L("%@ 操作未自动重发；请先刷新状态。", error.localizedDescription)
            await refresh()
            self.error = message
        }
    }
    public func freshFrame(at now: Date) -> LevelFrame? {
        guard connection == .online, transport == .serial, device?.connected == true,
              let frame = snapshot?.live_audio?.frames.last, let received = frame.receivedDate,
              now.timeIntervalSince(received) >= -1, now.timeIntervalSince(received) < 1.5,
              frame.device_id == device?.status.device_id else { return nil }
        return frame
    }
    public func vadLabel(at now: Date) -> String {
        guard connection == .online else { return L("状态已过期") }
        guard device?.connected == true else { return L("设备未连接") }
        guard let vad = device?.status.vad else { return L("设备未报告 VAD") }
        guard vad.enabled else { return L("VAD 未启用") }
        if transport == .http { return L("设备报告：%@（非实时采样）", vad.state) }
        guard let frame = freshFrame(at: now) else { return L("等待新鲜采样") }
        switch frame.vad_state {
        case "speech": return L("检测到话音")
        case "silence": return L("未检测到话音")
        case "error": return L("VAD 检测错误")
        default: return L("VAD 未启用")
        }
    }
}
