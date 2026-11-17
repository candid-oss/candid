import Foundation
import CandidLocalization
import Observation
import CandidClient

struct SkillPolicy: Codable, Equatable, Sendable {
    var enabled = true
    var automatic = false
    var trigger = "manual"
    var time = "21:00"
    var timezone = "Asia/Singapore"
    var notify = false
    var sources = ["*"]
    var role = "agent_chat"
    var allow_remote = false
    var daily_runs = 3
    var max_model_calls = 12
    var max_tool_calls = 40
    var max_output_tokens = 16_000
}
struct AutomationConfig: Codable, Sendable {
    var version = 1
    var skills: [String: SkillPolicy] = [:]
    var remote_sources: [String] = []
}
struct AutomationRun: Decodable, Identifiable, Sendable {
    struct Reference: Decodable, Identifiable, Sendable {
        var event_id: String; var source_id: String; var revision: String
        var id: String { event_id }
    }
    struct Step: Decodable, Sendable { var tool: String; var count: Int; var at_ms: Int64 }
    struct Model: Decodable, Sendable { var provider_name: String?; var model_id: String }
    var model: Model?
    var id: String; var skill: String; var trigger: String; var state: String; var created_ms: Int64
    var attempt: Int; var sources: [String]; var references: [Reference]; var steps: [Step]
    var context: String; var result: String?; var error: String?; var policy: SkillPolicy
    var notified: Bool; var unavailable: Bool?; var notify_ready: Bool?
    var displayResult: String? {
        guard let result else { return nil }
        if let data = result.data(using: .utf8), let value = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
           let summary = value["summary"] as? String { return summary }
        return result
    }
    var status: String {
        if unavailable == true { return L("来源已不可用") }
        return ["queued": L("排队中"), "running": L("运行中"), "done": L("完成"), "failed": L("失败"), "cancelled": L("已取消")][state] ?? state
    }
    var problem: String? {
        error.map { ["offline": L("模型离线"), "interrupted": L("上次运行中断，可重试"), "configuration_changed": L("设置已改变，请重新运行"), "source_or_configuration_changed": L("来源或设置已改变"), "user_cancelled": L("已取消"), "model_budget": L("本次模型额度已用完")][ $0 ] ?? $0 }
    }
}
struct AutomationSource: Decodable, Identifiable, Sendable {
    struct Policy: Decodable, Sendable { var store_text: Bool; var agent_visible: Bool }
    var source_id: String; var display_name: String; var status: String; var policy: Policy
    var id: String { source_id }
    var readable: Bool { status != "revoked" && policy.store_text && policy.agent_visible }
}
struct AutomationSnapshot: Decodable {
    struct Quality: Decodable { var passed: Bool }
    var config: AutomationConfig; var quality: Quality; var runs: [AutomationRun]
    var sources: [AutomationSource]; var catalog: [WorkflowControlModel.Feature]
}
struct AutomationAck: Decodable { }

func automationPost<T: Decodable>(_ channel: LocalChannel, _ body: [String: Any]) async throws -> T {
    let data = try JSONSerialization.data(withJSONObject: body)
    return try JSONDecoder().decode(T.self, from: try await channel.send("POST", "/local/automation", body: data).checked())
}

@MainActor @Observable final class AutomationModel {
    var config = AutomationConfig()
    var runs: [AutomationRun] = []
    var details: [String: AutomationRun] = [:]
    var sources: [AutomationSource] = []
    var catalog: [WorkflowControlModel.Feature] = []
    var passed = false
    var busy = false
    var error: String?
    private let channel: () -> LocalChannel?
    init(channel: @escaping () -> LocalChannel? = { TimelineService.shared.channel }) { self.channel = channel }

    func refresh(includeRuns: Bool = true) async {
        guard let channel = channel() else { return }
        do {
            let snapshot: AutomationSnapshot = try await automationPost(channel, ["action": "status", "include_runs": includeRuns])
            config = snapshot.config; runs = snapshot.runs; sources = snapshot.sources; catalog = snapshot.catalog
            passed = snapshot.quality.passed; error = nil
        } catch { self.error = AIPageModel.explain(error) }
    }
    func detail(_ id: String) async {
        guard let channel = channel() else { return }
        do { let run: AutomationRun = try await automationPost(channel, ["action": "get", "id": id]); details[id] = run }
        catch { self.error = AIPageModel.explain(error) }
    }
    func save(_ next: AutomationConfig) async {
        guard !busy, let channel = channel() else { return }
        busy = true; defer { busy = false }
        do {
            let object = try JSONSerialization.jsonObject(with: JSONEncoder().encode(next))
            let _: AutomationConfig = try await automationPost(channel, ["action": "config", "config": object])
            await refresh()
        } catch { self.error = AIPageModel.explain(error) }
    }
    func update(_ id: String, _ change: (inout SkillPolicy) -> Void) async {
        var next = config; guard var p = next.skills[id] else { return }
        change(&p); next.skills[id] = p; await save(next)
    }
    func command(_ action: String, id: String) async {
        guard let channel = channel() else { return }
        do { let _: AutomationRun = try await automationPost(channel, ["action": action, "id": id]); details[id] = nil; await refresh() }
        catch { self.error = AIPageModel.explain(error) }
    }
    func loadEvaluation(_ data: Data) async {
        guard let channel = channel() else { return }
        do {
            let report = try JSONSerialization.jsonObject(with: data)
            let _: AutomationSnapshot.Quality = try await automationPost(channel, ["action": "quality", "report": report])
            await refresh()
        } catch { self.error = AIPageModel.explain(error) }
    }
    func title(_ id: String) -> String { catalog.first { $0.id == id }?.title ?? L("技能") }
}

extension Notification.Name {
    static let candidRunSkill = Notification.Name("Candid.runSkill")
    static let candidOpenRecord = Notification.Name("Candid.openRecord")
    static let candidAutomationRun = Notification.Name("Candid.automationRun")
}
