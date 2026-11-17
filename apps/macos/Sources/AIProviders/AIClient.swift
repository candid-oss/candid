import Foundation
import CandidShared

public struct CallContext: Sendable {
    public var feature: String
    public var client: CallClient
    public var runID: String?
    public var modelRole: String? = nil
    public init(feature: String, client: CallClient = .app, runID: String? = nil) {
        self.feature = feature; self.client = client; self.runID = runID
    }
}

/// The single entry for model calls from the app. Every attempt, successful or not, becomes one `ai_calls` row
/// (counts, status, timing, sizes; never prompt or response text).
public final class AIClient: Sendable {
    public typealias CoreComplete = @Sendable (ChatRequest, ModelRef, CallContext) async throws -> ChatResponse
    private let coreComplete: CoreComplete?
    private let configStore: ProviderConfigStore
    private let secrets: SecretStore
    private let usage: AIUsageSink
    private let usageSource: AIUsageSource?
    private let sessionOverride: URLSession?
    /// Without an explicit session, each request follows 设置 → 模型 → 网络代理.
    private var session: URLSession { sessionOverride ?? NetworkProxy.session() }

    public init(configStore: ProviderConfigStore, secrets: SecretStore, usage: AIUsageSink,
                usageSource: AIUsageSource? = nil, session: URLSession? = nil, coreComplete: CoreComplete? = nil) {
        self.coreComplete = coreComplete
        self.configStore = configStore; self.secrets = secrets; self.usage = usage
        self.usageSource = usageSource ?? (usage as? AIUsageSource); self.sessionOverride = session
    }

    static func wire(for api: ProviderAPI) throws -> any WireFormat.Type {
        switch api {
        case .openaiChat, .ollama: OpenAIChatWire.self
        case .anthropicMessages: AnthropicWire.self
        default: throw AIError.unsupportedAPI(api)
        }
    }

    // MARK: Direct model

    public func complete(_ request: ChatRequest, model ref: ModelRef, context: CallContext) async throws -> ChatResponse {
        if let coreComplete { return try await coreComplete(request, ref, context) }
        let config = try configStore.load()
        return try await attempt(request, ref: ref, config: config, context: context, stream: nil)
    }

