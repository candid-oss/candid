import Foundation
import Testing
@testable import AIProviders

@Suite struct OpenAIChatTests {
    let reply = """
    {"id":"x","model":"served-m1","choices":[{"message":{"role":"assistant","content":"你好"},"finish_reason":"stop"}],
     "usage":{"prompt_tokens":120,"completion_tokens":30,"prompt_tokens_details":{"cached_tokens":100}}}
    """

    @Test func encodesRequestAndExtractsUsage() async throws {
        let rig = Rig(api: .openaiChat, cost: ModelCost(perMtok: .init(input: 1, output: 2, cacheRead: 0.1), source: .manual)) { _, _ in .init(body: json(reply)) }
        let r = try await rig.client.complete(ChatRequest("hi", system: "be brief", maxTokens: 50), model: rig.model.ref, context: .init(feature: "summary"))
        #expect(r.text == "你好")
        #expect(r.servedModel == "served-m1")
        // OpenAI counts cached tokens inside prompt_tokens; the row stores uncached input separately.
        #expect(r.usage == TokenUsage(input: 20, output: 30, cacheRead: 100))

        let (req, body) = try #require(StubProtocol.requests(rig.host).first)
        #expect(req.url?.path == "/v1/chat/completions")
        #expect(req.value(forHTTPHeaderField: "Authorization") == "Bearer sk-test-SECRET-123")
        let o = bodyJSON(body)
        #expect(o["model"] as? String == "m1")
        #expect(o["max_tokens"] as? Int == 50)
        #expect(o["stream"] as? Bool == false)
        let messages = try #require(o["messages"] as? [[String: String]])
        #expect(messages.map { $0["role"] } == ["system", "user"])

        let rows = await rig.sink.rows
        #expect(rows.count == 1)
        let row = rows[0]
        #expect(row.status == .ok && row.feature == "summary" && row.client == .app)
        #expect(row.inputTokens == 20 && row.outputTokens == 30 && row.cacheReadTokens == 100)
        #expect(row.costSource == .estimated)
        // 20*1 + 30*2 + 100*0.1 = 90 per million
        #expect(abs((row.cost ?? 0) - 90e-6) < 1e-12)
        #expect(row.requestBytes == body.count && row.responseBytes == reply.utf8.count)
        #expect(row.destination == .cloud && row.latencyMs != nil && row.endedMs != nil)
    }

    @Test func providerReportedCostWins() async throws {
        let body = #"{"choices":[{"message":{"content":"ok"}}],"usage":{"prompt_tokens":10,"completion_tokens":5,"cost":0.00042}}"#
        let rig = Rig(api: .openaiChat, cost: ModelCost(perMtok: .init(input: 99, output: 99), source: .manual)) { _, _ in .init(body: json(body)) }
        _ = try await rig.client.complete(ChatRequest("x"), model: rig.model.ref, context: .init(feature: "t"))
        let row = try #require(await rig.sink.rows.first)
        #expect(row.costSource == .provider && row.cost == 0.00042 && row.currency == "USD")
    }

    @Test func localWithoutPriceIsUnknownNotZero() async throws {
        let rig = Rig(api: .openaiChat, destination: .onDevice, key: nil) { req, _ in
            #expect(req.value(forHTTPHeaderField: "Authorization") == nil)
            return .init(body: json(reply))
        }
        _ = try await rig.client.complete(ChatRequest("x"), model: rig.model.ref, context: .init(feature: "t"))
        let row = try #require(await rig.sink.rows.first)
        #expect(row.costSource == .unknown && row.cost == nil)
    }

