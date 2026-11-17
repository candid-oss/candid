import AIProviders
import CandidLocalization
import Foundation
import Observation

/// State behind the 「模型」 settings section. Persistence goes through the AIProviders protocols, so swapping the
/// temporary JSON/JSONL implementations for the SQLite-backed ones is a change to `init` only.
@MainActor @Observable final class ModelSettingsModel {
    enum ProbeState: Equatable { case idle, testing, ok(latencyMs: Int, models: Int), failed(String) }

    private(set) var config = ProviderConfig()
    private(set) var probes: [String: ProbeState] = [:]
    private(set) var calls: [AICallRecord] = []
    private(set) var keyPresent: Set<String> = []
    var error: String?

    let client: AIClient
    private let store: ProviderConfigStore
    private let secrets: SecretStore
    private let sink: AIUsageSink & AIUsageSource

    static let configChanged = Notification.Name("Candid.aiConfigChanged")

    /// Shares the app-wide runtime, so what is saved here is what recognition and the agent use.
    init(runtime: AIRuntime = .shared) {
        store = runtime.store; secrets = runtime.secrets; sink = runtime.sink; client = runtime.client
        reload()
    }

    func reload() {
        do { config = try store.load() } catch { self.error = L("读取模型配置失败：%@", error.localizedDescription) }
        keyPresent = Set(config.providers.compactMap { p in
            guard let ref = p.secretRef, (try? secrets.hasSecret(for: ref)) == true else { return nil }
            return p.providerID
        })
    }

    private func commit(_ change: (inout ProviderConfig) throws -> Void) {
        var next = config
        do {
            try change(&next); try store.save(next); config = next; error = nil
            NotificationCenter.default.post(name: Self.configChanged, object: nil)
            if !AIRuntime.isTesting {
                UserDefaults.standard.set(true, forKey: AIRuntime.pendingCoreConfig)
                Task { do { try await AIRuntime.shared.syncCore() } catch { self.error = L("模型设置等待核心恢复后同步") } }
            }
        }
        catch { self.error = L("保存失败：%@", error) }
    }

    // MARK: Providers

    func save(_ provider: AIProvider, key: String?) {
        var p = provider
        let trimmed = key?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if !trimmed.isEmpty {
            let ref = p.secretRef ?? "provider.\(p.providerID)"
            do { try secrets.setSecret(trimmed, for: ref); p.secretRef = ref; keyPresent.insert(p.providerID) }
            catch { self.error = L("钥匙串写入失败：%@", error); return }
        }
        commit { try $0.upsert(p) }
    }

    func delete(_ provider: AIProvider) {
        if let ref = provider.secretRef { try? secrets.deleteSecret(for: ref) }
        keyPresent.remove(provider.providerID); probes[provider.providerID] = nil
        commit { $0.remove(providerID: provider.providerID) }
    }

    func setEnabled(_ provider: AIProvider, _ on: Bool) {
        var p = provider; p.enabled = on
        commit { try $0.upsert(p) }
    }

    /// Probes a saved provider and stores the discovered models.
    func test(_ provider: AIProvider) {
        probes[provider.providerID] = .testing
        let probe = CapabilityProbe(secrets: secrets)
        Task {
            let result = await probe.probe(provider)
            if result.reachable {
                probes[provider.providerID] = .ok(latencyMs: result.latencyMs ?? 0, models: result.models.count)
                commit { $0.merge(probed: result.models, for: provider.providerID) }
            } else { probes[provider.providerID] = .failed(result.error ?? L("失败")) }
        }
    }

    func acceptProbe(_ result: ProbeResult, for provider: AIProvider) {
        guard result.reachable else { return }
        probes[provider.providerID] = .ok(latencyMs: result.latencyMs ?? 0, models: result.models.count)
        commit { $0.merge(probed: result.models, for: provider.providerID) }
    }

    /// Only fills empty text/image roles after the user's explicit choice; existing assignments stay intact.
    func assignSuggestions(from provider: AIProvider) {
        commit { config in
            let available = config.models(of: provider.providerID)
            for id in ["agent_chat", "vision", "title", "summary"] {
                guard let role = ModelRoleInfo.info(id), config.assignment(id)?.model == nil,
                      config.assignment(id)?.engine == nil else { continue }
                guard let choice = available.first(where: { m in
                    (!role.needsImages || m.supportsImages == true) && (!role.needsTools || m.supportsTools == true)
                        && m.modalities.contains("text")
                }) else { continue }
                var assignment = config.assignment(id) ?? RoleAssignment(role: id)
                assignment.model = choice.ref
                try config.setAssignment(assignment)
            }
        }
    }

