import DesktopCore
import CandidLocalization
import Foundation
import CoreGraphics
import ImageIO

struct DesktopMarkdownEvidence {
    private let lines: [String]
    private let ordered: [Int]
    private let mainIndexes: Set<Int>?
    private let sentIndexes: Set<Int>
    let promptLines: String
    let paneGuide: String
    let mainPaneJPEG: Data?
    private let cropLeft: Int?

    init(ocr: DesktopRecognitionResult?, jpeg: Data? = nil, focusMainPane: Bool = false) {
        let boxes = ocr?.ocrLines
        lines = boxes?.map(\.text) ?? ocr?.text.split(separator: "\n").map(String.init) ?? []
        let dividers = jpeg.map(Self.paneDividers) ?? []
        let edges = [0] + dividers + [1000]
        let widest = (0..<(edges.count - 1)).max { edges[$0 + 1] - edges[$0] < edges[$1 + 1] - edges[$1] } ?? 0
        func pane(_ line: DesktopOCRLine) -> Int { dividers.filter { $0 <= line.x }.count }
        let crop = focusMainPane && dividers.count == 1 && widest == 1
            ? jpeg.flatMap { Self.cropRightPane($0, left: edges[widest]) } : nil
        mainPaneJPEG = crop
        cropLeft = crop == nil ? nil : edges[widest]
        if crop != nil {
            paneGuide = L("输入图片已沿分栏去掉左侧列表，只含完整的聊天主视图。OCR 坐标仍对应原始窗口。")
        } else {
            paneGuide = dividers.isEmpty ? "" :
                L("从截图检测到纵向分栏。最宽主视图约为 x=%@–%@。以下 OCR 已按栏分组，请与原图核对，侧栏预览不要写进主视图。", edges[widest], edges[widest + 1])
        }
        let main = dividers.isEmpty ? nil : boxes.map { boxes in
            Set(boxes.indices.filter { pane(boxes[$0]) == widest })
        }
        mainIndexes = main
        ordered = lines.indices.sorted {
            guard let boxes else { return $0 < $1 }
            let a = boxes[$0], b = boxes[$1]
            let paneA = pane(a), paneB = pane(b)
            return (paneA == widest ? -1 : paneA, a.y / 16, a.x, a.y, $0)
                < (paneB == widest ? -1 : paneB, b.y / 16, b.x, b.y, $1)
        }
        var encoded: [String] = []
        var remaining = 15_000
        var currentPane: Int?
        let promptIndexes = crop == nil ? ordered : ordered.filter { main?.contains($0) == true }
        var sent = Set<Int>()
        for index in promptIndexes.prefix(256) {
            let box = boxes.map { boxes in
                let line = boxes[index]
                return " [\(line.x),\(line.y),\(line.x + line.width),\(line.y + line.height)]"
            } ?? ""
            let entry = "\(index + 1)\(box) \(lines[index])"
            guard entry.utf8.count <= remaining else { break }
            remaining -= entry.utf8.count
            if !dividers.isEmpty, let boxes {
                let position = pane(boxes[index])
                if currentPane != position {
                    let label = position == widest ? L("主视图") : position < widest ? L("左侧区") : L("右侧区")
                    encoded.append(L("【%@ x=%@–%@】", label, edges[position], edges[position + 1]))
                    currentPane = position
                }
            }
            encoded.append(entry)
            sent.insert(index)
        }
        sentIndexes = sent
        promptLines = encoded.joined(separator: "\n")
    }

    func axPromptLines(_ outline: DesktopAXEvidence) -> String {
        let candidates = outline.nodes.indices.filter { index in
            let node = outline.nodes[index]
            if let cropLeft {
                guard let bounds = node.bounds, bounds.count == 4, (bounds[0] + bounds[2]) / 2 >= cropLeft else { return false }
            } else if node.bounds == nil && node.reportedVisible != true { return false }
            return node.label != nil || node.text != nil || node.placeholder != nil || node.selected == true || node.focused == true
        }
        func priority(_ i: Int) -> Int {
            let node = outline.nodes[i]
            if node.selected == true || node.focused == true { return 0 }
            if ["AXHeading", "AXRow", "AXCell", "AXTextArea", "AXStaticText"].contains(node.role) { return 1 }
            return 2
        }
        var lines: [Int: String] = [:]
        var remaining = 6_000
        for index in candidates.sorted(by: { (priority($0), $0) < (priority($1), $1) }) {
            let node = outline.nodes[index]
            let states = [node.selected == true ? L("选中") : nil, node.focused == true ? L("焦点") : nil,
                          node.expanded.map { $0 ? L("展开") : L("折叠") }, node.enabled == false ? L("不可用") : nil]
                .compactMap { $0 }.joined(separator: " ")
            let parent = node.parent.map { L(" 父节点AX%@", $0 + 1) } ?? ""
            let content = [node.label, node.text, node.detail, node.placeholder.map { L("占位提示：") + $0 }].compactMap { $0 }
                .joined(separator: " | ").split(whereSeparator: \.isNewline).joined(separator: " ")
            let snippet = DesktopTextPolicy.prefix(content, maxBytes: 500)
            let line = "AX\(index + 1)\(parent) \(node.role) \(node.bounds ?? []) \(states) \(snippet)"
            guard lines.count < 72, line.utf8.count + 1 <= remaining else { continue }
            lines[index] = line
            remaining -= line.utf8.count + 1
        }
        let meaningfulCount = lines.count
        for index in lines.keys.sorted() {
            var parent = outline.nodes[index].parent
            while let current = parent, outline.nodes.indices.contains(current), current < index {
                let node = outline.nodes[current]
                parent = node.parent
                if lines[current] != nil { break }
                let ancestor = node.parent.map { L(" 父节点AX%@", $0 + 1) } ?? ""
                let line = "AX\(current + 1)\(ancestor) \(node.role)"
                guard lines.count < 80, line.utf8.count + 1 <= remaining else { break }
                lines[current] = line
                remaining -= line.utf8.count + 1
                if parent.map({ $0 >= current }) == true { break }
            }
        }
        let trimmed = outline.partial || meaningfulCount < candidates.count
        let body = lines.keys.sorted().compactMap { lines[$0] }.joined(separator: "\n")
        return body.isEmpty ? "" : (trimmed ? L("（部分）\n") : "") + body
    }

