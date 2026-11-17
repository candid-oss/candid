import Foundation
import Testing
@testable import AIProviders

private func config() -> ProviderConfig {
    let local = AIProvider(providerID: "local", name: "LM Studio", api: .openaiChat, baseURL: "http://localhost:1234/v1", destination: .onDevice)
    let nas = AIProvider(providerID: "nas", name: "NAS", api: .openaiChat, baseURL: "http://nas.local:8000/v1", destination: .selfHosted)
    let cloud = AIProvider(providerID: "cloud", name: "Anthropic", api: .anthropicMessages, baseURL: "https://api.anthropic.com", secretRef: "provider.cloud", destination: .cloud)
    return ProviderConfig(providers: [local, nas, cloud], models: [
        AIModel(providerID: "local", modelID: "qwen"), AIModel(providerID: "nas", modelID: "big"), AIModel(providerID: "cloud", modelID: "claude")
    ])
}

@Suite struct FallbackRuleTests {
    @Test func onDeviceRoleRefusesCloudAndSelfHostedFallback() throws {
        var c = config()
        let local = ModelRef(providerID: "local", modelID: "qwen")
        for bad in [ModelRef(providerID: "cloud", modelID: "claude"), ModelRef(providerID: "nas", modelID: "big")] {
            #expect(throws: ConfigError.self) { try c.setAssignment(RoleAssignment(role: "summary", model: local, fallback: [bad])) }
        }
        #expect(c.roles.isEmpty)
        #expect(c.fallbackCandidates(primary: local).isEmpty)
    }

    @Test func cloudRoleMayFallBackToMoreLocal() throws {
        var c = config()
        let cloud = ModelRef(providerID: "cloud", modelID: "claude")
        try c.setAssignment(RoleAssignment(role: "agent_chat", model: cloud, fallback: [ModelRef(providerID: "nas", modelID: "big"), ModelRef(providerID: "local", modelID: "qwen")]))
        #expect(c.assignment("agent_chat")?.fallback.count == 2)
        #expect(c.fallbackCandidates(primary: cloud).count == 2)
    }

    @Test func changingDestinationDropsNowIllegalFallbacks() throws {
        var c = config()
        let nas = ModelRef(providerID: "nas", modelID: "big")
        try c.setAssignment(RoleAssignment(role: "summary", model: nas, fallback: [ModelRef(providerID: "local", modelID: "qwen")]))
        var local = try #require(c.provider("local")); local.destination = .cloud
        try c.upsert(local)
        #expect(c.assignment("summary")?.fallback.isEmpty == true)
    }

