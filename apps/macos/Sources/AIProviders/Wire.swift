import Foundation

// Per-API request building and response parsing. Pure functions over Data/URLRequest so they can be tested
// without a network. Parsing is deliberately tolerant: compatible servers omit or rename fields freely.

struct ParsedChat { var text: String; var usage: TokenUsage; var finishReason: String?; var servedModel: String? }

protocol StreamAccumulator {
    /// Feeds one line of the HTTP body; returns a text delta if the line carried one.
    mutating func feed(line: String) throws -> String?
    var usage: TokenUsage { get }
    var finishReason: String? { get }
    var servedModel: String? { get }
    var finished: Bool { get }
}

enum WireURL {
    /// `base + path`, tolerating a trailing slash and a base that already ends in the version segment.
    static func join(_ base: String, _ path: String) -> URL? {
        var b = base.trimmingCharacters(in: .whitespaces)
        while b.hasSuffix("/") { b.removeLast() }
        var p = path
        if p.hasPrefix("/v1/"), b.hasSuffix("/v1") { p.removeFirst(3) }
        return URL(string: b + p)
    }
}

func jsonObject(_ data: Data) -> [String: Any]? { (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] }
private func int(_ any: Any?) -> Int? { (any as? NSNumber).map { Int(truncating: $0) } }
private func dbl(_ any: Any?) -> Double? { (any as? NSNumber).map { Double(truncating: $0) } }

func errorMessage(from data: Data) -> String {
    if let o = jsonObject(data) {
        if let e = o["error"] as? [String: Any], let m = e["message"] as? String { return m }
        if let e = o["error"] as? String { return e }
        if let m = o["message"] as? String { return m }
    }
    return String(data: data.prefix(200), encoding: .utf8) ?? ""
}

/// OpenAI Chat: text-only messages keep a string `content`; with images it becomes content parts.
func openAIMessage(_ m: ChatMessage) -> [String: Any] {
    guard m.hasImages else { return ["role": m.role.rawValue, "content": m.content] }
    var parts: [[String: Any]] = [["type": "text", "text": m.content]]
    for image in m.images ?? [] {
        parts.append(["type": "image_url", "image_url": ["url": "data:\(image.mimeType);base64," + image.data.base64EncodedString()]])
    }
    return ["role": m.role.rawValue, "content": parts]
}

/// Anthropic Messages: images go first as base64 blocks, then the text.
func anthropicMessage(_ m: ChatMessage) -> [String: Any] {
    guard m.hasImages else { return ["role": m.role.rawValue, "content": m.content] }
    var blocks: [[String: Any]] = (m.images ?? []).map {
        ["type": "image", "source": ["type": "base64", "media_type": $0.mimeType, "data": $0.data.base64EncodedString()]]
    }
    blocks.append(["type": "text", "text": m.content])
    return ["role": m.role.rawValue, "content": blocks]
}

protocol WireFormat {
    static func chatRequest(provider: AIProvider, model: String, request: ChatRequest, stream: Bool, apiKey: String?) throws -> URLRequest
    static func parseChat(_ data: Data) throws -> ParsedChat
    static func makeStreamAccumulator() -> StreamAccumulator
    static func modelsRequest(provider: AIProvider, apiKey: String?) throws -> URLRequest
    static func parseModels(_ data: Data, providerID: String) throws -> [AIModel]
}

func baseRequest(url: URL?, method: String, provider: AIProvider, body: Data?, timeout: TimeInterval = 120) throws -> URLRequest {
    guard let url else { throw ConfigError.invalidBaseURL(provider.baseURL) }
    var r = URLRequest(url: url, timeoutInterval: timeout)
    r.httpMethod = method
    for (k, v) in provider.headers { r.setValue(v, forHTTPHeaderField: k) }
    if let body { r.httpBody = body; r.setValue("application/json", forHTTPHeaderField: "Content-Type") }
    return r
}

// MARK: - OpenAI Chat Completions (LM Studio, Ollama /v1, vLLM, OpenRouter, OpenAI)

enum OpenAIChatWire: WireFormat {
    /// Ollama's native kind talks to its OpenAI-compatible endpoint under `/v1`.
    static func v1Base(_ provider: AIProvider) -> String {
        var b = provider.baseURL.trimmingCharacters(in: .whitespaces)
        while b.hasSuffix("/") { b.removeLast() }
        if provider.api == .ollama, !b.hasSuffix("/v1") { b += "/v1" }
        return b
    }

