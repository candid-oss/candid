import AIProviders
import CandidLocalization
import DesktopPlatform
import Foundation

/// The one model client, configuration and usage log the whole app shares: settings edit it, recognition and
/// later the agent call through it, so every call is recorded in the same place. Created on first use only.
@MainActor final class AIRuntime {
    /// Tests get in-memory stores so they never read the user's model settings or keychain.
    /// The self-test copy (`--qa-snapshot-dir`) gets a synthetic sample instead, so it never touches the keychain
    /// (a differently signed copy would make macOS ask the user) or the real settings file.
    static let shared: AIRuntime = isTesting
        ? AIRuntime(store: InMemoryProviderConfigStore(ProviderConfig()), secrets: InMemorySecretStore(), sink: InMemoryUsageSink())
        : ProcessInfo.processInfo.arguments.contains("--qa-snapshot-dir")
            ? AIRuntime(store: InMemoryProviderConfigStore(syntheticSample), secrets: InMemorySecretStore(), sink: InMemoryUsageSink())
            : AIRuntime()

    /// Clearly synthetic: one local source with one vision model, used only by the self-test copy.
    static var syntheticSample: ProviderConfig {
        let p = AIProvider(providerID: "sample-lm", name: L("LM Studio（合成样例）"), api: .openaiChat, baseURL: "http://localhost:1234/v1", destination: .onDevice)
        var vlm = AIModel(providerID: p.providerID, modelID: "sample/vision-4b", modalities: ["text", "image"], supportsTools: true, probedMs: 1)
        vlm.modalitiesDeclared = true
        let text = AIModel(providerID: p.providerID, modelID: "sample/text-8b", supportsTools: true, probedMs: 1)
        var config = ProviderConfig(providers: [p], models: [vlm, text])
        try? config.setAssignment(RoleAssignment(role: "vision", model: vlm.ref))
        return config
    }

    static var isTesting: Bool { NSClassFromString("XCTestCase") != nil || ProcessInfo.processInfo.environment["XCTestSessionIdentifier"] != nil }

    static let pendingCoreConfig = "ai.models.pendingCoreConfig.v1"
    private var coreInitialized: String?
    let store: ProviderConfigStore
    let secrets: SecretStore
    let sink: AIUsageSink & AIUsageSource
    let client: AIClient

    init(store: ProviderConfigStore = JSONFileProviderConfigStore(), secrets: SecretStore = KeychainSecretStore(),
         sink: AIUsageSink & AIUsageSource = JSONLUsageSink()) {
        self.store = store; self.secrets = secrets; self.sink = sink
        let bridge: AIClient.CoreComplete?
        if Self.isTesting { bridge = nil }
        else { bridge = { request, model, context in
            try await AIRuntime.shared.coreComplete(request, model: model, context: context)
        } }
        client = AIClient(configStore: store, secrets: secrets, usage: sink, usageSource: sink, coreComplete: bridge)
    }

    func config() -> ProviderConfig { (try? store.load()) ?? ProviderConfig() }

    func assignment(_ role: String) -> RoleAssignment? { config().assignment(role) }

    /// The model chosen for a role, if any (a built-in engine is not a model).
    func model(for role: String) -> ModelRef? { assignment(role)?.model }

    func prompt(for role: String) -> String {
        let custom = assignment(role)?.prompt?.trimmingCharacters(in: .whitespacesAndNewlines)
        return (custom?.isEmpty == false ? custom : nil) ?? PromptCatalog.defaultPrompt(role)
    }

    /// Saves the user's prompt for a role; the default text (or empty) clears the override.
    func setPrompt(_ text: String?, for role: String) throws {
        var config = config()
        var a = config.assignment(role) ?? RoleAssignment(role: role)
        let trimmed = text?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        a.prompt = trimmed.isEmpty || trimmed == PromptCatalog.defaultPrompt(role).trimmingCharacters(in: .whitespacesAndNewlines) ? nil : trimmed
        try config.setAssignment(a)
        try store.save(config)
        if !Self.isTesting { UserDefaults.standard.set(true, forKey: Self.pendingCoreConfig) }
    }

    // MARK: One-time move of the old recognition settings

    static let legacyMigrationKey = "ai.migrated.desktopVision.v1"

