import Foundation
import CandidLocalization

public enum DesktopRecognitionKind: String, CaseIterable, Codable, Sendable {
    case ocr, vision

    public var title: String { self == .ocr ? L("OCR 文字识别") : L("VLM 图像分析") }
}

/// When screenshots are recognised automatically. *When power may be spent* is not decided here: the core's
/// one power gate (后台处理 随时 / 省电 / 手动) answers that for every automatic run (power v1 §3.2).
public enum DesktopRecognitionMode: String, CaseIterable, Codable, Sendable {
    case manual, immediate

    public var title: String {
        switch self {
        case .manual: L("仅手动")
        case .immediate: L("新截图自动处理")
        }
    }

    /// The removed scheduled-batch mode becomes automatic processing; the power gate now paces it.
    public init(from decoder: any Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = raw == "manual" ? .manual : .immediate
    }
}

public struct DesktopRecognitionPolicy: Codable, Equatable, Sendable {
    public var mode: DesktopRecognitionMode = .manual
    public var includeHistory = false
    public var automaticSince = Date()

    public init() {}

    public var earliestAutomaticDate: Date { includeHistory ? .distantPast : automaticSince }
}

/// One reading of the Mac's power state, reported to the core (`POST /local/power` `report`).
public struct DesktopRecognitionPower: Sendable, Equatable {
    public let onExternalPower: Bool?
    public let hasInternalBattery: Bool?
    public let batteryPercent: Int?
    public let lowPower: Bool
    /// `nominal`, `fair`, `serious` or `critical`.
    public let thermal: String

    public init(onExternalPower: Bool?, hasInternalBattery: Bool?, batteryPercent: Int?, lowPower: Bool, thermal: String) {
        self.onExternalPower = onExternalPower
        self.hasInternalBattery = hasInternalBattery
        self.batteryPercent = batteryPercent
        self.lowPower = lowPower
        self.thermal = thermal
    }

    /// The core's report fields. A Mac without an internal battery is on external power by definition.
    public func report(idleSeconds: Int?) -> [String: Any] {
        let external: Any = hasInternalBattery == false ? true : (onExternalPower.map { $0 as Any } ?? NSNull())
        return ["action": "report", "external_power": external, "battery_percent": batteryPercent.map { $0 as Any } ?? NSNull(),
                "low_power_mode": lowPower, "thermal": thermal, "idle_seconds": idleSeconds.map { $0 as Any } ?? NSNull()]
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