    static func chatRequest(provider: AIProvider, model: String, request: ChatRequest, stream: Bool, apiKey: String?) throws -> URLRequest {
        var body: [String: Any] = [
            "model": model,
            "messages": request.messages.map(openAIMessage),
            "stream": stream
        ]
        if let t = request.maxTokens {
            let official = URL(string: provider.baseURL)?.host == "api.openai.com"
            body[official ? "max_completion_tokens" : "max_tokens"] = t
        }
        if let t = request.temperature { body["temperature"] = t }
        if request.jsonMode { body["response_format"] = ["type": "json_object"] }
        if stream { body["stream_options"] = ["include_usage": true] }
        let data = try JSONSerialization.data(withJSONObject: body, options: [.sortedKeys])
        var r = try baseRequest(url: WireURL.join(v1Base(provider), "/chat/completions"), method: "POST", provider: provider, body: data)
        if let apiKey, !apiKey.isEmpty { r.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization") }
        if stream { r.setValue("text/event-stream", forHTTPHeaderField: "Accept") }
        return r
    }

    static func usage(from object: [String: Any]?) -> TokenUsage {
        guard let u = object else { return TokenUsage() }
        let prompt = int(u["prompt_tokens"])
        let details = u["prompt_tokens_details"] as? [String: Any]
        let cached = int(details?["cached_tokens"]) ?? int(u["cache_read_input_tokens"])
        let written = int(details?["cache_write_tokens"]) ?? int(details?["cache_creation_tokens"])
        let uncached = prompt.map { max(0, $0 - (cached ?? 0) - (written ?? 0)) }
        let cost = dbl(u["cost"])
        return TokenUsage(input: uncached, output: int(u["completion_tokens"]), cacheRead: cached, cacheWrite: written,
                          reportedCost: cost, reportedCurrency: cost == nil ? nil : "USD")
    }

    static func parseChat(_ data: Data) throws -> ParsedChat {
        guard let o = jsonObject(data) else { throw AIError.decoding("response is not a JSON object") }
        let choice = (o["choices"] as? [[String: Any]])?.first
        guard let message = choice?["message"] as? [String: Any] else { throw AIError.decoding("no choices[0].message") }
        let text = message["content"] as? String ?? ""
        return ParsedChat(text: text, usage: usage(from: o["usage"] as? [String: Any]),
                          finishReason: choice?["finish_reason"] as? String, servedModel: o["model"] as? String)
    }

    struct Accumulator: StreamAccumulator {
        var usage = TokenUsage()
        var finishReason: String?
        var servedModel: String?
        var finished = false
        mutating func feed(line: String) throws -> String? {
            guard line.hasPrefix("data:") else { return nil }
            let payload = line.dropFirst(5).trimmingCharacters(in: .whitespaces)
            if payload == "[DONE]" { finished = true; return nil }
            guard let o = jsonObject(Data(payload.utf8)) else { return nil }
            if let e = o["error"] as? [String: Any] { throw AIError.http(status: 500, message: e["message"] as? String ?? "stream error") }
            servedModel = o["model"] as? String ?? servedModel
            if let u = o["usage"] as? [String: Any] { usage.merge(OpenAIChatWire.usage(from: u)) }
            guard let choice = (o["choices"] as? [[String: Any]])?.first else { return nil }
            if let f = choice["finish_reason"] as? String { finishReason = f }
            let text = (choice["delta"] as? [String: Any])?["content"] as? String
            return (text?.isEmpty == false) ? text : nil
        }
    }
    static func makeStreamAccumulator() -> StreamAccumulator { Accumulator() }

    static func modelsRequest(provider: AIProvider, apiKey: String?) throws -> URLRequest {
        var r = try baseRequest(url: WireURL.join(v1Base(provider), "/models"), method: "GET", provider: provider, body: nil, timeout: 15)
        if let apiKey, !apiKey.isEmpty { r.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization") }
        return r
    }

    /// Reads whatever the server volunteers: OpenRouter (context_length, supported_parameters, architecture, pricing),
    /// LM Studio (max_context_length, type, capabilities), vLLM (max_model_len). Everything absent stays nil.
    static func parseModels(_ data: Data, providerID: String) throws -> [AIModel] {
        guard let o = jsonObject(data), let list = o["data"] as? [[String: Any]] else { throw AIError.decoding("no data[] in /models") }
        let stamp = nowMs()
        return list.compactMap { m -> AIModel? in
            guard let id = m["id"] as? String else { return nil }
            var model = AIModel(providerID: providerID, modelID: id, displayName: (m["name"] as? String).flatMap { $0 == id ? nil : $0 }, probedMs: stamp)
            model.contextWindow = int(m["context_length"]) ?? int(m["max_context_length"]) ?? int(m["max_model_len"]) ?? int(m["context_window"])
            if let top = m["top_provider"] as? [String: Any] { model.maxOutput = int(top["max_completion_tokens"]) }
            if let params = m["supported_parameters"] as? [String] {
                model.supportsTools = params.contains("tools")
                model.supportsJSONSchema = params.contains("structured_outputs") || params.contains("response_format")
            }
            if let caps = m["capabilities"] as? [String], model.supportsTools == nil { model.supportsTools = caps.contains("tool_use") || caps.contains("tools") }
            var modalities = ["text"]
            if let inputs = (m["architecture"] as? [String: Any])?["input_modalities"] as? [String] {
                modalities = inputs.isEmpty ? ["text"] : inputs; model.modalitiesDeclared = true
            } else if let type = m["type"] as? String, ["vlm", "llm"].contains(type) {
                modalities = type == "vlm" ? ["text", "image"] : ["text"]; model.modalitiesDeclared = true
            }
            model.modalities = modalities
            if let pricing = m["pricing"] as? [String: Any], let prompt = (pricing["prompt"] as? String).flatMap(Double.init),
               let completion = (pricing["completion"] as? String).flatMap(Double.init), prompt > 0 || completion > 0 {
                model.cost = ModelCost(perMtok: .init(input: prompt * 1e6, output: completion * 1e6,
                                                      cacheRead: (pricing["input_cache_read"] as? String).flatMap(Double.init).map { $0 * 1e6 },
                                                      cacheWrite: (pricing["input_cache_write"] as? String).flatMap(Double.init).map { $0 * 1e6 }),
                                       source: .catalog, asOf: Self.today())
            }
            return model
        }
    }

    static func today() -> String {
        let f = DateFormatter(); f.dateFormat = "yyyy-MM-dd"; f.locale = Locale(identifier: "en_US_POSIX"); return f.string(from: Date())
    }
}

// MARK: - LM Studio native model list

/// LM Studio's OpenAI-compatible `/v1/models` lists ids only. Its native REST API states capabilities:
/// v1 `GET /api/v1/models` → `{"models":[{"key","type","max_context_length","capabilities":{"vision","trained_for_tool_use"},
/// "loaded_instances":[{"id"}]}]}`; legacy v0 `GET /api/v0/models` → `{"data":[{"id","type":"vlm"|"llm"|"embeddings","max_context_length"}]}`.
/// Other OpenAI-compatible servers answer 404 here and are left untouched.
enum LMStudioNative {
    struct Info: Equatable { var vision: Bool?; var tools: Bool?; var context: Int?; var embedding: Bool }

    static func origin(_ provider: AIProvider) -> String? {
        guard let u = URL(string: provider.baseURL.trimmingCharacters(in: .whitespaces)), let scheme = u.scheme, let host = u.host else { return nil }
        return scheme + "://" + host + (u.port.map { ":\($0)" } ?? "")
    }

    static func requests(provider: AIProvider, apiKey: String?) -> [URLRequest] {
        guard let o = origin(provider) else { return [] }
        return ["/api/v1/models", "/api/v0/models"].compactMap { path in
            guard var r = try? baseRequest(url: URL(string: o + path), method: "GET", provider: provider, body: nil, timeout: 8) else { return nil }
            if let apiKey, !apiKey.isEmpty { r.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization") }
            return r
        }
    }

    /// Model id (as used by `/v1/models`) → what LM Studio states about it.
    static func parse(_ data: Data) -> [String: Info] {
        guard let o = jsonObject(data) else { return [:] }
        var out: [String: Info] = [:]
        if let list = o["models"] as? [[String: Any]] {          // v1
            for m in list {
                let caps = m["capabilities"] as? [String: Any]
                let type = m["type"] as? String
                let info = Info(vision: caps?["vision"] as? Bool ?? (type == "llm" ? false : nil),
                                tools: caps?["trained_for_tool_use"] as? Bool,
                                context: (m["max_context_length"] as? NSNumber).map { Int(truncating: $0) },
                                embedding: type == "embedding")
                var ids = Set<String>()
                if let key = m["key"] as? String { ids.insert(key) }
                for inst in (m["loaded_instances"] as? [[String: Any]]) ?? [] { if let id = inst["id"] as? String { ids.insert(id) } }
                for id in ids { out[id] = info }
            }
        } else if let list = o["data"] as? [[String: Any]] {      // v0
            for m in list {
                guard let id = m["id"] as? String else { continue }
                let type = m["type"] as? String
                out[id] = Info(vision: type == "vlm" ? true : (type == "llm" ? false : nil), tools: nil,
                               context: (m["max_context_length"] as? NSNumber).map { Int(truncating: $0) },
                               embedding: type == "embeddings")
            }
        }
        return out
    }

    static func apply(_ info: Info, to model: inout AIModel) {
        if let v = info.vision { model.modalities = v ? ["text", "image"] : ["text"]; model.modalitiesDeclared = true }
        if info.embedding { model.modalities = ["text"]; model.modalitiesDeclared = true }
        if model.supportsTools == nil, let t = info.tools { model.supportsTools = t }
        if model.contextWindow == nil, let c = info.context { model.contextWindow = c }
    }
}

// MARK: - Anthropic Messages

enum AnthropicWire: WireFormat {
    static let version = "2023-06-01"
    static let defaultMaxTokens = 1024

    static func chatRequest(provider: AIProvider, model: String, request: ChatRequest, stream: Bool, apiKey: String?) throws -> URLRequest {
        let system = request.messages.filter { $0.role == .system }.map(\.content).joined(separator: "\n\n")
        var body: [String: Any] = [
            "model": model,
            "max_tokens": request.maxTokens ?? defaultMaxTokens,
            "messages": request.messages.filter { $0.role != .system }.map(anthropicMessage),
            "stream": stream
        ]
        if !system.isEmpty { body["system"] = system }
        if let t = request.temperature { body["temperature"] = t }
        let data = try JSONSerialization.data(withJSONObject: body, options: [.sortedKeys])
        var r = try baseRequest(url: WireURL.join(provider.baseURL, "/v1/messages"), method: "POST", provider: provider, body: data)
        if let apiKey, !apiKey.isEmpty { r.setValue(apiKey, forHTTPHeaderField: "x-api-key") }
        r.setValue(version, forHTTPHeaderField: "anthropic-version")
        if stream { r.setValue("text/event-stream", forHTTPHeaderField: "Accept") }
        return r
    }

    static func usage(from u: [String: Any]?) -> TokenUsage {
        guard let u else { return TokenUsage() }
        return TokenUsage(input: int(u["input_tokens"]), output: int(u["output_tokens"]),
                          cacheRead: int(u["cache_read_input_tokens"]), cacheWrite: int(u["cache_creation_input_tokens"]))
    }

    static func parseChat(_ data: Data) throws -> ParsedChat {
        guard let o = jsonObject(data), let blocks = o["content"] as? [[String: Any]] else { throw AIError.decoding("no content[]") }
        let text = blocks.compactMap { $0["type"] as? String == "text" ? $0["text"] as? String : nil }.joined()
        return ParsedChat(text: text, usage: usage(from: o["usage"] as? [String: Any]),
                          finishReason: o["stop_reason"] as? String, servedModel: o["model"] as? String)
    }

    struct Accumulator: StreamAccumulator {
        var usage = TokenUsage()
        var finishReason: String?
        var servedModel: String?
        var finished = false
        mutating func feed(line: String) throws -> String? {
            guard line.hasPrefix("data:") else { return nil }
            guard let o = jsonObject(Data(line.dropFirst(5).trimmingCharacters(in: .whitespaces).utf8)) else { return nil }
            switch o["type"] as? String {
            case "message_start":
                let message = o["message"] as? [String: Any]
                servedModel = message?["model"] as? String
                usage.merge(AnthropicWire.usage(from: message?["usage"] as? [String: Any]))
            case "content_block_delta":
                let delta = o["delta"] as? [String: Any]
                if delta?["type"] as? String == "text_delta", let t = delta?["text"] as? String, !t.isEmpty { return t }
            case "message_delta":
                finishReason = (o["delta"] as? [String: Any])?["stop_reason"] as? String ?? finishReason
                // message_delta usage carries cumulative output_tokens (and sometimes the input fields again).
                usage.merge(AnthropicWire.usage(from: o["usage"] as? [String: Any]))
            case "message_stop": finished = true
            case "error":
                let e = o["error"] as? [String: Any]
                let overloaded = e?["type"] as? String == "overloaded_error"
                throw AIError.http(status: overloaded ? 529 : 500, message: e?["message"] as? String ?? "stream error")
            default: break
            }
            return nil
        }
    }
    static func makeStreamAccumulator() -> StreamAccumulator { Accumulator() }

    static func modelsRequest(provider: AIProvider, apiKey: String?) throws -> URLRequest {
        var r = try baseRequest(url: WireURL.join(provider.baseURL, "/v1/models?limit=100"), method: "GET", provider: provider, body: nil, timeout: 15)
        if let apiKey, !apiKey.isEmpty { r.setValue(apiKey, forHTTPHeaderField: "x-api-key") }
        r.setValue(version, forHTTPHeaderField: "anthropic-version")
        return r
    }

    /// The list endpoint names models; newer responses also carry token limits and a capabilities object.
    static func parseModels(_ data: Data, providerID: String) throws -> [AIModel] {
        guard let o = jsonObject(data), let list = o["data"] as? [[String: Any]] else { throw AIError.decoding("no data[] in /v1/models") }
        let stamp = nowMs()
        return list.compactMap { m in
            guard let id = m["id"] as? String else { return nil }
            var model = AIModel(providerID: providerID, modelID: id, displayName: m["display_name"] as? String, probedMs: stamp)
            model.contextWindow = int(m["max_input_tokens"])
            model.maxOutput = int(m["max_tokens"])
            if let caps = m["capabilities"] as? [String: Any] {
                func supported(_ key: String) -> Bool? { (caps[key] as? [String: Any])?["supported"] as? Bool }
                model.supportsJSONSchema = supported("structured_outputs")
                if let images = supported("image_input") {
                    model.modalities = images ? ["text", "image"] : ["text"]; model.modalitiesDeclared = true
                }
            }
            // Claude models take tool definitions across the Messages API; the list does not say so, so leave unknown.
            return model
        }
    }
}

// MARK: - Ollama native probe (/api/tags + /api/show)

enum OllamaProbe {
    static func tagsRequest(provider: AIProvider) throws -> URLRequest {
        try baseRequest(url: WireURL.join(provider.baseURL.replacingOccurrences(of: "/v1", with: ""), "/api/tags"),
                        method: "GET", provider: provider, body: nil, timeout: 15)
    }
    static func showRequest(provider: AIProvider, model: String) throws -> URLRequest {
        let body = try JSONSerialization.data(withJSONObject: ["model": model])
        return try baseRequest(url: WireURL.join(provider.baseURL.replacingOccurrences(of: "/v1", with: ""), "/api/show"),
                               method: "POST", provider: provider, body: body, timeout: 10)
    }
    static func parseTags(_ data: Data, providerID: String) throws -> [AIModel] {
        guard let o = jsonObject(data), let list = o["models"] as? [[String: Any]] else { throw AIError.decoding("no models[] in /api/tags") }
        let stamp = nowMs()
        return list.compactMap { m in
            guard let name = (m["model"] as? String) ?? (m["name"] as? String) else { return nil }
            return AIModel(providerID: providerID, modelID: name, probedMs: stamp)
        }
    }
    /// `capabilities` (completion, tools, vision, embedding) and `model_info["<arch>.context_length"]`.
    static func apply(show data: Data, to model: inout AIModel) {
        guard let o = jsonObject(data) else { return }
        if let caps = o["capabilities"] as? [String] {
            model.supportsTools = caps.contains("tools")
            model.modalities = caps.contains("vision") ? ["text", "image"] : ["text"]; model.modalitiesDeclared = true
            if caps.contains("embedding") && !caps.contains("completion") { model.modalities = ["text"] }
        }
        if let info = o["model_info"] as? [String: Any] {
            if let key = info.keys.first(where: { $0.hasSuffix(".context_length") }) { model.contextWindow = int(info[key]) }
        }
    }
}
