import Foundation
import CandidLocalization
import CandidShared

public struct ProbeResult: Sendable {
    public var reachable: Bool
    public var latencyMs: Int?
    public var models: [AIModel]
    public var error: String?
    public var probedMs: Int64
}

public struct CapabilityProbe: Sendable {
    private let secrets: SecretStore
    private let sessionOverride: URLSession?
    /// Without an explicit session, each request follows 设置 → 模型 → 网络代理.
    private var session: URLSession { sessionOverride ?? NetworkProxy.session() }
    public init(secrets: SecretStore, session: URLSession? = nil) { self.secrets = secrets; self.sessionOverride = session }

    /// Lists models and records whatever capabilities the API exposes. Unknown stays nil; nothing is guessed from names.
    public func probe(_ provider: AIProvider) async -> ProbeResult {
        let stamp = nowMs()
        let clock = ContinuousClock(); let t0 = clock.now
        var key: String?
        do {
            if let ref = provider.secretRef { key = try secrets.secret(for: ref) }
            let models: [AIModel]
            switch provider.api {
            case .openaiChat: models = await enrichFromLMStudio(try await fetch(OpenAIChatWire.self, provider, key), provider, key)
            case .anthropicMessages: models = try await fetch(AnthropicWire.self, provider, key)
            case .ollama: models = try await ollama(provider)
            default: throw AIError.unsupportedAPI(provider.api)
            }
            let d = t0.duration(to: clock.now)
            let ms = Int(d.components.seconds * 1000 + d.components.attoseconds / 1_000_000_000_000_000)
            return ProbeResult(reachable: true, latencyMs: ms, models: models.map { var m = $0; m.probedMs = stamp; return m }.sorted { $0.modelID < $1.modelID }, error: nil, probedMs: stamp)
        } catch {
            let mapped = AIClient.map(error, key: key)
            return ProbeResult(reachable: false, latencyMs: nil, models: [], error: Self.describe(mapped), probedMs: stamp)
        }
    }

    static func describe(_ e: AIError) -> String {
        switch e {
        case .http(let s, let m): s == 401 || s == 403 ? L("密钥无效或无权限（%@）", s) : L("服务返回 %@%@", s, m.isEmpty ? "" : L("：%@", m))
        case .network(let m): L("连不上：%@", m)
        case .timeout: L("连接超时")
        case .missingSecret: L("还没有保存密钥")
        case .decoding(let m): L("响应格式不认识：%@", m)
        case .unsupportedAPI(let api): L("暂不支持 %@", api.rawValue)
        default: e.code
        }
    }

    private func get(_ request: URLRequest) async throws -> Data {
        let (data, response) = try await session.data(for: request)
        if let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
            throw AIError.http(status: http.statusCode, message: errorMessage(from: data))
        }
        return data
    }

    private func fetch(_ wire: any WireFormat.Type, _ provider: AIProvider, _ key: String?) async throws -> [AIModel] {
        try wire.parseModels(try await get(try wire.modelsRequest(provider: provider, apiKey: key)), providerID: provider.providerID)
    }

    /// Best effort: only servers that expose LM Studio's native API change anything.
    private func enrichFromLMStudio(_ models: [AIModel], _ provider: AIProvider, _ key: String?) async -> [AIModel] {
        for request in LMStudioNative.requests(provider: provider, apiKey: key) {
            guard let data = try? await get(request) else { continue }
            let infos = LMStudioNative.parse(data)
            guard !infos.isEmpty else { continue }
            return models.map { m in
                var m = m
                if let info = infos[m.modelID] { LMStudioNative.apply(info, to: &m) }
                return m
            }
        }
        return models
    }

    private func ollama(_ provider: AIProvider) async throws -> [AIModel] {
        var models = try OllamaProbe.parseTags(try await get(try OllamaProbe.tagsRequest(provider: provider)), providerID: provider.providerID)
        for i in models.indices.prefix(40) {
            if let data = try? await get(try OllamaProbe.showRequest(provider: provider, model: models[i].modelID)) {
                OllamaProbe.apply(show: data, to: &models[i])
            }
        }
        return models
    }
}
