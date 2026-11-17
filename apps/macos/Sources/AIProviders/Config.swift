import Foundation

public enum ConfigError: Error, Equatable, Sendable {
    case unknownProvider(String)
    case unknownModel(ModelRef)
    case fallbackLessLocal(role: String, from: Destination, to: Destination)
    case secretInConfig(String)
    case invalidBaseURL(String)
}

/// Whole provider configuration: the contents of ai_providers + ai_models + ai_roles.
public struct ProviderConfig: Codable, Sendable, Equatable {
    public var version: Int = 1
    public var providers: [AIProvider] = []
    public var models: [AIModel] = []
    public var roles: [RoleAssignment] = []
    public init(providers: [AIProvider] = [], models: [AIModel] = [], roles: [RoleAssignment] = []) {
        self.providers = providers; self.models = models; self.roles = roles
    }

    public func provider(_ id: String) -> AIProvider? { providers.first { $0.providerID == id } }
    public func model(_ ref: ModelRef) -> AIModel? { models.first { $0.providerID == ref.providerID && $0.modelID == ref.modelID } }
    public func models(of providerID: String) -> [AIModel] { models.filter { $0.providerID == providerID } }
    public func assignment(_ role: String) -> RoleAssignment? { roles.first { $0.role == role } }

    public func destination(of ref: ModelRef) -> Destination? { provider(ref.providerID)?.destination }

    /// Fallbacks that are legal for `assignment`: same destination or more local than the primary.
    public func allowedFallbacks(for assignment: RoleAssignment) -> [ModelRef] {
        guard let primary = assignment.model, let base = destination(of: primary) else { return [] }
        return assignment.fallback.filter { ref in destination(of: ref).map { $0 <= base } ?? false }
    }

    /// Models that may be added as fallback for a primary model in the given destination.
    public func fallbackCandidates(primary: ModelRef) -> [AIModel] {
        guard let base = destination(of: primary) else { return [] }
        return models.filter { m in
            m.ref != primary && (provider(m.providerID).map { $0.enabled && $0.destination <= base } ?? false)
        }
    }

    /// Throws when the role breaks the "never fall back to a less local destination" rule or references unknown things.
    public func validate(_ assignment: RoleAssignment) throws {
        guard let primary = assignment.model else {
            if assignment.fallback.isEmpty { return }
            throw ConfigError.unknownModel(assignment.fallback[0])
        }
        guard let base = destination(of: primary) else { throw ConfigError.unknownProvider(primary.providerID) }
        guard model(primary) != nil else { throw ConfigError.unknownModel(primary) }
        for ref in assignment.fallback {
            guard let d = destination(of: ref) else { throw ConfigError.unknownProvider(ref.providerID) }
            guard model(ref) != nil else { throw ConfigError.unknownModel(ref) }
            if d > base { throw ConfigError.fallbackLessLocal(role: assignment.role, from: base, to: d) }
        }
    }

    public mutating func setAssignment(_ assignment: RoleAssignment) throws {
        try validate(assignment)
        if let i = roles.firstIndex(where: { $0.role == assignment.role }) { roles[i] = assignment } else { roles.append(assignment) }
    }

    public mutating func upsert(_ provider: AIProvider) throws {
        try Self.validate(provider)
        var p = provider; p.updatedMs = nowMs()
        if let i = providers.firstIndex(where: { $0.providerID == p.providerID }) { providers[i] = p } else { providers.append(p) }
        // Changing destination can make existing fallbacks illegal; drop those rather than silently allow them.
        roles = roles.map { r in var r = r; r.fallback = allowedFallbacks(for: r); return r }
    }

    public mutating func remove(providerID: String) {
        providers.removeAll { $0.providerID == providerID }
        models.removeAll { $0.providerID == providerID }
        roles = roles.map { r in
            var r = r
            if r.model?.providerID == providerID { r.model = nil }
            r.fallback.removeAll { $0.providerID == providerID }
            return r
        }
    }

