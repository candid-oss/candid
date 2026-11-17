import Foundation

public enum ChatRole: String, Codable, Sendable { case system, user, assistant }

/// An image attached to a message, sent inline (base64) to APIs that accept image input.
public struct ChatImage: Codable, Sendable, Hashable {
    public var data: Data
    public var mimeType: String
    public init(data: Data, mimeType: String = "image/jpeg") { self.data = data; self.mimeType = mimeType }
}

public struct ChatMessage: Codable, Sendable, Hashable {
    public var role: ChatRole
    public var content: String
    /// Images shown with this message (user messages only). nil or empty = text only.
    public var images: [ChatImage]?
    public init(_ role: ChatRole, _ content: String, images: [ChatImage]? = nil) {
        self.role = role; self.content = content; self.images = images
    }
    var hasImages: Bool { !(images ?? []).isEmpty }
}

public struct ChatRequest: Sendable {
    public var messages: [ChatMessage]
    public var maxTokens: Int?
    public var temperature: Double?
    /// Ask for a JSON object (OpenAI `response_format`); ignored by APIs without the switch.
    public var jsonMode: Bool
    public init(messages: [ChatMessage], maxTokens: Int? = nil, temperature: Double? = nil, jsonMode: Bool = false) {
        self.messages = messages; self.maxTokens = maxTokens; self.temperature = temperature; self.jsonMode = jsonMode
    }
    public init(_ prompt: String, system: String? = nil, maxTokens: Int? = nil) {
        self.init(messages: (system.map { [ChatMessage(.system, $0)] } ?? []) + [ChatMessage(.user, prompt)], maxTokens: maxTokens)
    }
}

/// Token counts as the timeline stores them: `input` excludes cache-read and cache-write tokens,
/// whichever way the API counts (OpenAI includes cached tokens in prompt_tokens, Anthropic does not).
public struct TokenUsage: Sendable, Hashable, Codable {
    public var input: Int?
    public var output: Int?
    public var cacheRead: Int?
    public var cacheWrite: Int?
    /// Cost the provider itself reported (e.g. OpenRouter `usage.cost`), in `reportedCurrency`.
    public var reportedCost: Double?
    public var reportedCurrency: String?
    public init(input: Int? = nil, output: Int? = nil, cacheRead: Int? = nil, cacheWrite: Int? = nil,
                reportedCost: Double? = nil, reportedCurrency: String? = nil) {
        self.input = input; self.output = output; self.cacheRead = cacheRead; self.cacheWrite = cacheWrite
        self.reportedCost = reportedCost; self.reportedCurrency = reportedCurrency
    }
    public var isEmpty: Bool { input == nil && output == nil && cacheRead == nil && cacheWrite == nil }

    /// Fields present in `other` replace fields here (streams report usage in pieces).
    mutating func merge(_ other: TokenUsage) {
        input = other.input ?? input; output = other.output ?? output
        cacheRead = other.cacheRead ?? cacheRead; cacheWrite = other.cacheWrite ?? cacheWrite
        reportedCost = other.reportedCost ?? reportedCost; reportedCurrency = other.reportedCurrency ?? reportedCurrency
    }
}

public struct ChatResponse: Sendable {
    public init(text: String, usage: TokenUsage, finishReason: String? = nil, servedModel: String? = nil, callID: String) {
        self.text = text; self.usage = usage; self.finishReason = finishReason; self.servedModel = servedModel; self.callID = callID
    }
    public var text: String
    public var usage: TokenUsage
    public var finishReason: String?
    /// Model name the server answered with (may differ from the alias that was requested).
    public var servedModel: String?
    public var callID: String
}

public enum StreamEvent: Sendable {
    case text(String)
    case finished(ChatResponse)
}

public enum AIError: Error, Equatable, Sendable {
    case unknownProvider(String)
    case unknownModel(ModelRef)
    case providerDisabled(String)
    case unsupportedAPI(ProviderAPI)
    case missingSecret(String)
    case roleUnassigned(String)
    case budgetExceeded(role: String)
    case http(status: Int, message: String)
    case network(String)
    case timeout
    case cancelled
    case decoding(String)
    case noUsableModel(String)

    /// Short stable code stored in `ai_calls.error_code`.
    public var code: String {
        switch self {
        case .http(let status, _): "http_\(status)"
        case .network: "network"
        case .timeout: "timeout"
        case .cancelled: "cancelled"
        case .decoding: "decode"
        case .missingSecret: "no_secret"
        case .unknownProvider, .unknownModel, .roleUnassigned, .noUsableModel: "config"
        case .providerDisabled: "disabled"
        case .unsupportedAPI: "unsupported_api"
        case .budgetExceeded: "budget"
        }
    }

    /// Worth trying the next fallback: the model could not be reached, not "the request is wrong".
    var allowsFallback: Bool {
        switch self {
        case .network, .timeout, .missingSecret, .providerDisabled: true
        case .http(let status, _): status == 408 || status == 429 || status >= 500
        default: false
        }
    }
}

/// Removes anything that looks like the configured key from text that may reach logs or the UI.
enum Redactor {
    static func scrub(_ text: String, secret: String?) -> String {
        var out = text
        if let secret, secret.count >= 6 { out = out.replacingOccurrences(of: secret, with: "•••") }
        if out.count > 300 { out = String(out.prefix(300)) + "…" }
        return out
    }
}

extension AIError: LocalizedError {
    public var errorDescription: String? {
        switch self {
        case .roleUnassigned(let role): "还没有为「\(ModelRoleInfo.info(role)?.title ?? role)」选择模型。请在 设置 → 模型 → 角色 里选择。"
        case .noUsableModel(let role): "「\(ModelRoleInfo.info(role)?.title ?? role)」没有可用的模型：所选来源已停用或缺少密钥。"
        case .unknownProvider: "所选模型的来源已被删除，请重新选择模型。"
        case .unknownModel(let ref): "来源里已没有模型 \(ref.modelID)，请重新测试来源或改选模型。"
        case .providerDisabled: "所选模型的来源已停用。"
        case .unsupportedAPI(let api): "暂不支持 \(api.rawValue) 接口。"
        case .missingSecret: "这个来源需要密钥，请在 设置 → 模型 → 来源 里填写。"
        case .budgetExceeded(let role): "「\(ModelRoleInfo.info(role)?.title ?? role)」今天的用量已达上限。"
        case .http(let status, let message):
            status == 401 || status == 403 ? "模型服务拒绝访问（\(status)），请检查密钥。"
                : "模型服务返回 \(status)\(message.isEmpty ? "" : "：" + String(message.prefix(160)))"
        case .network(let message): "连不上模型服务：\(message)"
        case .timeout: "模型服务响应超时。"
        case .cancelled: "已取消。"
        case .decoding(let message): "模型服务返回的内容无法识别：\(message)"
        }
    }
}

