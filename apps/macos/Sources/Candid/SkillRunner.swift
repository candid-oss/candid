import Foundation
import CandidLocalization
import CandidClient

/// App lifetime worker; skill jobs survive page changes. The core owns leases, permission checks and deduplication.
@MainActor final class SkillRunner {
    static let shared = SkillRunner()
    private let channel: () -> LocalChannel?
    private let agent: AIChatAgent
    private let notifier: AutomationNotifying
    private var active: AutomationRun?
    private var result = ""
    private var failure: String?
    private var stopping = false
    private var finishing = false
    private let skills: () -> [AISkill]
    init(channel: @escaping () -> LocalChannel? = { TimelineService.shared.channel }, agent: AIChatAgent? = nil,
         notifier: AutomationNotifying = AutomationNotifier.shared,
         skills: @escaping () -> [AISkill] = { AISkill.load(from: AISkill.folders(for: .resolved())) }) {
        self.channel = channel; self.agent = agent ?? PiAgent(channel: channel)
        self.notifier = notifier; self.skills = skills
        self.agent.onEvent = { [weak self] in self?.receive($0) }
    }
    func runLoop() async {
        while !Task.isCancelled {
            await tick()
            try? await Task.sleep(for: .seconds(10))
        }
        await agent.stop()
    }
    func tick() async {
        guard !finishing, let channel = channel() else { return }
        do {
            if let active {
                do { let _: AutomationAck = try await automationPost(channel, ["action": "heartbeat", "id": active.id, "attempt": active.attempt]) }
                catch { stopping = true; agent.cancel(); await agent.stop(); self.active = nil; stopping = false }
                return
            }
            // A foreground conversation has priority. Its idle process can be replaced safely after settling.
            struct PiStatus: Decodable { struct Run: Decodable { var active: Bool?; var stopped: String? }; var run: Run? }
            if let state: PiStatus = try? await channel.get("/local/pi/run"), state.run?.active == true && state.run?.stopped == nil { return }
            let next: AutomationRun? = try await automationPost(channel, ["action": "claim"])
            if let next { await execute(next) }
            let snapshot: AutomationSnapshot = try await automationPost(channel, ["action": "status", "notifications_only": true])
            for run in snapshot.runs where run.state == "done" && !run.notified && run.unavailable != true {
                if await notifier.deliver(run, title: snapshot.catalog.first { $0.id == run.skill }?.title ?? L("AI 处理")) {
                    let _: AutomationAck = try await automationPost(channel, ["action": "notified", "id": run.id])
                }
            }
        } catch { /* Persisted queued/failed state is shown in the settings; the worker retries connection, not model calls. */ }
    }
    private func execute(_ run: AutomationRun) async {
        guard let skill = skills().first(where: { $0.id == run.skill }) else {
            await finish(run, state: "failed", error: L("找不到技能正文")); return
        }
        active = run; result = ""; failure = nil
        do {
            var config = PiRunConfig()
            config.runID = run.id; config.automationID = run.id; config.attempt = run.attempt
            try await agent.start(config)
            let context = run.references.map { "[[\($0.event_id)]]" }.joined(separator: L("、"))
            let input = run.context + (context.isEmpty ? "" : L("\n本次来源：") + context)
            try agent.prompt(skill.prompt(today: Date(), context: [], extra: input + L("\n最后只输出 JSON：{\"has_updates\":true或false,\"summary\":\"简短结果\"}；没有新内容时 has_updates=false，不编造。")))
        } catch { active = nil; await agent.stop(); await finish(run, state: "failed", error: AIPageModel.explain(error)) }
    }
    private func receive(_ event: PiEvent) {
        guard !stopping, let run = active else { return }
        switch event {
        case .assistantMessage(let text): if !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { result = text }
        case .error(let text): failure = text
        case .settled(let aborted):
            active = nil; finishing = true
            let state = failure != nil ? "failed" : aborted ? "cancelled" : "done"
            let error = failure
            Task { await agent.stop(); await finish(run, state: state, error: error); finishing = false }
        default: break
        }
    }
    private func finish(_ run: AutomationRun, state: String, error: String?) async {
        guard let channel = channel() else { return }
        var body: [String: Any] = ["action": "finish", "id": run.id, "attempt": run.attempt, "state": state, "result": result]
        if let error { body["error"] = error }
        let _: AutomationRun? = try? await automationPost(channel, body)
    }
}
