import Foundation
import CandidLocalization
import AIProviders
import CandidClient
import Observation

@MainActor @Observable final class WorkflowControlModel {
    struct Catalog: Decodable { var features: [Feature]? }
    struct Feature: Decodable, Identifiable {
        var id: String; var title: String; var mode: String; var implemented: Bool
        var trigger: String; var role: String; var job_kind: String?
    }
    struct FeaturePolicy: Codable, Equatable {
        var enabled: Bool; var sources: [String]; var allow_remote: Bool; var daily_calls: Int
    }
    struct FeatureSettings: Codable { var version = 1; var items: [String: FeaturePolicy] = [:] }
    struct Job: Decodable, Identifiable {
        var id: String; var kind: String; var input_ref: String; var state: String; var error_code: String?
        var title: String { kind == "workflow.speakers" ? L("说话人整理") : kind == "screen.summarize" ? L("屏幕片段摘要") : kind == "agent.skill" ? L("技能运行") : kind == "speech.recognize" ? L("语音转写") : ["workflow.title": L("标题"), "workflow.summary": L("摘要"), "workflow.tags": L("标签"), "workflow.decisions": L("决定与待办")][kind] ?? L("整理") }
        var status: String { if let error_code { return ["feature_disabled": L("功能已关闭"), "waiting_feature_source": L("来源不在功能范围内"), "waiting_feature_remote": L("此功能只用本机模型"), "waiting_feature_budget": L("已到每日调用上限"), "waiting_model_request_rejected": L("摘要请求被模型拒绝，请检查模型设置后重试"), "waiting_model": L("等待选择模型"), "waiting_authorization": L("等待授权"), "waiting_budget_cost_unknown": L("费用未知，等待预算确认"), "offline": L("模型离线"), "timeout": L("请求超时"), "http_429": L("模型繁忙"), "bad_json": L("模型返回格式有误"), "configuration_changed": L("模型配置已改变"), "source_or_configuration_changed": L("来源或配置已改变")][error_code] ?? L("处理未完成，请重试") }; return ["queued": L("排队中"), "running": L("处理中"), "failed": L("失败"), "done": L("完成"), "cancelled": L("已取消")][state] ?? state }
    }
    struct Settings: Codable {
        var version = 1; var automatic = true; var summary = false; var mode = "conversation"
        /// Titles for transcribed recordings; on unless turned off, also for settings saved before it existed.
        var titles = true
        static func automaticKey(for id: String) -> WritableKeyPath<Self, Bool>? {
            switch id {
            case "transcribe": \.automatic
            case "title": \.titles
            case "summary": \.summary
            default: nil
            }
        }
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
    enum ProcessingMode: String, CaseIterable {
        case off, manual, automatic

        var title: String {
            switch self {
            case .off: L("关闭")
            case .manual: L("仅手动")
            case .automatic: L("自动")
            }
        }
    }
    struct Snapshot: Decodable { var catalog: Catalog; var settings: Settings?; var jobs: [Job]?; var approvals: [String: Bool]?; var features: FeatureSettings? }
    struct Media: Decodable, Identifiable { var id: String; var title: String?; var filename: String; var kind: String; var recording_parts: [Part]?
        struct Part: Decodable { var id: String }
    }
    struct Draft: Decodable { var text: String }
    var remoteProviders: [AIProvider] = []
    var approvals: [String: Bool] = [:]
    var media: [Media] = []
    var selectedID = ""
    var drafts: [String: Draft] = [:]
    var jobs: [Job] = []
    var features: [Feature] = []
    var sourceCatalog: [AutomationSource] = []
    var featureSettings = FeatureSettings()
    var saving = false
    var settings = Settings()
    var error: String?
    var loading = false
    private let channel: () -> LocalChannel?

    init(channel: @escaping () -> LocalChannel? = { TimelineService.shared.channel }) { self.channel = channel }

    func refresh(includeActivity: Bool = true) async {
        guard !loading, !saving else { return }
        guard let channel = channel() else { error = L("时间线库尚未打开"); return }
        loading = true; defer { loading = false }
        do {
            let request: [String: Any] = ["action": "status", "include_activity": includeActivity]
            let data = try await channel.send("POST", "/local/workflows", body: JSONSerialization.data(withJSONObject: request)).checked()
            let snapshot = try JSONDecoder().decode(Snapshot.self, from: data)
            approvals = snapshot.approvals ?? [:]
            features = snapshot.catalog.features ?? []
            featureSettings = snapshot.features ?? FeatureSettings()
            settings = snapshot.settings ?? Settings()
            if includeActivity {
                jobs = snapshot.jobs ?? []
                struct MediaList: Decodable { var items: [Media] }
                let list: MediaList = try await channel.post("/local/media", body: ["action": "list"])
                media = list.items
                await loadDrafts()
            } else {
                let config: ProviderConfig = try await channel.post("/local/models", body: ["action": "get"])
                remoteProviders = config.providers.filter { $0.destination != .onDevice }
                let automation: AutomationSnapshot = try await automationPost(channel, ["action": "status", "include_runs": false])
                sourceCatalog = automation.sources
            }
            error = nil
        } catch { self.error = error.localizedDescription }
    }

