import Foundation
import CryptoKit

public enum DesktopSamplingMode: String, Codable, Sendable, Hashable, CaseIterable {
    case balanced, detailed
}

public struct DesktopSamplingDiagnostics: Sendable, Equatable {
    public var signals = 0
    public var contentReads = 0
    public var unchangedChecks = 0
    public var snapshotsSaved = 0
    public var imageAttempts = 0
    public var imageSaved = 0
    public var listReloads = 0
    public init() {}
}

/// Content equality only. Idle time, caret, focus label, power and thermal state
/// do not make a new document; actual text (including whitespace) stays exact.
public enum DesktopContentSignature {
    public static func make(_ record: DesktopRecord) -> String {
        fingerprint([record.bundleID, record.windowTitle, record.context?.readableText,
                     record.context?.selectedText, record.context?.textStatus,
                     record.context?.recordEvidence?.input_value, record.context?.recordEvidence?.input_status,
                     record.context?.recordEvidence?.confirmation?.message_id, record.context?.recordEvidence?.field, record.context?.recordEvidence?.object.key,
                     record.context?.isProtected.map { String($0) }])
    }
    public static func source(bundleID: String, pid: Int32?, windowIdentity: UInt?, title: String) -> String {
        fingerprint([bundleID, pid.map { String($0) }, windowIdentity.map { String($0) }, title])
    }
    private static func fingerprint(_ values: [String?]) -> String {
        // JSON array preserves boundaries and absent vs empty text.
        let bytes = (try? JSONEncoder().encode(values)) ?? Data()
        return SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined()
    }
}

/// User-controlled sampling policy; immutable protection checks live outside it.
public struct DesktopSamplingOptions: Codable, Sendable, Equatable {
    public static let samplingIntervalRange: ClosedRange<Double> = 1...300
    public static let samplingCapacityRange = 1...60
    public static let periodicIntervalRange: ClosedRange<Double> = 5...3600
    public var windowTriggersEnabled = true
    public var returnEnabled = true
    public var scrollEnabled = true
    public var minimumContentInterval: Double = 15
    public var contentCapacity = 4
    public var imageCapacity = 4
    public var imageFallbackInterval: Double = 120
    public init() {}
    public var isValid: Bool {
        Self.samplingIntervalRange.contains(minimumContentInterval) &&
        Self.samplingCapacityRange.contains(contentCapacity) && Self.samplingCapacityRange.contains(imageCapacity) &&
        Self.periodicIntervalRange.contains(imageFallbackInterval)
    }
}
