import Foundation

/// The capture settings, kept by the App as `preferences.json` in its capture folder (platform settings: what to
/// capture and how often). Written atomically; an invalid file is refused rather than silently reset.
public struct DesktopPreferencesFile: Sendable {
    public let url: URL
    public init(directory: URL) { url = directory.appendingPathComponent("preferences.json") }

    public var exists: Bool { FileManager.default.fileExists(atPath: url.path) }

    public func load() throws -> DesktopPreferences {
        guard exists else { return DesktopPreferences() }
        let preferences = try JSONDecoder().decode(DesktopPreferences.self, from: Data(contentsOf: url))
        try Self.validate(preferences)
        return preferences
    }

    public func save(_ preferences: DesktopPreferences) throws {
        try Self.validate(preferences)
        let encoder = JSONEncoder(); encoder.outputFormatting = [.sortedKeys, .prettyPrinted]
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        try DesktopOutbox.writeDurably(try encoder.encode(preferences), to: url)
    }

    /// Settings an older version kept in the desktop ledger, decoded and checked the same way.
    public static func decodeLegacy(_ data: Data) throws -> DesktopPreferences {
        let preferences = try JSONDecoder().decode(DesktopPreferences.self, from: data)
        try validate(preferences)
        return preferences
    }

    public static func validate(_ preferences: DesktopPreferences) throws {
        guard preferences.maximumBytes >= 1024 * 1024, preferences.samplingOptions.isValid, preferences.imageOptions.isValid,
              preferences.minimumCaptureInterval.isFinite, preferences.periodicInterval.isFinite,
              DesktopSamplingOptions.samplingIntervalRange.contains(preferences.minimumCaptureInterval),
              DesktopSamplingOptions.periodicIntervalRange.contains(preferences.periodicInterval),
              (1...3650).contains(preferences.retentionDays),
              preferences.allowedBundleIDs.count <= 256, preferences.excludedBundleIDs.count <= 256,
              preferences.windowCompatibilityBundleIDs.isSubset(of: preferences.allowedBundleIDs),
              preferences.windowCompatibilityBundleIDs.isDisjoint(with: preferences.excludedBundleIDs),
              preferences.textAllowedBundleIDs.count <= 256,
              preferences.contentOptions.maximumChangeWait.map({ $0.isFinite && (5...60).contains($0) }) ?? true else { throw DesktopStoreError.invalidRecord }
    }
}
