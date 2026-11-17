import Foundation
import CandidLocalization

public enum DesktopRecognitionKind: String, CaseIterable, Codable, Sendable {
    case ocr, vision

    public var title: String { self == .ocr ? L("OCR 文字识别") : L("VLM 图像分析") }
}

public enum DesktopRecognitionMode: String, CaseIterable, Codable, Sendable {
    case manual, immediate, batch

    public var title: String {
        switch self {
        case .manual: L("仅手动")
        case .immediate: L("新截图自动处理")
        case .batch: L("定时批处理")
        }
    }
}

public struct DesktopRecognitionPolicy: Codable, Equatable, Sendable {
    public var mode: DesktopRecognitionMode = .manual
    public var requireExternalPower = false
    public var minimumBatteryPercent = 0
    public var pluggedMinutes = 0
    public var pauseInLowPower = true
    public var batchIntervalMinutes = 30
    public var batchLimit = 10
    public var includeHistory = false
    public var automaticSince = Date()

    public init() {}

    public var earliestAutomaticDate: Date { includeHistory ? .distantPast : automaticSince }
    public var batchInterval: TimeInterval { TimeInterval(min(max(batchIntervalMinutes, 1), 1440) * 60) }
    public var batchSize: Int { min(max(batchLimit, 1), 100) }
}

public struct DesktopRecognitionPower: Sendable {
    public let onExternalPower: Bool?
    public let hasInternalBattery: Bool?
    public let batteryPercent: Int?
    public let lowPower: Bool
    public let thermalBlocked: Bool

    public init(onExternalPower: Bool?, hasInternalBattery: Bool?, batteryPercent: Int?, lowPower: Bool, thermalBlocked: Bool) {
        self.onExternalPower = onExternalPower
        self.hasInternalBattery = hasInternalBattery
        self.batteryPercent = batteryPercent
        self.lowPower = lowPower
        self.thermalBlocked = thermalBlocked
    }
}

public struct DesktopRecognitionPowerGate: Sendable {
    private var pluggedSince: TimeInterval?

    public init() {}

    public mutating func reset() { pluggedSince = nil }

    public func remainingPluggedWait(policy: DesktopRecognitionPolicy, uptime: TimeInterval) -> TimeInterval? {
        guard policy.requireExternalPower, let pluggedSince else { return nil }
        let remaining = Double(policy.pluggedMinutes) * 60 - (uptime - pluggedSince)
        return remaining > 0 ? remaining : nil
    }

    public mutating func blockReason(policy: DesktopRecognitionPolicy, power: DesktopRecognitionPower, uptime: TimeInterval) -> String? {
        if power.onExternalPower == true { pluggedSince = pluggedSince ?? uptime }
        else { reset() }
        if power.thermalBlocked { return L("等待 Mac 降温") }
        if policy.pauseInLowPower && power.lowPower { return L("低电量模式已开启") }
        if policy.requireExternalPower && power.onExternalPower != true { return L("等待接入外部电源") }
        if policy.minimumBatteryPercent > 0 && power.hasInternalBattery != false {
            guard let percent = power.batteryPercent else { return L("无法读取当前电量") }
            if percent < policy.minimumBatteryPercent { return L("等待电量达到 %@%%", policy.minimumBatteryPercent) }
        }
        if let remaining = remainingPluggedWait(policy: policy, uptime: uptime) {
            return L("还需持续插电约 %@ 分钟", Int(ceil(remaining / 60)))
        }
        return nil
    }
}

public struct DesktopOCRLine: Codable, Equatable, Sendable {
    public let text: String
    public let x: Int
    public let y: Int
    public let width: Int
    public let height: Int

    public init(text: String, x: Int, y: Int, width: Int, height: Int) {
        self.text = text
        self.x = x
        self.y = y
        self.width = width
        self.height = height
    }
}

public struct DesktopWindowLayout: Codable, Equatable, Sendable {
    public struct Region: Codable, Equatable, Sendable {
        public let role: String
        public let name: String
        public let visibleState: String
        public let text: [String]
        public let ocrLineIDs: [Int]
        public let visualSupplement: String
        public let tableRows: [[String]]?

