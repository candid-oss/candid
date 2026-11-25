import SwiftUI
import CandidLocalization

struct RecordingProcessing: Decodable, Equatable {
    let state: String
    let kind: String
    let reason: String?
    let stage: String?
    let createdMs: Int64
    let updatedMs: Int64
    let stageSeconds: Int?
    let elapsedSeconds: Int
    let completedSeconds: Double?
    let totalSeconds: Double?
    let cancellationSupported: Bool?
    let modelMayContinue: Bool?
    let stopConfirmed: Bool?
    var isActive: Bool { ["queued", "running", "cancelling"].contains(state) }
    var isWaiting: Bool { state == "queued" && reason != nil }
    var revisionKey: String { "\(state):\(stage ?? ""):\(reason ?? ""):\(updatedMs):\(elapsedSeconds):\(completedSeconds ?? 0):\(stopConfirmed == true)" }
    var label: String {
        if state == "cancelling" { return L("正在停止模型计算") }
        if state == "cancelled", reason == "user_cancelled" || reason == nil { return stopConfirmed == false ? L("结果已取消，停止未确认") : L("已停止") }
        if let reason, state != "done" {
            return ["waiting_feature_source":L("此来源未开启该功能"), "waiting_feature_remote":L("此功能仅允许本机模型"), "waiting_feature_budget":L("已到每日调用上限"), "waiting_model":L("尚未选择模型"), "waiting_authorization":L("模型尚未授权"), "waiting_recording_transcript":L("等待转写完成"), "feature_disabled":L("语音识别已关闭"), "offline":L("模型服务离线"), "timeout":L("模型请求超时"), "http_429":L("模型服务繁忙"), "inference_failed":L("模型计算失败"), "cancel_unconfirmed":L("未收到服务停止确认"), "waiting_budget":L("等待预算额度"), "waiting_budget_cost_unknown":L("模型费用配置待确认")][reason] ?? L("处理未完成：%@", reason)
        }
        if state == "failed" { return L("识别失败") }
        if state == "done" { return L("已完成") }
        if state == "queued" { return L("排队中") }
        return ["starting":L("正在启动计算任务"), "decoding":L("正在读取音频"), "loading_model":L("正在加载模型"), "transcribing":L("正在转写"), "voiceprints":L("正在提取声纹"), "speaker_grouping":L("正在整理整条录音的说话人"), "finalizing":L("正在保存结果")][stage ?? ""] ?? L("正在等待模型返回")
    }
    var recoverySettings: Bool { reason?.hasPrefix("waiting_") == true || reason == "feature_disabled" }
}

struct RecordingProcessingView: View {
    let item: MediaLibraryRecord
    let store: MediaWorkspaceStore
    @ViewState private var stopping = false
    private var active: Bool { item.processing?.isActive == true || item.processingStatus.isActive }
    private var retrying: Bool {
        item.processingMode != nil || !item.segments.isEmpty || item.processingStatus == .failed || item.processing?.state == "cancelled"
    }
    var body: some View {
        VStack(alignment: .leading, spacing: Space.s) {
            HStack(spacing: Space.s) {
                Text(item.processing?.kind == "workflow.speakers" ? L("说话人整理") : L("转写")).typography(.headline)
                Text(item.processing?.label ?? item.processingStatus.label).typography(.secondary).foregroundStyle(.secondary)
                    .accessibilityIdentifier("media-recognition-status")
                Spacer()
                if active {
                    if item.processing?.isWaiting != true { ProgressView().controlSize(.mini) }
                    QuietButton(title: stopping ? L("正在停止") : L("停止"), role: .destructive) {
                        stopping = true
                        Task { await store.cancelProcessing(item); stopping = false }
                    }.disabled(stopping || item.processing?.state == "cancelling")
                }
            }
            if let status = item.processing, status.isActive {
                HStack(spacing: Space.s) {
                    Text("\(status.state == "queued" ? L("已等待") : L("已耗时")) \(MediaWorkspaceStore.clockText(elapsed: Double(status.elapsedSeconds)))")
                    if let completed = status.completedSeconds, let total = status.totalSeconds, total > 0 {
                        Text(L("已处理 %@ / %@", mediaTime(completed), mediaTime(total)))
                    }
                    if status.state == "running", let seconds = status.stageSeconds, seconds >= 30 { Text(L("当前阶段 %@", MediaWorkspaceStore.clockText(elapsed: Double(seconds)))) }
                    Spacer()
                    if status.recoverySettings { QuietButton(title: L("设置")) { NotificationCenter.default.post(name: AIFunctionSettingsView.open, object: nil) } }
                }.typography(.caption).foregroundStyle(.secondary)
                if status.state == "running", status.stage == nil {
                    Text(L("模型服务未提供内部进度；可停止接收本次结果。"))
                        .typography(.caption).foregroundStyle(.secondary)
                }
            }
            if let status = item.processing, status.state == "cancelled", status.modelMayContinue == true {
                Text(L("已停止接收结果；该模型服务不支持中断，当前计算可能继续。"))
                    .typography(.caption).foregroundStyle(.secondary)
            }
            HStack(spacing: Space.s) {
                ForEach(MediaLibraryRecord.Mode.allCases) { mode in
                    QuietButton(title: mode.transcriptionTitle(retrying: retrying)) { Task { await store.recognize(item.id, mode: mode) } }
                        .disabled(store.isDemo || store.busy || active)
                        .accessibilityIdentifier("media-recognize-" + mode.rawValue)
                }
                Spacer()
                aiAction(L("AI 重命名"), step: "title")
                aiAction(L("总结"), step: "summary")
            }
            ForEach(["title", "summary"], id: \.self) { step in
                if let status = item.aiProcessing?[step], status.isActive || status.state == "failed" || status.state == "cancelled" {
                    HStack {
                        Text("\(step == "title" ? L("重命名") : L("总结")) · \(status.label)").typography(.caption).foregroundStyle(.secondary)
                        if status.recoverySettings { QuietButton(title: L("设置")) { NotificationCenter.default.post(name: AIFunctionSettingsView.open, object: nil) } }
                        if status.isActive { QuietButton(title: L("停止"), role: .destructive) { Task { await store.cancelProcessing(item) } } }
                    }
                }
            }
            if let summary = item.aiDrafts?["summary"]?.text, !summary.isEmpty {
                Text(summary).typography(.body).textSelection(.enabled)
            }
            if let title = item.aiDrafts?["title"]?.text, title != item.displayTitle, !title.isEmpty {
                HStack { Text(L("建议标题：%@", title)).typography(.secondary); QuietButton(title: L("采用")) { Task { await store.edit(item, title: title, notes: item.notes ?? "") } } }
            }
        }
    }
    private func aiAction(_ title: String, step: String) -> some View {
        QuietButton(title: title) { Task { await store.runAI(item, step: step) } }
            .disabled(store.isDemo || store.busy || item.segments.isEmpty || item.aiProcessing?[step]?.isActive == true)
    }
}
