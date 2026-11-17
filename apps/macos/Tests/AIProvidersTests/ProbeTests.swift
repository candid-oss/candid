import Foundation
import Testing
@testable import AIProviders

@Suite struct ProbeTests {
    private func provider(_ api: ProviderAPI, host: String, base: String = "/v1", key: Bool = false) -> AIProvider {
        AIProvider(providerID: "p", name: "t", api: api, baseURL: "https://\(host)\(base)", secretRef: key ? "k" : nil, destination: .cloud)
    }

    @Test func openAICompatibleCatalogWithPricingAndCapabilities() async throws {
        let body = """
        {"data":[
          {"id":"vendor/big","name":"Big","context_length":200000,"top_provider":{"max_completion_tokens":8192},
           "architecture":{"input_modalities":["text","image"]},"supported_parameters":["tools","response_format","temperature"],
           "pricing":{"prompt":"0.000003","completion":"0.000015"}},
          {"id":"bare-model"}]}
        """
        let host = "p\(UUID().uuidString.prefix(6).lowercased()).stub.test"
        let session = StubProtocol.session(host: host) { _, _ in .init(body: json(body)) }
        let secrets = InMemorySecretStore(["k": "sk-1"])
        let result = await CapabilityProbe(secrets: secrets, session: session).probe(provider(.openaiChat, host: host, key: true))
        #expect(result.reachable && result.latencyMs != nil && result.error == nil)
        let big = try #require(result.models.first { $0.modelID == "vendor/big" })
        #expect(big.contextWindow == 200_000 && big.maxOutput == 8192 && big.supportsTools == true && big.supportsJSONSchema == true)
        #expect(big.modalities == ["text", "image"] && big.supportsImages == true && big.probedMs == result.probedMs)
        #expect(big.cost?.source == .catalog)
        #expect(abs((big.cost?.perMtok.input ?? 0) - 3) < 1e-9 && abs((big.cost?.perMtok.output ?? 0) - 15) < 1e-9)
        let bare = try #require(result.models.first { $0.modelID == "bare-model" })
        #expect(bare.supportsTools == nil && bare.contextWindow == nil && bare.cost == nil)
        #expect(StubProtocol.requests(host).first?.0.value(forHTTPHeaderField: "Authorization") == "Bearer sk-1")
    }