    /// Earlier builds kept their own vision endpoint, model and prompt in user defaults. Move them onto the
    /// 「图像理解」 role once: reuse a source with the same address (localhost and 127.0.0.1 count as one),
    /// otherwise add one. The old token was never stored, so a source that needs a key asks for it in settings.
    func migrateLegacyVision(defaults: UserDefaults = .standard) {
        guard !defaults.bool(forKey: Self.legacyMigrationKey) else { return }
        defer { defaults.set(true, forKey: Self.legacyMigrationKey) }
        var config = config()
        let model = (defaults.string(forKey: "desktop.vision.model") ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        var vision = config.assignment("vision") ?? RoleAssignment(role: "vision")
        if let legacyPrompt = defaults.string(forKey: "desktop.vision.prompt")?.trimmingCharacters(in: .whitespacesAndNewlines),
           !legacyPrompt.isEmpty, vision.prompt == nil,
           legacyPrompt != DesktopVisionClient.defaultPrompt.trimmingCharacters(in: .whitespacesAndNewlines) {
            vision.prompt = legacyPrompt
        }
        if vision.model == nil, !model.isEmpty {
            let endpoint = defaults.string(forKey: "desktop.vision.endpoint") ?? DesktopVisionClient.defaultEndpoint
            let provider: AIProvider
            if let existing = config.providers.first(where: { Self.sameEndpoint($0.baseURL, endpoint) }) {
                provider = existing
            } else {
                let host = URL(string: endpoint)?.host(percentEncoded: false) ?? ""
                let local = ["localhost", "127.0.0.1", "::1"].contains(host)
                provider = AIProvider(name: L("视觉服务（来自识别设置）"), api: .openaiChat, baseURL: endpoint,
                                      destination: local ? .onDevice : .selfHosted)
                guard (try? config.upsert(provider)) != nil else { return }
            }
            config.ensureModel(AIModel(providerID: provider.providerID, modelID: model))
            vision.model = ModelRef(providerID: provider.providerID, modelID: model)
        }
        guard vision != (config.assignment("vision") ?? RoleAssignment(role: "vision")) else { return }
        guard (try? config.setAssignment(vision)) != nil else { return }
        try? store.save(config)
    }

    static func sameEndpoint(_ a: String, _ b: String) -> Bool {
        func norm(_ s: String) -> String {
            var v = s.trimmingCharacters(in: .whitespaces).lowercased()
            while v.hasSuffix("/") { v.removeLast() }
            return v.replacingOccurrences(of: "://127.0.0.1", with: "://localhost").replacingOccurrences(of: "://[::1]", with: "://localhost")
        }
        return norm(a) == norm(b)
    }
}


extension AIRuntime {
    /// Reconcile once when a core starts, then on each model action. Keys only cross the private socket in memory.
    func syncCore() async throws {
        guard let channel = TimelineService.shared.channel else { return }
        let first = coreInitialized != TimelineService.shared.rootPath
        if first && !UserDefaults.standard.bool(forKey: Self.pendingCoreConfig) {
            if let remote: ProviderConfig = try? await channel.post("/local/models", body: ["action": "get"]), !remote.providers.isEmpty, !UserDefaults.standard.bool(forKey: Self.pendingCoreConfig) { try store.save(remote) }
        }
        var config = config()
        if first && config.assignment("asr")?.model == nil {
            let provider = AIProvider(providerID: "candid-local-voice", name: L("本机语音服务"), api: .other, baseURL: "http://127.0.0.1:8767/v1", destination: .onDevice)
            try config.upsert(provider)
            for (role, modelID) in [("asr", "qwen3-asr"), ("diarization", "moss-campp")] {
                let model = AIModel(providerID: provider.providerID, modelID: modelID, modalities: ["audio"])
                config.ensureModel(model)
                if config.assignment(role)?.model == nil { try config.setAssignment(RoleAssignment(role: role, model: model.ref)) }
            }
            try store.save(config)
        }
        let body = try JSONSerialization.data(withJSONObject: ["action": "save", "config": JSONSerialization.jsonObject(with: JSONEncoder().encode(config))])
        _ = try await channel.send("POST", "/local/models", body: body).checked()
        // Keychain reads run off the main thread and never prompt: a pending system dialog must not freeze the App.
        let refs = config.providers.compactMap(\.secretRef), secrets = self.secrets
        let (keys, blocked) = await Task.detached(priority: .utility) { () -> ([String: String], Bool) in
            var keys: [String: String] = [:], blocked = false
            for ref in refs {
                do { if let key = try secrets.secretWithoutPrompt(for: ref) { keys[ref] = key } } catch { blocked = true }
            }
            return (keys, blocked)
        }.value
        if blocked {
            NotificationCenter.default.post(name: .candidActionError, object: L("模型密钥需要钥匙串授权：在 AI 页用一次模型，弹出提示时选「始终允许」"))
        }
        _ = try await channel.send("POST", "/local/models/authorize", body: JSONSerialization.data(withJSONObject: ["secrets": keys])).checked()
        let calls = first ? await sink.calls(from: 0, to: nowMs() + 1) : []
        if !calls.isEmpty {
            let body = try JSONSerialization.data(withJSONObject: ["action": "import_usage", "calls": JSONSerialization.jsonObject(with: JSONEncoder().encode(calls))])
            _ = try await channel.send("POST", "/local/models", body: body).checked()
        }
        if !Self.isTesting { UserDefaults.standard.set(false, forKey: Self.pendingCoreConfig) }
        coreInitialized = TimelineService.shared.rootPath
    }

    func coreComplete(_ request: ChatRequest, model: ModelRef, context: CallContext) async throws -> ChatResponse {
        try await syncCore()
        guard let channel = TimelineService.shared.channel else { throw AIError.network(L("时间线库尚未打开")) }
        let messages: [[String: Any]] = request.messages.map { message in
            var parts: [[String: Any]] = [["type": "text", "text": message.content]]
            for image in message.images ?? [] { parts.append(["type": "image_url", "image_url": ["url": "data:" + image.mimeType + ";base64," + image.data.base64EncodedString()]]) }
            return ["role": message.role.rawValue, "content": parts]
        }
        var payload: [String: Any] = ["model": ["provider_id": model.providerID, "model_id": model.modelID], "feature": context.feature, "role": context.modelRole ?? context.feature, "messages": messages, "max_tokens": request.maxTokens ?? 1024, "temperature": request.temperature ?? 0, "json_mode": request.jsonMode]
        if let runID = context.runID { payload["run_id"] = runID }
        let body = try JSONSerialization.data(withJSONObject: payload)
        let data = try await channel.send("POST", "/local/models/complete", body: body, timeout: 150).checked()
        struct Response: Decodable { var text: String; var call_id: String; var usage: Usage?
            struct Usage: Decodable { var prompt_tokens: Int?; var completion_tokens: Int? } }
        let response = try JSONDecoder().decode(Response.self, from: data)
        return ChatResponse(text: response.text, usage: TokenUsage(input: response.usage?.prompt_tokens, output: response.usage?.completion_tokens), callID: response.call_id)
    }
}
