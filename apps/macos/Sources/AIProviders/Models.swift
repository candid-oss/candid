import Foundation

// Value types mirroring timeline-v1 §4.9 (ai_providers / ai_models / ai_roles / ai_calls).
// JSON keys are the SQL column names so the SQLite-backed stores can reuse them.

public enum ProviderAPI: String, Codable, Sendable, CaseIterable, Hashable {
    case openaiChat = "openai-chat"
    case openaiResponses = "openai-responses"
    case anthropicMessages = "anthropic-messages"
    case gemini
    case ollama
    case other

    /// Kinds this module can talk to today.
    public var isImplemented: Bool {
        switch self {
        case .openaiChat, .anthropicMessages, .ollama: true
        case .openaiResponses, .gemini, .other: false
        }
    }
}

/// Where the prompt goes. Ordered from most to least local.
public enum Destination: String, Codable, Sendable, CaseIterable, Hashable, Comparable {
    case onDevice = "on_device"
    case selfHosted = "self_hosted"
    case cloud

    public var locality: Int {
        switch self { case .onDevice: 0; case .selfHosted: 1; case .cloud: 2 }
    }
    public static func < (a: Destination, b: Destination) -> Bool { a.locality < b.locality }
}

public struct AIProvider: Codable, Sendable, Hashable, Identifiable {
    public var providerID: String
    public var name: String
    public var api: ProviderAPI
    public var baseURL: String
    /// Reference into the SecretStore. Never the key itself.
    public var secretRef: String?
    public var destination: Destination
    public var enabled: Bool
    /// Extra non-secret headers.
    public var headers: [String: String]
    public var createdMs: Int64
    public var updatedMs: Int64

    public var id: String { providerID }

    public init(providerID: String = UUID().uuidString.lowercased(), name: String, api: ProviderAPI, baseURL: String,
                secretRef: String? = nil, destination: Destination, enabled: Bool = true,
                headers: [String: String] = [:], createdMs: Int64 = nowMs(), updatedMs: Int64 = nowMs()) {
        self.providerID = providerID; self.name = name; self.api = api; self.baseURL = baseURL
        self.secretRef = secretRef; self.destination = destination; self.enabled = enabled
        self.headers = headers; self.createdMs = createdMs; self.updatedMs = updatedMs
    }

    enum CodingKeys: String, CodingKey {
        case providerID = "provider_id", name, api, baseURL = "base_url", secretRef = "secret_ref", destination, enabled
        case headers = "headers_json", createdMs = "created_ms", updatedMs = "updated_ms"
    }
}

public struct ModelCost: Codable, Sendable, Hashable {
    public struct PerMTok: Codable, Sendable, Hashable {
        public var input: Double?
        public var output: Double?
        public var cacheRead: Double?
        public var cacheWrite: Double?
        public init(input: Double? = nil, output: Double? = nil, cacheRead: Double? = nil, cacheWrite: Double? = nil) {
            self.input = input; self.output = output; self.cacheRead = cacheRead; self.cacheWrite = cacheWrite
        }
        enum CodingKeys: String, CodingKey { case input, output, cacheRead = "cache_read", cacheWrite = "cache_write" }
    }
    public enum Source: String, Codable, Sendable { case catalog, manual }

    public var currency: String
    public var perMtok: PerMTok
    public var source: Source
    public var asOf: String?

    public init(currency: String = "USD", perMtok: PerMTok, source: Source, asOf: String? = nil) {
        self.currency = currency; self.perMtok = perMtok; self.source = source; self.asOf = asOf
    }
    enum CodingKeys: String, CodingKey { case currency, perMtok = "per_mtok", source, asOf = "as_of" }
}

public struct ModelRef: Codable, Sendable, Hashable {
    public var providerID: String
    public var modelID: String
    public init(providerID: String, modelID: String) { self.providerID = providerID; self.modelID = modelID }
    enum CodingKeys: String, CodingKey { case providerID = "provider_id", modelID = "model_id" }
}

public struct AIModel: Codable, Sendable, Hashable, Identifiable {
    public var providerID: String
    public var modelID: String
    public var displayName: String?
    public var contextWindow: Int?
    public var maxOutput: Int?
    public var modalities: [String]
    /// true when the server stated the input modalities; nil/false means it did not say,
    /// so `["text"]` is only a default and must not be read as "no images".
    public var modalitiesDeclared: Bool?
    /// nil = not probed / API does not say.
    public var supportsTools: Bool?
    public var supportsJSONSchema: Bool?
    public var cost: ModelCost?
    public var probedMs: Int64?

    public var id: String { providerID + "/" + modelID }
    public var ref: ModelRef { ModelRef(providerID: providerID, modelID: modelID) }
    public var supportsImages: Bool? {
        if modalities.contains("image") { return true }
        return modalitiesDeclared == true ? false : nil
    }

    public init(providerID: String, modelID: String, displayName: String? = nil, contextWindow: Int? = nil,
                maxOutput: Int? = nil, modalities: [String] = ["text"], supportsTools: Bool? = nil,
                supportsJSONSchema: Bool? = nil, cost: ModelCost? = nil, probedMs: Int64? = nil) {
        self.providerID = providerID; self.modelID = modelID; self.displayName = displayName
        self.contextWindow = contextWindow; self.maxOutput = maxOutput; self.modalities = modalities
        self.supportsTools = supportsTools; self.supportsJSONSchema = supportsJSONSchema
        self.cost = cost; self.probedMs = probedMs
    }