    @Test func clientNeverCallsCloudBehindLocalPrimary() async throws {
        // Hand-edited config file with an illegal fallback: the local primary is down, the cloud one must stay untouched.
        var c = config()
        c.providers[0].baseURL = "http://local.stub.test/v1"
        c.providers[2].baseURL = "https://cloud.stub.test"
        let local = ModelRef(providerID: "local", modelID: "qwen")
        c.roles = [RoleAssignment(role: "summary", model: local, fallback: [ModelRef(providerID: "cloud", modelID: "claude")])]
        let secrets = InMemorySecretStore(["provider.cloud": "k"])
        let sink = InMemoryUsageSink()
        let host = "local.stub.test"
        let session = StubProtocol.session(host: host) { _, _ in .init(body: Data(), error: URLError(.cannotConnectToHost)) }
        _ = StubProtocol.session(host: "cloud.stub.test") { _, _ in .init(body: json(#"{"content":[{"type":"text","text":"leak"}],"usage":{}}"#)) }
        let client = AIClient(configStore: InMemoryProviderConfigStore(c), secrets: secrets, usage: sink, usageSource: sink, session: session)
        await #expect(throws: AIError.self) { try await client.complete(ChatRequest("x"), role: "summary", context: .init(feature: "summary")) }
        #expect(StubProtocol.requests("cloud.stub.test").isEmpty)
        #expect(await sink.rows.map(\.providerID) == ["local"])
    }

    @Test func roleFallsBackWithinAllowedChainAndRecordsEachAttempt() async throws {
        var c = config()
        c.providers[1].baseURL = "http://nas.stub.test/v1"
        c.providers[0].baseURL = "http://local2.stub.test/v1"
        let ok = #"{"choices":[{"message":{"content":"from-local"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#
        try c.setAssignment(RoleAssignment(role: "summary", model: ModelRef(providerID: "nas", modelID: "big"), fallback: [ModelRef(providerID: "local", modelID: "qwen")]))
        _ = StubProtocol.session(host: "local2.stub.test") { _, _ in .init(body: json(ok)) }
        let session = StubProtocol.session(host: "nas.stub.test") { _, _ in .init(status: 503, body: json("{}")) }
        let sink = InMemoryUsageSink()
        // Both hosts are registered in the shared handler table; the session itself only has to carry StubProtocol.
        let client = AIClient(configStore: InMemoryProviderConfigStore(c), secrets: InMemorySecretStore(), usage: sink, usageSource: sink, session: session)
        let r = try await client.complete(ChatRequest("x"), role: "summary", context: .init(feature: "summary"))
        #expect(r.text == "from-local")
        let rows = await sink.rows
        #expect(rows.map(\.providerID) == ["nas", "local"] && rows.map(\.status) == [.error, .ok])
    }

    @Test func roleLimitsClampOutputAndEnforceBudget() async throws {
        var c = config()
        c.providers[0].baseURL = "http://budget.stub.test/v1"
        c.models[0].cost = ModelCost(perMtok: .init(input: 1000, output: 1000), source: .manual)
        let ref = ModelRef(providerID: "local", modelID: "qwen")
        try c.setAssignment(RoleAssignment(role: "summary", model: ref, limits: RoleLimits(maxOutputTokens: 64, dailyBudget: 0.5)))
        let reply = #"{"choices":[{"message":{"content":"x"}}],"usage":{"prompt_tokens":500,"completion_tokens":500}}"#
        let session = StubProtocol.session(host: "budget.stub.test") { _, _ in .init(body: json(reply)) }
        let sink = InMemoryUsageSink()
        let client = AIClient(configStore: InMemoryProviderConfigStore(c), secrets: InMemorySecretStore(), usage: sink, usageSource: sink, session: session)
        _ = try await client.complete(ChatRequest("x", maxTokens: 4000), role: "summary", context: .init(feature: "summary"))
        #expect(bodyJSON(try #require(StubProtocol.requests("budget.stub.test").first).1)["max_tokens"] as? Int == 64)
        // 1000 tokens × $1000/M = $1.00 spent, over the $0.50 budget.
        await #expect(throws: AIError.budgetExceeded(role: "summary")) {
            try await client.complete(ChatRequest("x"), role: "summary", context: .init(feature: "summary"))
        }
    }
}

@Suite struct PersistenceTests {
    @Test func secretsNeverReachConfigFileOrUsageLog() async throws {
        let dir = FileManager.default.temporaryDirectory.appending(path: "aip-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = JSONFileProviderConfigStore(url: dir.appending(path: "config.json"))
        let secrets = InMemorySecretStore()
        let secret = "sk-ant-VERY-SECRET-VALUE"
        var c = config()
        let ref = try #require(c.provider("cloud")?.secretRef)
        try secrets.setSecret(secret, for: ref)
        try store.save(c)
        let text = try String(contentsOf: store.url, encoding: .utf8)
        #expect(!text.contains(secret) && text.contains("provider.cloud"))
        #expect(try store.load() == c)
        let mode = try FileManager.default.attributesOfItem(atPath: store.url.path)[.posixPermissions] as? Int
        #expect(mode == 0o600)

        let log = JSONLUsageSink(url: dir.appending(path: "calls.jsonl"))
        await log.record(AICallRecord(startedMs: 1000, feature: "t", providerID: "cloud", modelID: "claude", destination: .cloud, status: .ok))
        await log.record(AICallRecord(startedMs: 2000, feature: "t", providerID: "cloud", modelID: "claude", destination: .cloud, status: .error, errorCode: "http_500"))
        let reread = JSONLUsageSink(url: dir.appending(path: "calls.jsonl"))
        #expect(await reread.calls(from: 0, to: 3000).count == 2)
        #expect(await reread.calls(from: 1500, to: 3000).first?.errorCode == "http_500")
        c.providers[0].enabled = false
        try store.save(c)
        #expect(try store.load().providers[0].enabled == false)
    }

    @Test func rejectsCredentialsInProviderFields() throws {
        func make(_ url: String = "https://api.example.com/v1", headers: [String: String] = [:]) -> AIProvider {
            AIProvider(name: "x", api: .openaiChat, baseURL: url, destination: .cloud, headers: headers)
        }
        #expect(throws: ConfigError.self) { try ProviderConfig.validate(make(headers: ["Authorization": "Bearer abc"])) }
        #expect(throws: ConfigError.self) { try ProviderConfig.validate(make(headers: ["x-api-key": "abc"])) }
        #expect(throws: ConfigError.self) { try ProviderConfig.validate(make("https://user:pw@api.example.com/v1")) }
        #expect(throws: ConfigError.self) { try ProviderConfig.validate(make("https://api.example.com/v1?api_key=abc")) }
        #expect(throws: ConfigError.self) { try ProviderConfig.validate(make("not a url")) }
        #expect(throws: Never.self) { try ProviderConfig.validate(make(headers: ["HTTP-Referer": "https://candid.local"])) }
        // The file store refuses to write them too.
        let store = JSONFileProviderConfigStore(url: FileManager.default.temporaryDirectory.appending(path: "aip-\(UUID().uuidString).json"))
        #expect(throws: ConfigError.self) { try store.save(ProviderConfig(providers: [make(headers: ["Authorization": "x"])])) }
    }

    @Test func rowEncodesWithTableColumnNamesAndNoContent() throws {
        let row = AICallRecord(startedMs: 5, feature: "f", client: .pi, providerID: "p", modelID: "m", destination: .selfHosted,
                               inputTokens: 1, costSource: .unknown, status: .cancelled)
        let o = bodyJSON(try JSONEncoder().encode(row))
        let expected: Set<String> = ["call_id", "started_ms", "feature", "client", "provider_id", "model_id", "destination", "input_tokens", "cost_source", "status"]
        #expect(expected.isSubset(of: Set(o.keys)))
        #expect(o["client"] as? String == "pi" && o["destination"] as? String == "self_hosted" && o["status"] as? String == "cancelled")
        #expect(!o.keys.contains { $0.contains("prompt") || $0.contains("response_text") || $0.contains("content") })
    }

    @Test func probeMergeKeepsManualCostAndReferencedModels() throws {
        var c = config()
        c.models[0].cost = ModelCost(perMtok: .init(input: 7, output: 9), source: .manual)
        try c.setAssignment(RoleAssignment(role: "summary", model: ModelRef(providerID: "local", modelID: "qwen")))
        c.merge(probed: [AIModel(providerID: "local", modelID: "qwen", cost: ModelCost(perMtok: .init(input: 1, output: 1), source: .catalog)),
                         AIModel(providerID: "local", modelID: "new")], for: "local")
        #expect(c.model(ModelRef(providerID: "local", modelID: "qwen"))?.cost?.perMtok.input == 7)
        #expect(c.models(of: "local").count == 2)
        c.merge(probed: [AIModel(providerID: "local", modelID: "new")], for: "local")
        #expect(c.model(ModelRef(providerID: "local", modelID: "qwen")) != nil, "role still references it")
    }
}

@Suite struct UsageSummaryTests {
    @Test func separatesReportedEstimatedAndUnknown() {
        func row(_ ms: Int64, _ feature: String, _ src: CostSource?, _ cost: Double?, status: CallStatus = .ok) -> AICallRecord {
            AICallRecord(startedMs: ms, feature: feature, providerID: "p", modelID: "m", destination: .cloud, inputTokens: 10, outputTokens: 5,
                         cacheReadTokens: 2, cost: cost, currency: cost == nil ? nil : "USD", costSource: src, status: status)
        }
        let day = Int64(86_400_000)
        let calls = [row(0, "a", .provider, 1.0), row(1, "a", .estimated, 0.5), row(day * 3, "b", .unknown, nil), row(day * 3, "b", .unknown, nil, status: .error)]
        var cal = Calendar(identifier: .gregorian); cal.timeZone = TimeZone(identifier: "UTC")!
        let t = UsageSummary.totals(calls)
        #expect(t.calls == 4 && t.failed == 1 && t.inputTokens == 40 && t.outputTokens == 20 && t.cacheTokens == 8)
        #expect(t.reportedCost == ["USD": 1.0] && t.estimatedCost == ["USD": 0.5] && t.unknownCostCalls == 1)
        #expect(UsageSummary.grouped(calls, by: .day, calendar: cal).map(\.key) == ["1970-01-01", "1970-01-04"])
        #expect(UsageSummary.grouped(calls, by: .feature).first?.key == "a" || UsageSummary.grouped(calls, by: .feature).count == 2)
    }
}