    func resolve(_ output: String) -> DesktopWindowMarkdown {
        var markdown = output.trimmingCharacters(in: .whitespacesAndNewlines)
        if markdown.lowercased().hasPrefix("```markdown\n"), markdown.hasSuffix("\n```") {
            markdown = String(markdown.dropFirst(12).dropLast(4)).trimmingCharacters(in: .whitespacesAndNewlines)
        }
        let pattern = /\[\[([0-9]{1,5})\]\]/
        let matches = markdown.matches(of: pattern)
        var used = Set<Int>()
        for match in matches.reversed() {
            guard let id = Int(match.1), id > 0, sentIndexes.contains(id - 1) else {
                markdown.replaceSubrange(match.range, with: "［未找到对应 OCR 行］")
                continue
            }
            used.insert(id - 1)
            let before = markdown[..<match.range.lowerBound]
            let lineStart = before.lastIndex(of: "\n").map { before.index(after: $0) } ?? before.startIndex
            let sameLine = before[lineStart...]
            let key = Self.comparisonKey(lines[id - 1])
            if !key.isEmpty && Self.comparisonKey(String(sameLine)).hasSuffix(key) {
                let trimStart = sameLine.lastIndex(where: { !$0.isWhitespace }).map { markdown.index(after: $0) }
                    ?? match.range.lowerBound
                markdown.removeSubrange(trimStart..<match.range.upperBound)
                continue
            }
            let escaped = lines[id - 1].reduce(into: "") { result, character in
                if "\\`*_[]<>|~".contains(character) { result.append("\\") }
                result.append(character)
            }
            markdown.replaceSubrange(match.range, with: escaped)
        }
        let normalized = Self.comparisonKey(markdown)
        let exactLines = Set(markdown.split(separator: "\n").flatMap { line -> [String] in
            let whole = Self.comparisonKey(String(line))
            if let colon = line.firstIndex(where: { $0 == "：" || $0 == ":" }) {
                return [whole, Self.comparisonKey(String(line[line.index(after: colon)...]))]
            }
            return [whole]
        })
        let missing = ordered.filter { index in
            let key = Self.comparisonKey(lines[index])
            return !used.contains(index) && !key.isEmpty
                && !exactLines.contains(key) && !(key.count >= 4 && normalized.contains(key))
        }
        return DesktopWindowMarkdown(body: markdown, matchedOCRLineCount: lines.count - missing.count,
            inputOCRLineCount: lines.count, unplacedOCR: missing.map { lines[$0] },
            unplacedMainOCR: mainIndexes.map { indexes in missing.filter(indexes.contains).map { lines[$0] } },
            mainOCRLineCount: mainIndexes?.count)
    }

    private static func cropRightPane(_ jpeg: Data, left: Int) -> Data? {
        guard let source = CGImageSourceCreateWithData(jpeg as CFData, nil),
              let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else { return nil }
        let x = image.width * left / 1000
        guard x > 0, x < image.width,
              let crop = image.cropping(to: CGRect(x: x, y: 0, width: image.width - x, height: image.height)) else { return nil }
        let data = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(data, "public.jpeg" as CFString, 1, nil) else { return nil }
        CGImageDestinationAddImage(destination, crop, [kCGImageDestinationLossyCompressionQuality: 0.9] as CFDictionary)
        return CGImageDestinationFinalize(destination) ? data as Data : nil
    }

    private static func paneDividers(_ jpeg: Data) -> [Int] {
        guard let source = CGImageSourceCreateWithData(jpeg as CFData, nil),
              let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else { return [] }
        let width = 512, height = 384
        var pixels = [UInt8](repeating: 0, count: width * height)
        let rendered = pixels.withUnsafeMutableBytes { bytes -> Bool in
            guard let context = CGContext(data: bytes.baseAddress, width: width, height: height,
                bitsPerComponent: 8, bytesPerRow: width, space: CGColorSpaceCreateDeviceGray(),
                bitmapInfo: CGImageAlphaInfo.none.rawValue) else { return false }
            context.interpolationQuality = .medium
            context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
            return true
        }
        guard rendered else { return [] }
        let band = 26..<344
        var runs: [[(x: Int, strength: Int)]] = []
        for x in 51..<450 {
            let strength = band.reduce(0) { count, y in
                count + (abs(Int(pixels[y * width + x - 2]) - Int(pixels[y * width + x + 2])) > 2 ? 1 : 0)
            }
            guard strength * 100 >= band.count * 85 else { continue }
            if let last = runs.last?.last, x - last.x <= 5 { runs[runs.count - 1].append((x, strength)) }
            else { runs.append([(x, strength)]) }
        }
        guard runs.count <= 3 else { return [] }
        return runs.compactMap { run in
            guard run.count <= 14, let left = run.first, let right = run.last else { return nil }
            return (left.x + right.x) * 500 / width
        }
    }

    private static func comparisonKey(_ string: String) -> String {
        String(string.lowercased().unicodeScalars.filter { CharacterSet.alphanumerics.contains($0) })
    }
}
