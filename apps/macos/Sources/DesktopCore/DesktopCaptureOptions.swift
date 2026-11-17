import Foundation

public struct DesktopContentOptions: Codable, Sendable, Equatable {
    /// Explicit opt-in within the existing text scope. Missing in older preferences means disabled.
    public var inputValueBundleIDs: Set<String>?
    /// Longest wait during a changing AX stream; absent preferences use 10 s.
    public var maximumChangeWait: Double?
    public var windowTitleEnabled = true
    public var readableTextEnabled = true
    public var selectedTextEnabled = true
    public var focusEnabled = true
    public var systemStateEnabled = true
    public init() {}
}

public struct DesktopImageOptions: Codable, Sendable, Equatable {
    public var maximumEdge = 1920
    public var jpegQuality = 0.8
    public var reduceOversizedImages = true
    public init() {}
    public var isValid: Bool {
        [1280, 1920, 2560, 3840].contains(maximumEdge) && jpegQuality.isFinite && (0.5...0.95).contains(jpegQuality)
    }
}

/// Remove disabled fields at the final publication boundary. Transient source
/// identity and protection checks remain independent of persisted content.
public enum DesktopContentRedaction {
    public static func apply(_ original: DesktopRecord, options: DesktopContentOptions) -> DesktopRecord {
        var record = original
        if !options.windowTitleEnabled { record.windowTitle = "" }
        if var context = record.context {
            if !options.readableTextEnabled { context.readableText = nil; context.axEvidence = nil; context.recordEvidence?.reading = [] }
            if options.inputValueBundleIDs?.contains(record.bundleID) != true {
                context.recordEvidence?.confirmation = nil; context.recordEvidence?.input_value = nil; context.recordEvidence?.input_status = "disabled"
            }
            if !options.selectedTextEnabled { context.selectedText = nil }
            if !options.focusEnabled {
                context.focusedRole = nil; context.focusedLabel = nil
                if let ax = context.axEvidence {
                    context.axEvidence = DesktopAXEvidence(nodes: ax.nodes.map { node in
                        var copy = node; copy.focused = nil; return copy
                    }, partial: ax.partial, observedAt: ax.observedAt)
                }
            }
            if !options.systemStateEnabled {
                context.idleSeconds = nil; context.onBattery = nil; context.lowPower = nil; context.thermalState = nil; context.sessionActive = nil
            }
            if !options.readableTextEnabled && !options.selectedTextEnabled && options.inputValueBundleIDs?.contains(record.bundleID) != true && context.isProtected != true && context.isComposing != true { context.textStatus = "disabled" }
            record.context = context
        }
        return record
    }
}