    public func stream(_ request: ChatRequest, model ref: ModelRef, context: CallContext) -> AsyncThrowingStream<StreamEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    if let coreComplete {
                        let response = try await coreComplete(request, ref, context)
                        continuation.yield(.text(response.text)); continuation.yield(.finished(response)); continuation.finish(); return
                    }
                    let config = try configStore.load()
                    _ = try await attempt(request, ref: ref, config: config, context: context, stream: continuation)
                    continuation.finish()
                } catch { continuation.finish(throwing: error) }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    // MARK: By role

    /// Calls the role's model; on an unreachable/overloaded model tries the role's fallbacks, which can only be
    /// the same destination or more local (a cloud fallback behind an on-device primary is never called).
    public func complete(_ request: ChatRequest, role: String, context: CallContext) async throws -> ChatResponse {
        let config = try configStore.load()
        guard let assignment = config.assignment(role), let primary = assignment.model else { throw AIError.roleUnassigned(role) }
        if let coreComplete { var routed = context; routed.modelRole = role; return try await coreComplete(request, primary, routed) }
        try await checkBudget(assignment, config: config)
        var request = request
        if let cap = assignment.limits.maxOutputTokens { request.maxTokens = min(request.maxTokens ?? cap, cap) }
        var chain = [primary] + config.allowedFallbacks(for: assignment)
        chain = chain.reduce(into: []) { if !$0.contains($1) { $0.append($1) } }
        var lastError: AIError = .noUsableModel(role)
        for ref in chain {
            do { return try await attempt(request, ref: ref, config: config, context: context, stream: nil) }
            catch let e as AIError where e.allowsFallback { lastError = e; continue }
        }
        throw lastError
    }

    private func checkBudget(_ assignment: RoleAssignment, config: ProviderConfig) async throws {
        guard let budget = assignment.limits.dailyBudget, let source = usageSource else { return }
        let start = Int64(Calendar.current.startOfDay(for: Date()).timeIntervalSince1970 * 1000)
        let spent = await source.calls(from: start, to: nowMs() + 1)
            .filter { $0.status == .ok && $0.feature == assignment.role }
            .compactMap(\.cost).reduce(0, +)
        if spent >= budget { throw AIError.budgetExceeded(role: assignment.role) }
    }

    // MARK: One attempt

    private func attempt(_ request: ChatRequest, ref: ModelRef, config: ProviderConfig, context: CallContext,
                         stream continuation: AsyncThrowingStream<StreamEvent, Error>.Continuation?) async throws -> ChatResponse {
        guard let provider = config.provider(ref.providerID) else { throw AIError.unknownProvider(ref.providerID) }
        let model = config.model(ref)
        let started = nowMs()
        var row = AICallRecord(startedMs: started, feature: context.feature, client: context.client, runID: context.runID,
                               providerID: ref.providerID, modelID: ref.modelID, destination: provider.destination, status: .error)
        let clock = ContinuousClock(); let t0 = clock.now
        var apiKey: String?
        do {
            guard provider.enabled else { throw AIError.providerDisabled(provider.name) }
            let wire = try Self.wire(for: provider.api)
            if let secretRef = provider.secretRef {
                guard let key = try secrets.secret(for: secretRef), !key.isEmpty else { throw AIError.missingSecret(provider.name) }
                apiKey = key
            }
            let urlRequest = try wire.chatRequest(provider: provider, model: ref.modelID, request: request,
                                                  stream: continuation != nil, apiKey: apiKey)
            row.requestBytes = urlRequest.httpBody?.count
            let outcome: (ParsedChat, Int)
            if let continuation { outcome = try await runStream(urlRequest, wire: wire, continuation: continuation, key: apiKey) }
            else { outcome = try await runPlain(urlRequest, wire: wire, key: apiKey) }
            let parsed = outcome.0
            let estimate = CostCalculator.cost(usage: parsed.usage, model: model)
            row.status = .ok
            row.inputTokens = parsed.usage.input; row.outputTokens = parsed.usage.output
            row.cacheReadTokens = parsed.usage.cacheRead; row.cacheWriteTokens = parsed.usage.cacheWrite
            row.cost = estimate.amount; row.currency = estimate.currency; row.costSource = estimate.source
            row.responseBytes = outcome.1
            Self.finish(&row, t0: t0, clock: clock)
            await usage.record(row)
            let response = ChatResponse(text: parsed.text, usage: parsed.usage, finishReason: parsed.finishReason,
                                        servedModel: parsed.servedModel, callID: row.callID)
            continuation?.yield(.finished(response))
            return response
        } catch {
            let mapped = Self.map(error, key: apiKey)
            switch mapped {
            case .cancelled: row.status = .cancelled
            case .timeout: row.status = .timeout
            default: row.status = .error
            }
            row.errorCode = mapped.code
            row.costSource = .unknown
            Self.finish(&row, t0: t0, clock: clock)
            await usage.record(row)
            throw mapped
        }
    }

    private static func finish(_ row: inout AICallRecord, t0: ContinuousClock.Instant, clock: ContinuousClock) {
        let d = t0.duration(to: clock.now)
        let ms = Int(d.components.seconds * 1000 + d.components.attoseconds / 1_000_000_000_000_000)
        row.latencyMs = ms
        row.endedMs = row.startedMs + Int64(ms)
    }

    static func map(_ error: Error, key: String?) -> AIError {
        if let e = error as? AIError {
            if case .http(let s, let m) = e { return .http(status: s, message: Redactor.scrub(m, secret: key)) }
            return e
        }
        if error is CancellationError { return .cancelled }
        if let u = error as? URLError {
            switch u.code {
            case .cancelled: return .cancelled
            case .timedOut: return .timeout
            default: return .network(Redactor.scrub(u.localizedDescription, secret: key))
            }
        }
        return .network(Redactor.scrub(String(describing: type(of: error)), secret: key))
    }

    private func runPlain(_ request: URLRequest, wire: any WireFormat.Type, key: String?) async throws -> (ParsedChat, Int) {
        let (data, response) = try await session.data(for: request)
        try Task.checkCancellation()
        if let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
            throw AIError.http(status: http.statusCode, message: errorMessage(from: data))
        }
        return (try wire.parseChat(data), data.count)
    }

    private func runStream(_ request: URLRequest, wire: any WireFormat.Type,
                           continuation: AsyncThrowingStream<StreamEvent, Error>.Continuation, key: String?) async throws -> (ParsedChat, Int) {
        let (bytes, response) = try await session.bytes(for: request)
        if let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
            var body = Data()
            for try await b in bytes { body.append(b); if body.count > 4096 { break } }
            throw AIError.http(status: http.statusCode, message: errorMessage(from: body))
        }
        var acc = wire.makeStreamAccumulator()
        var text = ""
        var count = 0
        for try await line in bytes.lines {
            try Task.checkCancellation()
            count += line.utf8.count + 1
            if let delta = try acc.feed(line: line) { text += delta; continuation.yield(.text(delta)) }
        }
        return (ParsedChat(text: text, usage: acc.usage, finishReason: acc.finishReason, servedModel: acc.servedModel), count)
    }
}