    @Test func streamingParsesDeltasAndFinalUsage() async throws {
        let sse = """
        data: {"model":"m1","choices":[{"delta":{"role":"assistant"}}]}

        data: {"choices":[{"delta":{"content":"Hel"}}]}

        data: {"choices":[{"delta":{"content":"lo"},"finish_reason":"stop"}]}

        data: {"choices":[],"usage":{"prompt_tokens":8,"completion_tokens":2}}

        data: [DONE]

        """
        let rig = Rig(api: .openaiChat) { _, _ in .init(body: json(sse)) }
        var deltas: [String] = []; var final: ChatResponse?
        for try await e in rig.client.stream(ChatRequest("x"), model: rig.model.ref, context: .init(feature: "agent_chat")) {
            switch e { case .text(let t): deltas.append(t); case .finished(let r): final = r }
        }
        #expect(deltas == ["Hel", "lo"])
        #expect(final?.text == "Hello" && final?.usage.input == 8 && final?.usage.output == 2 && final?.finishReason == "stop")
        let body = bodyJSON(try #require(StubProtocol.requests(rig.host).first).1)
        #expect(body["stream"] as? Bool == true)
        #expect((body["stream_options"] as? [String: Bool])?["include_usage"] == true)
        #expect(await rig.sink.rows.count == 1)
    }

    @Test func httpErrorIsRecordedWithoutLeakingKey() async throws {
        let rig = Rig(api: .openaiChat) { _, _ in .init(status: 401, body: json(#"{"error":{"message":"bad key sk-test-SECRET-123"}}"#)) }
        do {
            _ = try await rig.client.complete(ChatRequest("x"), model: rig.model.ref, context: .init(feature: "t"))
            Issue.record("expected throw")
        } catch let e as AIError {
            #expect(e.code == "http_401")
            if case .http(_, let m) = e { #expect(!m.contains("SECRET")) }
        }
        let row = try #require(await rig.sink.rows.first)
        #expect(row.status == .error && row.errorCode == "http_401")
        let encoded = String(data: try JSONEncoder().encode(row), encoding: .utf8) ?? ""
        #expect(!encoded.contains("SECRET"))
    }

    @Test func timeoutAndMissingKeyAreRecorded() async throws {
        let slow = Rig(api: .openaiChat) { _, _ in .init(body: Data(), error: URLError(.timedOut)) }
        await #expect(throws: AIError.timeout) { try await slow.client.complete(ChatRequest("x"), model: slow.model.ref, context: .init(feature: "t")) }
        #expect(await slow.sink.rows.first?.status == .timeout)

        let rig = Rig(api: .openaiChat) { _, _ in .init(body: json(reply)) }
        try rig.secrets.deleteSecret(for: "provider.test")
        await #expect(throws: AIError.missingSecret("Test")) { try await rig.client.complete(ChatRequest("x"), model: rig.model.ref, context: .init(feature: "t")) }
        #expect(await rig.sink.rows.first?.errorCode == "no_secret")
        #expect(StubProtocol.requests(rig.host).isEmpty)
    }

    @Test func ollamaKindUsesOpenAICompatiblePath() throws {
        let p = AIProvider(name: "O", api: .ollama, baseURL: "http://localhost:11434/", destination: .onDevice)
        let r = try OpenAIChatWire.chatRequest(provider: p, model: "qwen", request: ChatRequest("x"), stream: false, apiKey: nil)
        #expect(r.url?.absoluteString == "http://localhost:11434/v1/chat/completions")
    }
}

@Suite struct AnthropicTests {
    @Test func encodesRequestAndExtractsUsage() async throws {
        let reply = """
        {"model":"claude-x","content":[{"type":"text","text":"Hi "},{"type":"text","text":"there"}],"stop_reason":"end_turn",
         "usage":{"input_tokens":50,"output_tokens":10,"cache_read_input_tokens":200,"cache_creation_input_tokens":30}}
        """
        let cost = ModelCost(perMtok: .init(input: 3, output: 15, cacheRead: 0.3, cacheWrite: 3.75), source: .catalog)
        let rig = Rig(api: .anthropicMessages, cost: cost) { _, _ in .init(body: json(reply)) }
        let r = try await rig.client.complete(ChatRequest("hello", system: "sys"), model: rig.model.ref, context: .init(feature: "episode", client: .worker, runID: "run-1"))
        #expect(r.text == "Hi there")
        #expect(r.usage == TokenUsage(input: 50, output: 10, cacheRead: 200, cacheWrite: 30))

        let (req, body) = try #require(StubProtocol.requests(rig.host).first)
        #expect(req.url?.path == "/v1/messages")
        #expect(req.value(forHTTPHeaderField: "x-api-key") == "sk-test-SECRET-123")
        #expect(req.value(forHTTPHeaderField: "anthropic-version") == "2023-06-01")
        #expect(req.value(forHTTPHeaderField: "Authorization") == nil)
        let o = bodyJSON(body)
        #expect(o["system"] as? String == "sys" && o["max_tokens"] as? Int == 1024)
        #expect((o["messages"] as? [[String: String]])?.map { $0["role"] } == ["user"])

        let row = try #require(await rig.sink.rows.first)
        #expect(row.client == .worker && row.runID == "run-1" && row.costSource == .estimated)
        // 50*3 + 10*15 + 200*0.3 + 30*3.75 = 472.5 per million
        #expect(abs((row.cost ?? 0) - 472.5e-6) < 1e-12)
    }

    @Test func streamingParsesEvents() async throws {
        let sse = """
        event: message_start
        data: {"type":"message_start","message":{"model":"claude-x","usage":{"input_tokens":25,"output_tokens":1,"cache_read_input_tokens":5}}}

        event: content_block_delta
        data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hel"}}

        event: content_block_delta
        data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"lo"}}

        event: message_delta
        data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":15}}

        event: message_stop
        data: {"type":"message_stop"}

        """
        let rig = Rig(api: .anthropicMessages) { _, _ in .init(body: json(sse)) }
        var final: ChatResponse?; var text = ""
        for try await e in rig.client.stream(ChatRequest("x"), model: rig.model.ref, context: .init(feature: "t")) {
            switch e { case .text(let t): text += t; case .finished(let r): final = r }
        }
        #expect(text == "Hello")
        #expect(final?.usage == TokenUsage(input: 25, output: 15, cacheRead: 5))
        #expect(final?.finishReason == "end_turn" && final?.servedModel == "claude-x")
    }

    @Test func streamErrorEventThrows() async throws {
        let sse = "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"busy\"}}\n\n"
        let rig = Rig(api: .anthropicMessages) { _, _ in .init(body: json(sse)) }
        await #expect(throws: AIError.http(status: 529, message: "busy")) {
            for try await _ in rig.client.stream(ChatRequest("x"), model: rig.model.ref, context: .init(feature: "t")) {}
        }
        #expect(await rig.sink.rows.first?.errorCode == "http_529")
    }
}

@Suite struct CostTests {
    @Test func cacheFallsBackToInputPriceAndUnknownWithoutPrice() {
        let m = AIModel(providerID: "p", modelID: "m", cost: ModelCost(perMtok: .init(input: 2, output: 4), source: .manual))
        let e = CostCalculator.cost(usage: TokenUsage(input: 1_000_000, output: 500_000, cacheRead: 1_000_000), model: m)
        #expect(e.source == .estimated && e.amount == 2 + 2 + 2 && e.currency == "USD")
        #expect(CostCalculator.cost(usage: TokenUsage(input: 5), model: AIModel(providerID: "p", modelID: "m")).source == .unknown)
        #expect(CostCalculator.cost(usage: TokenUsage(), model: m).source == .unknown)
    }

    @Test func imagesAreSentAsContentPartsForBothAPIs() throws {
        let image = ChatImage(data: Data([0xFF, 0xD8, 0xFF]), mimeType: "image/jpeg")
        let message = ChatMessage(.user, "describe", images: [image])
        let openai = openAIMessage(message)
        let parts = try #require(openai["content"] as? [[String: Any]])
        #expect(parts.first?["type"] as? String == "text" && parts.first?["text"] as? String == "describe")
        #expect(((parts.last?["image_url"] as? [String: Any])?["url"] as? String) == "data:image/jpeg;base64,/9j/")
        let anthropic = anthropicMessage(message)
        let blocks = try #require(anthropic["content"] as? [[String: Any]])
        #expect(blocks.first?["type"] as? String == "image" && blocks.last?["type"] as? String == "text")
        #expect(((blocks.first?["source"] as? [String: Any])?["media_type"] as? String) == "image/jpeg")
        // Text-only messages keep the plain string form.
        #expect(openAIMessage(ChatMessage(.user, "hi"))["content"] as? String == "hi")
    }
}