    /// Probes an unsaved draft with the key typed in the sheet; nothing is stored.
    func trial(_ provider: AIProvider, key: String) async -> ProbeResult {
        var secretStore: SecretStore = secrets
        if !key.isEmpty {
            let temp = InMemorySecretStore([provider.secretRef ?? "draft": key])
            secretStore = temp
        }
        var p = provider
        if !key.isEmpty, p.secretRef == nil { p.secretRef = "draft" }
        return await CapabilityProbe(secrets: secretStore).probe(p)
    }

    // MARK: Roles

    func assign(role: String, to ref: ModelRef?) {
        var a = config.assignment(role) ?? RoleAssignment(role: role)
        a.model = ref
        if ref != nil { a.engine = nil }
        commit { c in
            a.fallback = c.allowedFallbacks(for: a)
            try c.setAssignment(a)
        }
    }

    func addFallback(role: String, _ ref: ModelRef) {
        guard var a = config.assignment(role) else { return }
        if !a.fallback.contains(ref) { a.fallback.append(ref) }
        commit { try $0.setAssignment(a) }
    }

    func removeFallback(role: String, _ ref: ModelRef) {
        guard var a = config.assignment(role) else { return }
        a.fallback.removeAll { $0 == ref }
        commit { try $0.setAssignment(a) }
    }

    /// Picks a built-in engine for the role (e.g. system text recognition) instead of a model.
    func useEngine(role: String, _ engine: BuiltinEngine) {
        var a = config.assignment(role) ?? RoleAssignment(role: role)
        a.engine = engine.id; a.model = nil; a.fallback = []
        commit { try $0.setAssignment(a) }
    }

    func prompt(for role: String) -> String {
        let custom = config.assignment(role)?.prompt?.trimmingCharacters(in: .whitespacesAndNewlines)
        return (custom?.isEmpty == false ? custom : nil) ?? PromptCatalog.defaultPrompt(role)
    }

    func isCustomPrompt(_ role: String) -> Bool { config.assignment(role)?.prompt != nil }

    /// Saving the default text (or nothing) clears the override.
    func setPrompt(_ text: String, for role: String) {
        var a = config.assignment(role) ?? RoleAssignment(role: role)
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        a.prompt = trimmed.isEmpty || trimmed == PromptCatalog.defaultPrompt(role).trimmingCharacters(in: .whitespacesAndNewlines) ? nil : trimmed
        commit { try $0.setAssignment(a) }
    }

    func setLimits(role: String, _ limits: RoleLimits) {
        guard var a = config.assignment(role) else { return }
        a.limits = limits
        commit { try $0.setAssignment(a) }
    }

    func dailyCalls(_ provider: String, now: Date = Date()) -> [Int] {
        let calendar = Calendar.current
        let today = calendar.startOfDay(for: now)
        return (0..<7).map { index in
            let start = calendar.date(byAdding: .day, value: index - 6, to: today)!
            let end = calendar.date(byAdding: .day, value: 1, to: start)!
            return calls.filter { $0.providerID == provider && Double($0.startedMs) >= start.timeIntervalSince1970 * 1000 && Double($0.startedMs) < end.timeIntervalSince1970 * 1000 }.count
        }
    }

    // MARK: Usage

    func loadUsage(days: Int) async {
        let cal = Calendar.current
        let start = cal.date(byAdding: .day, value: -(days - 1), to: cal.startOfDay(for: Date())) ?? Date()
        if !AIRuntime.isTesting, let channel = TimelineService.shared.channel {
            struct Usage: Decodable { var calls: [AICallRecord] }
            if let result: Usage = try? await channel.post("/local/models", body: ["action": "usage"]) {
                calls = result.calls.filter { $0.startedMs >= Int64(start.timeIntervalSince1970 * 1000) }; return
            }
        }
        calls = await sink.calls(from: Int64(start.timeIntervalSince1970 * 1000), to: nowMs() + 1)
    }
}