    /// Regression: LM Studio's /v1/models lists ids only, which used to be recorded as "text only".
    @Test func lmStudioNativeAPIStatesVisionAndUnknownStaysUnknown() async throws {
        let host = "l\(UUID().uuidString.prefix(6).lowercased()).stub.test"
        let session = StubProtocol.session(host: host) { req, _ in
            switch req.url?.path {
            case "/v1/models": return .init(body: json(#"{"object":"list","data":[{"id":"google/gemma-4-e4b","object":"model"},{"id":"qwen/qwen3-8b","object":"model"},{"id":"mystery","object":"model"}]}"#))
            case "/api/v1/models": return .init(body: json(#"{"models":[{"type":"llm","key":"google/gemma-4-e4b","max_context_length":131072,"capabilities":{"vision":true,"trained_for_tool_use":true},"loaded_instances":[]},{"type":"llm","key":"qwen/qwen3-8b","max_context_length":40960,"capabilities":{"vision":false,"trained_for_tool_use":true},"loaded_instances":[{"id":"qwen/qwen3-8b","config":{"context_length":8192}}]}]}"#))
            default: return .init(status: 404, body: json(#"{"error":"not found"}"#))
            }
        }
        let lm = AIProvider(providerID: "p", name: "LM Studio", api: .openaiChat, baseURL: "https://\(host)/v1", secretRef: "k", destination: .onDevice)
        let result = await CapabilityProbe(secrets: InMemorySecretStore(["k": "lm-token"]), session: session).probe(lm)
        let gemma = try #require(result.models.first { $0.modelID == "google/gemma-4-e4b" })
        let qwen = try #require(result.models.first { $0.modelID == "qwen/qwen3-8b" })
        let mystery = try #require(result.models.first { $0.modelID == "mystery" })
        #expect(gemma.supportsImages == true && gemma.supportsTools == true && gemma.contextWindow == 131_072)
        #expect(qwen.supportsImages == false && qwen.supportsTools == true)
        #expect(mystery.supportsImages == nil && mystery.supportsTools == nil)
        // The native call carries the same token as the compatible one.
        let native = try #require(StubProtocol.requests(host).first { $0.0.url?.path == "/api/v1/models" }?.0)
        #expect(native.value(forHTTPHeaderField: "Authorization") == "Bearer lm-token")
    }

    @Test func modelsSavedBeforeTheFixReadAsUnknownNotTextOnly() throws {
        let old = #"{"provider_id":"p","model_id":"m","modalities_json":["text"],"probed_ms":1}"#
        let m = try JSONDecoder().decode(AIModel.self, from: Data(old.utf8))
        #expect(m.supportsImages == nil)
    }

    @Test func anthropicModelList() async throws {
        let body = #"{"data":[{"id":"claude-x","display_name":"Claude X","max_input_tokens":200000,"max_tokens":64000,"capabilities":{"image_input":{"supported":true},"structured_outputs":{"supported":false}}}]}"#
        let host = "a\(UUID().uuidString.prefix(6).lowercased()).stub.test"
        let session = StubProtocol.session(host: host) { _, _ in .init(body: json(body)) }
        let result = await CapabilityProbe(secrets: InMemorySecretStore(["k": "sk"]), session: session).probe(provider(.anthropicMessages, host: host, base: "", key: true))
        let m = try #require(result.models.first)
        #expect(m.displayName == "Claude X" && m.contextWindow == 200_000 && m.maxOutput == 64_000 && m.modalities.contains("image") && m.supportsJSONSchema == false)
        let req = try #require(StubProtocol.requests(host).first?.0)
        #expect(req.url?.path == "/v1/models" && req.value(forHTTPHeaderField: "x-api-key") == "sk")
    }

    @Test func ollamaTagsAndShow() async throws {
        let host = "o\(UUID().uuidString.prefix(6).lowercased()).stub.test"
        let session = StubProtocol.session(host: host) { req, body in
            if req.url?.path == "/api/tags" { return .init(body: json(#"{"models":[{"name":"qwen3:4b","model":"qwen3:4b"},{"name":"llava:7b","model":"llava:7b"}]}"#)) }
            let model = bodyJSON(body)["model"] as? String
            return model == "llava:7b"
                ? .init(body: json(#"{"capabilities":["completion","vision"],"model_info":{"llama.context_length":4096}}"#))
                : .init(body: json(#"{"capabilities":["completion","tools"],"model_info":{"qwen3.context_length":40960}}"#))
        }
        let result = await CapabilityProbe(secrets: InMemorySecretStore(), session: session).probe(provider(.ollama, host: host, base: ""))
        let qwen = try #require(result.models.first { $0.modelID == "qwen3:4b" })
        let llava = try #require(result.models.first { $0.modelID == "llava:7b" })
        #expect(qwen.supportsTools == true && qwen.contextWindow == 40960 && qwen.supportsImages == false)
        #expect(llava.supportsTools == false && llava.supportsImages == true && llava.contextWindow == 4096)
    }

    @Test func failuresAreReportedNotThrown() async throws {
        let host = "f\(UUID().uuidString.prefix(6).lowercased()).stub.test"
        let denied = StubProtocol.session(host: host) { _, _ in .init(status: 401, body: json(#"{"error":{"message":"no"}}"#)) }
        var r = await CapabilityProbe(secrets: InMemorySecretStore(), session: denied).probe(provider(.openaiChat, host: host))
        #expect(!r.reachable && r.error?.contains("401") == true)
        let down = StubProtocol.session(host: host) { _, _ in .init(body: Data(), error: URLError(.cannotConnectToHost)) }
        r = await CapabilityProbe(secrets: InMemorySecretStore(), session: down).probe(provider(.openaiChat, host: host))
        #expect(!r.reachable && r.models.isEmpty)
        let junk = StubProtocol.session(host: host) { _, _ in .init(body: json("<html>")) }
        r = await CapabilityProbe(secrets: InMemorySecretStore(), session: junk).probe(provider(.openaiChat, host: host))
        #expect(!r.reachable && r.error?.contains("格式") == true)
        let unsupported = await CapabilityProbe(secrets: InMemorySecretStore()).probe(provider(.gemini, host: host))
        #expect(!unsupported.reachable)
    }
}
