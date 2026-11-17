import Foundation

public enum DesktopRecordKind: String, Codable, Sendable, CaseIterable {
    case application, window, context, screenshot, marker, system, gap
    public var title: String {
        switch self { case .application: "应用活动"; case .window: "窗口变化"; case .context: "文字与状态"; case .screenshot: "屏幕证据"; case .marker: "手动标记"; case .system: "系统事件"; case .gap: "记录间隔" }
    }
    public var symbol: String {
        switch self { case .application: "app"; case .window: "macwindow"; case .context: "text.alignleft"; case .screenshot: "photo"; case .marker: "bookmark"; case .system: "desktopcomputer"; case .gap: "pause.circle" }
    }
}

/// Best-effort OS observations, never inferred text or keystrokes. Missing means
/// unavailable/not requested; it must not be fabricated from another window.
public struct DesktopContextSnapshot: Codable, Sendable, Hashable {
    public var focusedRole: String?
    public var focusedLabel: String?
    public var selectedText: String?
    public var readableText: String?
    public var recordEvidence: DesktopRecordEvidence?
    public var axEvidence: DesktopAXEvidence?
    public var textStatus: String?
    public var isProtected: Bool?
    public var sessionActive: Bool?
    public var onBattery: Bool?
    public var lowPower: Bool?
    public var thermalState: String?
    public var idleSeconds: Double?
    public var isComposing: Bool?
    public init(focusedRole: String? = nil, focusedLabel: String? = nil, selectedText: String? = nil, readableText: String? = nil, textStatus: String? = nil, isProtected: Bool? = nil, sessionActive: Bool? = nil, onBattery: Bool? = nil, lowPower: Bool? = nil, thermalState: String? = nil, idleSeconds: Double? = nil, isComposing: Bool? = nil) {
        self.focusedRole = focusedRole; self.focusedLabel = focusedLabel; self.selectedText = selectedText; self.readableText = readableText; self.textStatus = textStatus; self.isProtected = isProtected; self.sessionActive = sessionActive; self.onBattery = onBattery; self.lowPower = lowPower; self.thermalState = thermalState; self.idleSeconds = idleSeconds
        self.isComposing = isComposing
    }
}

public struct DesktopRecord: Codable, Sendable, Hashable, Identifiable {
    public var id: String
    public var timestamp: Date
    public var kind: DesktopRecordKind
    public var appName: String
    public var bundleID: String
    public var manualTitle: String? = nil
    public var manualNotes: String? = nil
    public var windowTitle: String
    public var detail: String
    public var imageHash: String?
    public var imageWidth: Int?
    public var imageHeight: Int?
    public var parentID: String?
    public var runID: String
    public var trigger: String?
    public var context: DesktopContextSnapshot?
    public var activityID: String?
    public init(id: String = UUID().uuidString, timestamp: Date = Date(), kind: DesktopRecordKind, appName: String = "", bundleID: String = "", windowTitle: String = "", detail: String = "", imageHash: String? = nil, imageWidth: Int? = nil, imageHeight: Int? = nil, parentID: String? = nil, runID: String = "", trigger: String? = nil, context: DesktopContextSnapshot? = nil, activityID: String? = nil) {
        self.id = id; self.timestamp = timestamp; self.kind = kind; self.appName = appName; self.bundleID = bundleID; self.windowTitle = windowTitle; self.detail = detail; self.imageHash = imageHash; self.imageWidth = imageWidth; self.imageHeight = imageHeight; self.parentID = parentID; self.runID = runID
        self.trigger = trigger; self.context = context; self.activityID = activityID
    }
    public var displayTitle: String {
        if let manualTitle { return manualTitle }
        if !windowTitle.isEmpty { return windowTitle }
        if !appName.isEmpty { return appName }
        if (kind == .system || kind == .gap) && !detail.isEmpty { return detail }
        return kind.title
    }
}