    enum CodingKeys: String, CodingKey {
        case providerID = "provider_id", modelID = "model_id", displayName = "display_name"
        case contextWindow = "context_window", maxOutput = "max_output", modalities = "modalities_json"
        case modalitiesDeclared = "modalities_declared"
        case supportsTools = "supports_tools", supportsJSONSchema = "supports_json_schema"
        case cost = "cost_json", probedMs = "probed_ms"
    }
}

public struct RoleLimits: Codable, Sendable, Hashable {
    public var maxInputTokens: Int?
    public var maxOutputTokens: Int?
    /// Daily spend ceiling in the models' currency (USD for the built-in presets).
    public var dailyBudget: Double?
    public init(maxInputTokens: Int? = nil, maxOutputTokens: Int? = nil, dailyBudget: Double? = nil) {
        self.maxInputTokens = maxInputTokens; self.maxOutputTokens = maxOutputTokens; self.dailyBudget = dailyBudget
    }
    enum CodingKeys: String, CodingKey { case maxInputTokens = "max_input_tokens", maxOutputTokens = "max_output_tokens", dailyBudget = "daily_budget" }
}

public struct RoleAssignment: Codable, Sendable, Hashable, Identifiable {
    public var role: String
    /// A built-in engine (e.g. on-device text recognition) used instead of a model. `model` wins when both are set.
    public var engine: String?
    /// The user's own prompt for this role; nil means the role's default prompt.
    public var prompt: String?
    public var model: ModelRef?
    /// Tried in order. Each must be the same destination as `model` or more local.
    public var fallback: [ModelRef]
    public var limits: RoleLimits
    public var id: String { role }
    public init(role: String, model: ModelRef? = nil, fallback: [ModelRef] = [], limits: RoleLimits = RoleLimits()) {
        self.role = role; self.model = model; self.fallback = fallback; self.limits = limits
    }
    enum CodingKeys: String, CodingKey { case role, engine, prompt, model, fallback = "fallback_json", limits = "limits_json" }
}

public enum CallClient: String, Codable, Sendable { case app, pi, worker }
public enum CallStatus: String, Codable, Sendable { case ok, error, cancelled, timeout }
public enum CostSource: String, Codable, Sendable { case provider, estimated, unknown }

/// One row of `ai_calls`. Contains no prompt or response text.
public struct AICallRecord: Codable, Sendable, Hashable, Identifiable {
    public var callID: String
    public var startedMs: Int64
    public var endedMs: Int64?
    public var feature: String
    public var client: CallClient
    public var runID: String?
    public var providerID: String
    public var modelID: String
    public var destination: Destination
    public var inputTokens: Int?
    public var outputTokens: Int?
    public var cacheReadTokens: Int?
    public var cacheWriteTokens: Int?
    public var cost: Double?
    public var currency: String?
    public var costSource: CostSource?
    public var latencyMs: Int?
    public var status: CallStatus
    public var errorCode: String?
    public var requestBytes: Int?
    public var responseBytes: Int?

    public var id: String { callID }

    public init(callID: String = UUID().uuidString.lowercased(), startedMs: Int64, endedMs: Int64? = nil, feature: String,
                client: CallClient = .app, runID: String? = nil, providerID: String, modelID: String, destination: Destination,
                inputTokens: Int? = nil, outputTokens: Int? = nil, cacheReadTokens: Int? = nil, cacheWriteTokens: Int? = nil,
                cost: Double? = nil, currency: String? = nil, costSource: CostSource? = nil, latencyMs: Int? = nil,
                status: CallStatus, errorCode: String? = nil, requestBytes: Int? = nil, responseBytes: Int? = nil) {
        self.callID = callID; self.startedMs = startedMs; self.endedMs = endedMs; self.feature = feature
        self.client = client; self.runID = runID; self.providerID = providerID; self.modelID = modelID
        self.destination = destination; self.inputTokens = inputTokens; self.outputTokens = outputTokens
        self.cacheReadTokens = cacheReadTokens; self.cacheWriteTokens = cacheWriteTokens; self.cost = cost
        self.currency = currency; self.costSource = costSource; self.latencyMs = latencyMs; self.status = status
        self.errorCode = errorCode; self.requestBytes = requestBytes; self.responseBytes = responseBytes
    }

    enum CodingKeys: String, CodingKey {
        case callID = "call_id", startedMs = "started_ms", endedMs = "ended_ms", feature, client, runID = "run_id"
        case providerID = "provider_id", modelID = "model_id", destination
        case inputTokens = "input_tokens", outputTokens = "output_tokens"
        case cacheReadTokens = "cache_read_tokens", cacheWriteTokens = "cache_write_tokens"
        case cost, currency, costSource = "cost_source", latencyMs = "latency_ms", status, errorCode = "error_code"
        case requestBytes = "request_bytes", responseBytes = "response_bytes"
    }
}

public func nowMs() -> Int64 { Int64((Date().timeIntervalSince1970 * 1000).rounded()) }
