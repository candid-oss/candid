import Foundation
import CandidLocalization
import AIProviders
import Observation

@MainActor @Observable final class WorkflowControlModel {
    struct Catalog: Decodable { var workflows: [Definition]; var features: [Feature]? }
    struct Feature: Decodable, Identifiable {
        var id: String; var title: String; var mode: String; var implemented: Bool
        var trigger: String; var role: String; var job_kind: String?
    }
    struct FeaturePolicy: Codable, Equatable {
        var enabled: Bool; var sources: [String]; var allow_remote: Bool; var daily_calls: Int
    }
    struct FeatureSettings: Codable { var version = 1; var items: [String: FeaturePolicy] = [:] }
    struct Definition: Decodable, Identifiable { var id: String; var title: String; var implemented: Bool }
    struct Job: Decodable, Identifiable {
        var id: String; var kind: String; var input_ref: String; var state: String; var error_code: String?
        var title: String { kind == "workflow.speakers" ? L("说话人整理") : kind == "screen.summarize" ? L("屏幕片段摘要") : kind == "agent.skill" ? L("技能运行") : kind == "speech.recognize" ? L("语音转写") : ["workflow.title": L("标题"), "workflow.summary": L("摘要"), "workflow.tags": L("标签"), "workflow.decisions": L("决定与待办")][kind] ?? L("整理") }
        var status: String { if let error_code { return ["feature_disabled": L("功能已关闭"), "waiting_feature_source": L("来源不在功能范围内"), "waiting_feature_remote": L("此功能只用本机模型"), "waiting_feature_budget": L("已到每日调用上限"), "waiting_model_request_rejected": L("摘要请求被模型拒绝，请检查模型设置后重试"), "waiting_model": L("等待选择模型"), "waiting_authorization": L("等待授权"), "waiting_budget_cost_unknown": L("费用未知，等待预算确认"), "offline": L("模型离线"), "timeout": L("请求超时"), "http_429": L("模型繁忙"), "bad_json": L("模型返回格式有误"), "configuration_changed": L("模型配置已改变"), "source_or_configuration_changed": L("来源或配置已改变")][error_code] ?? L("处理未完成，请重试") }; return ["queued": L("排队中"), "running": L("处理中"), "failed": L("失败"), "done": L("完成"), "cancelled": L("已取消")][state] ?? state }
    }
    struct Settings: Codable {
        var version = 1; var automatic = true; var summary = false; var mode = "conversation"
        /// Titles for transcribed recordings; on unless turned off, also for settings saved before it existed.
        var titles = true
        init() {}
        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            version = try c.decodeIfPresent(Int.self, forKey: .version) ?? 1
            automatic = try c.decodeIfPresent(Bool.self, forKey: .automatic) ?? true
            summary = try c.decodeIfPresent(Bool.self, forKey: .summary) ?? false
            mode = try c.decodeIfPresent(String.self, forKey: .mode) ?? "conversation"
            titles = try c.decodeIfPresent(Bool.self, forKey: .titles) ?? true
        }
    }
    struct Snapshot: Decodable { var catalog: Catalog; var settings: Settings?; var jobs: [Job]; var approvals: [String: Bool]?; var features: FeatureSettings? }
    struct Media: Decodable, Identifiable { var id: String; var title: String?; var filename: String; var kind: String; var recording_parts: [Part]?
        struct Part: Decodable { var id: String }
    }
    struct Draft: Decodable { var text: String }
    var remoteProviders: [AIProvider] = []
    var approvals: [String: Bool] = [:]
    var media: [Media] = []
    var selectedID = ""
    var drafts: [String: Draft] = [:]
    var definitions: [Definition] = []
    var jobs: [Job] = []
    var features: [Feature] = []
    var sourceCatalog: [AutomationSource] = []
    var featureSettings = FeatureSettings()
    var saving = false
    var settings = Settings()
    var error: String?
    var loading = false

    func refresh() async {
        guard let channel = TimelineService.shared.channel else { error = L("时间线库尚未打开"); return }
        loading = true; defer { loading = false }
        do {
            let snapshot: Snapshot = try await channel.post("/local/workflows", body: ["action": "status"])
            approvals = snapshot.approvals ?? [:]
            if let config: ProviderConfig = try? await channel.post("/local/models", body: ["action": "get"]) { remoteProviders = config.providers.filter { $0.destination != .onDevice } }
            if let automation: AutomationSnapshot = try? await automationPost(channel, ["action": "status", "include_runs": false]) { sourceCatalog = automation.sources }
            features = snapshot.catalog.features ?? []; featureSettings = snapshot.features ?? FeatureSettings();
            definitions = snapshot.catalog.workflows; jobs = snapshot.jobs; settings = snapshot.settings ?? Settings(); error = nil
            struct MediaList: Decodable { var items: [Media] }
            let list: MediaList = try await channel.post("/local/media", body: ["action": "list"])
            media = list.items
            if !selectedID.isEmpty { drafts = (try? await channel.post("/local/workflows", body: ["action": "drafts", "id": selectedID])) ?? [:] }
        } catch { self.error = error.localizedDescription }
    }
    func setPolicy(_ id: String, _ policy: FeaturePolicy) async {
        guard !saving, let channel = TimelineService.shared.channel else { return }
        saving = true; defer { saving = false }
        var next = featureSettings; next.items[id] = policy
        do {
            let config = try JSONSerialization.jsonObject(with: JSONEncoder().encode(next))
            let body = try JSONSerialization.data(withJSONObject: ["action": "features", "config": config])
            _ = try await channel.send("POST", "/local/workflows", body: body).checked()
            error = nil
            await refresh()
        } catch { self.error = error.localizedDescription }
    }

    func sourceName(_ job: Job) -> String {
        if job.input_ref.hasPrefix("screen:") { return "屏幕片段" }
        if job.input_ref.hasPrefix("workflow-day:") { return "录音日回顾" }
        guard let item = media.first(where: { $0.id == job.input_ref || $0.recording_parts?.contains(where: { $0.id == job.input_ref }) == true }) else { return "录音资料" }
        return item.title?.isEmpty == false ? item.title! : item.filename
    }
    func authorize(_ provider: AIProvider, source: String, allowed: Bool) async {
        guard let channel = TimelineService.shared.channel else { return }
        do {
            let body = try JSONSerialization.data(withJSONObject: ["action": "authorize", "source": source, "provider_id": provider.providerID, "allowed": allowed])
            _ = try await channel.send("POST", "/local/workflows", body: body).checked(); await refresh()
        } catch { self.error = error.localizedDescription }
    }
    func enqueue(_ step: String) async {
        guard let channel = TimelineService.shared.channel else { return }
        do {
            try await AIRuntime.shared.syncCore()
            let body: [String: String]
            if step == "daily" {
                let formatter = DateFormatter(); formatter.dateFormat = "yyyy-MM-dd"
                let day = formatter.string(from: Date()); selectedID = "workflow-day:" + day
                body = ["action": "daily", "day": day, "start_ms": String(Int64(Calendar.current.startOfDay(for: Date()).timeIntervalSince1970 * 1000)), "end_ms": String(Int64((Calendar.current.date(byAdding: .day, value: 1, to: Calendar.current.startOfDay(for: Date())) ?? Date()).timeIntervalSince1970 * 1000))]
            } else { body = ["action": "enqueue", "id": selectedID, "step": step] }
            _ = try await channel.send("POST", "/local/workflows", body: JSONSerialization.data(withJSONObject: body)).checked()
            await refresh()
        } catch { self.error = error.localizedDescription }
    }
    func save() async {
        guard let channel = TimelineService.shared.channel else { return }
        do {
            try await AIRuntime.shared.syncCore()
            let body = try JSONSerialization.data(withJSONObject: ["action": "settings", "settings": JSONSerialization.jsonObject(with: JSONEncoder().encode(settings))])
            _ = try await channel.send("POST", "/local/workflows", body: body).checked()
            await refresh()
        } catch { self.error = error.localizedDescription }
    }
    func command(_ action: String, job: Job) async {
        guard let channel = TimelineService.shared.channel else { return }
        do {
            try await AIRuntime.shared.syncCore()
            _ = try await channel.send("POST", "/local/jobs", body: JSONSerialization.data(withJSONObject: ["action": action, "id": job.id])).checked()
            await refresh()
        } catch { self.error = error.localizedDescription }
    }
}