public struct DesktopPreferences: Codable, Sendable, Equatable {
    public var screenshotEnabled = false
    public var automaticScreenshotEnabled = false
    public var windowCompatibilityBundleIDs: Set<String> = []
    public var allowedBundleIDs: Set<String> = []
    public var excludedBundleIDs: Set<String> = []
    public var minimumCaptureInterval: Double = 15
    public var retentionDays = 7
    public var maximumBytes: Int64 = 5 * 1024 * 1024 * 1024
    public var textEnabled = false
    public var textAllowedBundleIDs: Set<String> = []
    public var periodicEnabled = false
    public var periodicInterval: Double = 60
    public var samplingMode: DesktopSamplingMode = .balanced
    public var interactionTriggersEnabled = false
    public var samplingOptions = DesktopSamplingOptions()
    public var contentOptions = DesktopContentOptions()
    public var imageOptions = DesktopImageOptions()
    public init() {}
    private enum CodingKeys: String, CodingKey {
        case screenshotEnabled, automaticScreenshotEnabled, windowCompatibilityBundleIDs, allowedBundleIDs, excludedBundleIDs, minimumCaptureInterval, retentionDays, maximumBytes
        case textEnabled, textAllowedBundleIDs, periodicEnabled, periodicInterval, samplingMode, interactionTriggersEnabled, samplingOptions, contentOptions, imageOptions
    }
    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        screenshotEnabled = try c.decodeIfPresent(Bool.self, forKey: .screenshotEnabled) ?? false
        automaticScreenshotEnabled = try c.decodeIfPresent(Bool.self, forKey: .automaticScreenshotEnabled) ?? false
        windowCompatibilityBundleIDs = try c.decodeIfPresent(Set<String>.self, forKey: .windowCompatibilityBundleIDs) ?? []
        allowedBundleIDs = try c.decodeIfPresent(Set<String>.self, forKey: .allowedBundleIDs) ?? []
        excludedBundleIDs = try c.decodeIfPresent(Set<String>.self, forKey: .excludedBundleIDs) ?? []
        minimumCaptureInterval = try c.decodeIfPresent(Double.self, forKey: .minimumCaptureInterval) ?? 10
        retentionDays = try c.decodeIfPresent(Int.self, forKey: .retentionDays) ?? 7
        maximumBytes = try c.decodeIfPresent(Int64.self, forKey: .maximumBytes) ?? 5 * 1024 * 1024 * 1024
        textEnabled = try c.decodeIfPresent(Bool.self, forKey: .textEnabled) ?? false
        textAllowedBundleIDs = try c.decodeIfPresent(Set<String>.self, forKey: .textAllowedBundleIDs) ?? []
        periodicEnabled = try c.decodeIfPresent(Bool.self, forKey: .periodicEnabled) ?? false
        periodicInterval = try c.decodeIfPresent(Double.self, forKey: .periodicInterval) ?? 60
        samplingMode = try c.decodeIfPresent(DesktopSamplingMode.self, forKey: .samplingMode) ?? .balanced
        interactionTriggersEnabled = try c.decodeIfPresent(Bool.self, forKey: .interactionTriggersEnabled) ?? false
        contentOptions = try c.decodeIfPresent(DesktopContentOptions.self, forKey: .contentOptions) ?? DesktopContentOptions()
        imageOptions = try c.decodeIfPresent(DesktopImageOptions.self, forKey: .imageOptions) ?? DesktopImageOptions()
        if let options = try c.decodeIfPresent(DesktopSamplingOptions.self, forKey: .samplingOptions) {
            samplingOptions = options
        } else {
            // Preserve the effective pre-customization policy once, then honor
            // explicit settings across every subsequent launch.
            if samplingMode == .balanced {
                periodicInterval = max(60, periodicInterval)
                minimumCaptureInterval = max(15, minimumCaptureInterval)
            } else { samplingOptions.imageCapacity = 6 }
            samplingOptions.imageFallbackInterval = max(120, periodicInterval * 2)
        }
    }
}

/// Presentation-only grouping; raw immutable records remain the stored contract.
public struct DesktopTimelineEvent: Identifiable, Sendable, Hashable {
    public var record: DesktopRecord
    public var observations: [DesktopRecord]
    public var observationOffset: Int
    public var sampleCount: Int
    public var latestRecord: DesktopRecord
    public var startDate: Date
    public var endDate: Date
    public var screenshots: [DesktopRecord]
    public var issues: [DesktopRecord]
    public var id: String { record.id }
    public var hasEarlier: Bool { observationOffset > 0 }
    public var hasLater: Bool { observationOffset + observations.count < sampleCount }
    public var hasScreenshot: Bool
    public var hasIssue: Bool
    /// Loaded records only for a paged event. Ledger export refreshes the complete
    /// activity independently; a UI page can never truncate an export silently.
    public var allRecords: [DesktopRecord] { [record] + Self.ordered(Self.unique(observations + screenshots + issues).filter { $0.id != record.id }) }
    public init(record: DesktopRecord, screenshots: [DesktopRecord] = [], issues: [DesktopRecord] = [], observations: [DesktopRecord]? = nil, observationOffset: Int = 0, sampleCount: Int? = nil, latestRecord: DesktopRecord? = nil, startDate: Date? = nil, endDate: Date? = nil, hasScreenshot: Bool? = nil, hasIssue: Bool? = nil) {
        self.record = record; self.observations = Self.ordered(Self.unique(observations ?? [record]))
        self.screenshots = Self.ordered(Self.unique(screenshots)); self.issues = Self.ordered(Self.unique(issues))
        self.observationOffset = observationOffset; self.sampleCount = sampleCount ?? self.observations.count
        self.latestRecord = latestRecord ?? self.observations.last ?? record
        self.startDate = startDate ?? self.observations.first?.timestamp ?? record.timestamp
        self.endDate = endDate ?? self.observations.last?.timestamp ?? record.timestamp
        self.hasScreenshot = hasScreenshot ?? (record.imageHash != nil || !screenshots.isEmpty)
        self.hasIssue = hasIssue ?? !issues.isEmpty
    }
    private static func unique(_ records: [DesktopRecord]) -> [DesktopRecord] {
        var seen = Set<String>(); return records.filter { seen.insert($0.id).inserted }
    }
    private static func ordered(_ records: [DesktopRecord]) -> [DesktopRecord] {
        records.sorted { $0.timestamp == $1.timestamp ? $0.id < $1.id : $0.timestamp < $1.timestamp }
    }
}

public struct DesktopStatistics: Sendable, Equatable {
    public var recordCount: Int
    public var screenshotCount: Int
    public var storageBytes: Int64
    public init(recordCount: Int = 0, screenshotCount: Int = 0, storageBytes: Int64 = 0) {
        self.recordCount = recordCount; self.screenshotCount = screenshotCount; self.storageBytes = storageBytes
    }
}

public struct DesktopInfoRow: Identifiable, Sendable, Hashable {
    public var id: String { label }
    public var label: String
    public var value: String
    public var symbol: String
    public init(_ label: String, _ value: String, symbol: String = "info.circle") { self.label = label; self.value = value; self.symbol = symbol }
}

public struct DesktopApplication: Identifiable, Sendable, Hashable {
    public var id: String { bundleID }
    public var bundleID: String
    public var name: String
    public var pid: Int32
    public var bundlePath: String?
    public init(bundleID: String, name: String, pid: Int32, bundlePath: String? = nil) { self.bundleID = bundleID; self.name = name; self.pid = pid; self.bundlePath = bundlePath }
}