    func loadDrafts() async {
        let id = selectedID
        guard !id.isEmpty, let channel = channel() else { drafts = [:]; return }
        let result: [String: Draft]? = try? await channel.post("/local/workflows", body: ["action": "drafts", "id": id])
        if selectedID == id { drafts = result ?? [:] }
    }

    func refreshJobs() async {
        guard let channel = channel() else { return }
        struct JobList: Decodable { var jobs: [Job] }
        if let list: JobList = try? await channel.post("/local/jobs", body: ["action": "list"]), !Task.isCancelled {
            jobs = list.jobs
        }
    }

    func processingMode(_ id: String) -> ProcessingMode {
        guard featureSettings.items[id]?.enabled == true else { return .off }
        guard let key = Settings.automaticKey(for: id) else { return .automatic }
        return settings[keyPath: key] ? .automatic : .manual
    }

    func setProcessingMode(_ id: String, _ mode: ProcessingMode) async {
        guard !saving, !loading, let channel = channel(),
              var policy = featureSettings.items[id] else { return }
        saving = true; defer { saving = false }
        do {
            // Set the trigger before enabling a disabled feature. If enabling fails, it stays off.
            if mode != .off, let key = Settings.automaticKey(for: id), settings[keyPath: key] != (mode == .automatic) {
                var next = settings
                next[keyPath: key] = mode == .automatic
                try await saveSettings(next, channel: channel)
            }
            if policy.enabled != (mode != .off) {
                policy.enabled = mode != .off
                try await savePolicy(id, policy, channel: channel)
            }
            error = nil
        } catch { self.error = error.localizedDescription }
    }

    func setTranscriptionMode(_ mode: String) async {
        guard !saving, !loading, let channel = channel() else { return }
        saving = true; defer { saving = false }
        do {
            var next = settings; next.mode = mode
            try await saveSettings(next, channel: channel)
            error = nil
        } catch { self.error = error.localizedDescription }
    }

    func setPolicy(_ id: String, _ policy: FeaturePolicy) async {
        guard !saving, !loading, let channel = channel() else { return }
        saving = true; defer { saving = false }
        do {
            try await savePolicy(id, policy, channel: channel)
            error = nil
        } catch { self.error = error.localizedDescription }
    }

    private func savePolicy(_ id: String, _ policy: FeaturePolicy, channel: LocalChannel) async throws {
        var next = featureSettings; next.items[id] = policy
        let config = try JSONSerialization.jsonObject(with: JSONEncoder().encode(next))
        let body = try JSONSerialization.data(withJSONObject: ["action": "features", "config": config])
        featureSettings = try JSONDecoder().decode(FeatureSettings.self, from: try await channel.send("POST", "/local/workflows", body: body).checked())
    }

    private func saveSettings(_ next: Settings, channel: LocalChannel) async throws {
        let body = try JSONSerialization.data(withJSONObject: ["action": "settings", "settings": JSONSerialization.jsonObject(with: JSONEncoder().encode(next))])
        settings = try JSONDecoder().decode(Settings.self, from: try await channel.send("POST", "/local/workflows", body: body).checked())
    }

    func sourceName(_ job: Job) -> String {
        if job.input_ref.hasPrefix("screen:") { return L("屏幕片段") }
        if job.input_ref.hasPrefix("workflow-day:") { return L("录音日回顾") }
        guard let item = media.first(where: { $0.id == job.input_ref || $0.recording_parts?.contains(where: { $0.id == job.input_ref }) == true }) else { return L("录音资料") }
        return item.title?.isEmpty == false ? item.title! : item.filename
    }
    func authorize(_ provider: AIProvider, source: String, allowed: Bool) async {
        guard !saving, !loading, let channel = channel() else { return }
        saving = true; defer { saving = false }
        do {
            let body = try JSONSerialization.data(withJSONObject: ["action": "authorize", "source": source, "provider_id": provider.providerID, "allowed": allowed])
            _ = try await channel.send("POST", "/local/workflows", body: body).checked()
            approvals[source + ":" + provider.providerID] = allowed
            error = nil
        } catch { self.error = error.localizedDescription }
    }
    func enqueue(_ step: String) async {
        guard let channel = channel() else { error = L("时间线库尚未打开"); return }
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
    func command(_ action: String, job: Job) async {
        guard let channel = channel() else { return }
        do {
            try await AIRuntime.shared.syncCore()
            _ = try await channel.send("POST", "/local/jobs", body: JSONSerialization.data(withJSONObject: ["action": action, "id": job.id])).checked()
            await refresh()
        } catch { self.error = error.localizedDescription }
    }
}