    /// Replaces the model list of one provider with probe output, keeping user-entered costs for models that remain.
    public mutating func merge(probed: [AIModel], for providerID: String) {
        let old = Dictionary(uniqueKeysWithValues: models(of: providerID).map { ($0.modelID, $0) })
        var merged: [AIModel] = probed.map { m in
            var m = m
            if let prev = old[m.modelID], let manual = prev.cost, manual.source == .manual { m.cost = manual }
            return m
        }
        // Models still referenced by a role must not vanish from under it.
        let kept = Set(merged.map(\.modelID))
        for (id, prev) in old where !kept.contains(id) && isReferenced(prev.ref) { merged.append(prev) }
        models.removeAll { $0.providerID == providerID }
        models.append(contentsOf: merged)
    }

    /// Adds a model the user named (e.g. migrated from older settings) without touching probed ones.
    public mutating func ensureModel(_ model: AIModel) {
        guard self.model(model.ref) == nil else { return }
        models.append(model)
    }

    private func isReferenced(_ ref: ModelRef) -> Bool {
        roles.contains { $0.model == ref || $0.fallback.contains(ref) }
    }

    static let secretHeaderNames: Set<String> = ["authorization", "proxy-authorization", "x-api-key", "api-key", "x-goog-api-key", "cookie"]

    /// Providers are persisted verbatim, so nothing that looks like a credential may be part of one.
    public static func validate(_ p: AIProvider) throws {
        guard let url = URL(string: p.baseURL.trimmingCharacters(in: .whitespaces)), let scheme = url.scheme?.lowercased(),
              scheme == "http" || scheme == "https", url.host != nil else { throw ConfigError.invalidBaseURL(p.baseURL) }
        if url.user != nil || url.password != nil { throw ConfigError.secretInConfig("base_url") }
        let query = (url.query ?? "").lowercased()
        if ["key=", "token=", "secret=", "api_key"].contains(where: query.contains) { throw ConfigError.secretInConfig("base_url") }
        for name in p.headers.keys where secretHeaderNames.contains(name.lowercased()) { throw ConfigError.secretInConfig(name) }
        if let ref = p.secretRef, ref.hasPrefix("sk-") || ref.count > 80 { throw ConfigError.secretInConfig("secret_ref") }
    }
}

// MARK: - Persistence protocols

public protocol ProviderConfigStore: Sendable {
    func load() throws -> ProviderConfig
    func save(_ config: ProviderConfig) throws
}

public protocol AIUsageSink: Sendable {
    func record(_ call: AICallRecord) async
}

public protocol AIUsageSource: Sendable {
    /// Calls with `from <= started_ms < to`, oldest first.
    func calls(from: Int64, to: Int64) async -> [AICallRecord]
}

// MARK: - Secrets

public protocol SecretStore: Sendable {
    func secret(for ref: String) throws -> String?
    func setSecret(_ value: String, for ref: String) throws
    func deleteSecret(for ref: String) throws
    /// For background work: never shows a system prompt; a secret that would need one reads as unavailable.
    func secretWithoutPrompt(for ref: String) throws -> String?
    /// Whether a secret exists, without reading (decrypting) it, so it never prompts.
    func hasSecret(for ref: String) throws -> Bool
}

public extension SecretStore {
    func secretWithoutPrompt(for ref: String) throws -> String? { try secret(for: ref) }
    func hasSecret(for ref: String) throws -> Bool { try secret(for: ref) != nil }
}

public final class InMemorySecretStore: SecretStore, @unchecked Sendable {
    private let lock = NSLock()
    private var values: [String: String] = [:]
    public init(_ initial: [String: String] = [:]) { values = initial }
    public func secret(for ref: String) throws -> String? { lock.withLock { values[ref] } }
    public func setSecret(_ value: String, for ref: String) throws { lock.withLock { values[ref] = value } }
    public func deleteSecret(for ref: String) throws { lock.withLock { values[ref] = nil } }
}
