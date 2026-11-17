import Foundation

// TEMPORARY persistence. timeline-v1 §4.9 puts these tables in the SQLite timeline store; once the
// TimelineStore session lands, implement ProviderConfigStore / AIUsageSink / AIUsageSource over
// ai_providers, ai_models, ai_roles and ai_calls and delete this file.

/// Config as one JSON file (default `Application Support/Candid/ai-providers/config.json`). Holds no secrets.
public final class JSONFileProviderConfigStore: ProviderConfigStore, @unchecked Sendable {
    public let url: URL
    private let lock = NSLock()

    public init(url: URL = JSONFileProviderConfigStore.defaultURL) { self.url = url }

    public static var defaultURL: URL {
        URL.applicationSupportDirectory.appending(path: "Candid/ai-providers/config.json", directoryHint: .notDirectory)
    }

    public func load() throws -> ProviderConfig {
        try lock.withLock {
            guard FileManager.default.fileExists(atPath: url.path) else { return ProviderConfig() }
            return try JSONDecoder().decode(ProviderConfig.self, from: Data(contentsOf: url))
        }
    }

    public func save(_ config: ProviderConfig) throws {
        for p in config.providers { try ProviderConfig.validate(p) }
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        let data = try encoder.encode(config)
        try lock.withLock {
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try data.write(to: url, options: .atomic)
            try? FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
        }
    }
}


/// Config kept in memory; for tests and previews.
public final class InMemoryProviderConfigStore: ProviderConfigStore, @unchecked Sendable {
    private let lock = NSLock()
    private var config: ProviderConfig
    public init(_ config: ProviderConfig = ProviderConfig()) { self.config = config }
    public func load() throws -> ProviderConfig { lock.withLock { config } }
    public func save(_ config: ProviderConfig) throws { lock.withLock { self.config = config } }
}

/// Usage rows in memory only.
public actor InMemoryUsageSink: AIUsageSink, AIUsageSource {
    public private(set) var rows: [AICallRecord] = []
    public init() {}
    public func record(_ call: AICallRecord) { rows.append(call) }
    public func calls(from: Int64, to: Int64) -> [AICallRecord] {
        rows.filter { $0.startedMs >= from && $0.startedMs < to }.sorted { $0.startedMs < $1.startedMs }
    }
}

/// One JSON line per call, appended to a file. Rows never contain prompt or response text.
public actor JSONLUsageSink: AIUsageSink, AIUsageSource {
    public let url: URL
    private var cache: [AICallRecord]?

    public init(url: URL = JSONLUsageSink.defaultURL) { self.url = url }

    public static var defaultURL: URL {
        URL.applicationSupportDirectory.appending(path: "Candid/ai-providers/ai_calls.jsonl", directoryHint: .notDirectory)
    }

    public func record(_ call: AICallRecord) {
        let encoder = JSONEncoder(); encoder.outputFormatting = [.sortedKeys]
        guard var line = try? encoder.encode(call) else { return }
        line.append(0x0A)
        let fm = FileManager.default
        try? fm.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        if !fm.fileExists(atPath: url.path) {
            fm.createFile(atPath: url.path, contents: nil, attributes: [.posixPermissions: 0o600])
        }
        if let handle = try? FileHandle(forWritingTo: url) {
            defer { try? handle.close() }
            _ = try? handle.seekToEnd()
            try? handle.write(contentsOf: line)
        }
        cache?.append(call)
    }

    public func calls(from: Int64, to: Int64) -> [AICallRecord] {
        if cache == nil { cache = Self.read(url) }
        return (cache ?? []).filter { $0.startedMs >= from && $0.startedMs < to }.sorted { $0.startedMs < $1.startedMs }
    }

    private static func read(_ url: URL) -> [AICallRecord] {
        guard let data = try? Data(contentsOf: url) else { return [] }
        let decoder = JSONDecoder()
        return data.split(separator: 0x0A).compactMap { try? decoder.decode(AICallRecord.self, from: Data($0)) }
    }
}