        public init(role: String, name: String, visibleState: String, text: [String], ocrLineIDs: [Int],
                    visualSupplement: String, tableRows: [[String]]? = nil) {
            self.role = role
            self.name = name
            self.visibleState = visibleState
            self.text = text
            self.ocrLineIDs = ocrLineIDs
            self.visualSupplement = visualSupplement
            self.tableRows = tableRows
        }
    }

    public let regions: [Region]
    public let unclassifiedText: [String]
    public let excludedText: [String]
    public let focusLabel: String?

    public init(regions: [Region], unclassifiedText: [String], excludedText: [String], focusLabel: String? = nil) {
        self.regions = regions
        self.unclassifiedText = unclassifiedText
        self.excludedText = excludedText
        self.focusLabel = focusLabel
    }

    public var markdown: String {
        var sections: [String] = []
        if let focusLabel { sections.append(L("**窗口焦点** ") + Self.escape(focusLabel)) }
        for region in regions {
            var blocks = ["## " + Self.escape(region.name)]
            if !region.visibleState.isEmpty { blocks.append(Self.escape(region.visibleState)) }
            if let rows = region.tableRows, let columns = rows.map(\.count).max(), columns > 0 {
                let header = (1...columns).map { L("列 %@", $0) }
                let separator = Array(repeating: "---", count: columns)
                let table = ([header, separator] + rows).map { row in
                    let cells = row.map(Self.escape) + Array(repeating: "", count: columns - row.count)
                    return "| " + cells.joined(separator: " | ") + " |"
                }
                blocks.append(table.joined(separator: "\n"))
            } else if !region.text.isEmpty {
                blocks.append(region.text.map { "- " + Self.escape($0) }.joined(separator: "\n"))
            }
            if !region.visualSupplement.isEmpty {
                blocks.append(L("**画面补充** ") + Self.escape(region.visualSupplement))
            }
            sections.append(blocks.joined(separator: "\n\n"))
        }
        if !unclassifiedText.isEmpty {
            sections.append(L("## 未归类文字\n\n") + unclassifiedText.map { "- " + Self.escape($0) }.joined(separator: "\n"))
        }
        return sections.joined(separator: "\n\n")
    }

    private static func escape(_ text: String) -> String {
        let punctuation: Set<Character> = ["\\", "`", "*", "_", "[", "]", "<", ">", "|", "~", "&"]
        return text.replacingOccurrences(of: "\n", with: " ").reduce(into: "") { escaped, character in
            if punctuation.contains(character) { escaped.append("\\") }
            escaped.append(character)
        }
    }

}

public struct DesktopWindowMarkdown: Codable, Equatable, Sendable {
    public let body: String
    public let matchedOCRLineCount: Int
    public let inputOCRLineCount: Int
    public let unplacedOCR: [String]
    public let unplacedMainOCR: [String]?
    public let mainOCRLineCount: Int?

    public init(body: String, matchedOCRLineCount: Int, inputOCRLineCount: Int, unplacedOCR: [String],
                unplacedMainOCR: [String]? = nil, mainOCRLineCount: Int? = nil) {
        self.body = body
        self.matchedOCRLineCount = matchedOCRLineCount
        self.inputOCRLineCount = inputOCRLineCount
        self.unplacedOCR = unplacedOCR
        self.unplacedMainOCR = unplacedMainOCR
        self.mainOCRLineCount = mainOCRLineCount
    }
}

public struct DesktopRecognitionResult: Codable, Equatable, Sendable {
    public let source: String
    public let text: String
    public let completedAt: Date
    public let usedOCRReference: Bool
    public let ocrLines: [DesktopOCRLine]?
    public let windowLayout: DesktopWindowLayout?
    public let windowMarkdown: DesktopWindowMarkdown?

    public init(source: String, text: String, completedAt: Date = Date(), usedOCRReference: Bool = false,
                ocrLines: [DesktopOCRLine]? = nil, windowLayout: DesktopWindowLayout? = nil,
                windowMarkdown: DesktopWindowMarkdown? = nil) {
        self.source = source
        self.text = text
        self.completedAt = completedAt
        self.usedOCRReference = usedOCRReference
        self.ocrLines = ocrLines
        self.windowLayout = windowLayout
        self.windowMarkdown = windowMarkdown
    }
}
