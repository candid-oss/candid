import Foundation

/// Decides the next recognition step for one frame. Inventory comes from an
/// independent region detector or an audited page adapter, never the AX tree alone.
public enum DesktopRecognitionRouting {
    public enum Kind: String, Codable, Sendable {
        case text, selected, expanded, enabled, visual
    }

    public struct Field: Codable, Sendable {
        public let id: String
        public let kind: Kind
        public let bounds: [Int]
        public let axRole: String?

        public init(id: String, kind: Kind, bounds: [Int], axRole: String? = nil) {
            self.id = id; self.kind = kind; self.bounds = bounds; self.axRole = axRole
        }
    }

    public struct Input: Codable, Sendable {
        /// A complete inventory of required fields. Each text field is one item or line.
        public let inventory: [Field]?
        public let ax: DesktopAXEvidence?
        /// Nil means OCR has not run; [] means it ran without producing text lines.
        public let ocr: [DesktopOCRLine]?

        public init(inventory: [Field]?, ax: DesktopAXEvidence? = nil, ocr: [DesktopOCRLine]? = nil) {
            self.inventory = inventory; self.ax = ax; self.ocr = ocr
        }
    }

    public enum Step: String, Codable, Sendable {
        case axOnly, ocr, vlm, done
    }

    public enum Evidence: String, Codable, Sendable {
        case ax, ocr
    }

    public enum Gap: String, Codable, Sendable {
        case unverifiedInventory, missingText, missingState, needsVisualInterpretation, conflictingText, ambiguousText
    }

    public struct FieldResult: Codable, Sendable {
        public let id: String
        public let source: Evidence?
        public let value: String?
        public let gap: Gap?
        public let needs: Step?
    }

    public struct Decision: Codable, Sendable {
        public let next: Step
        public let gap: Gap?
        public let fields: [FieldResult]
    }

    public static func decide(_ input: Input) -> Decision {
        guard let inventory = input.inventory, !inventory.isEmpty,
              inventory.allSatisfy({ !$0.id.isEmpty && validBox($0.bounds) }),
              Set(inventory.map(\.id)).count == inventory.count else {
            return Decision(next: input.ocr == nil ? .ocr : .vlm, gap: .unverifiedInventory, fields: [])
        }

        let nodes = input.ax?.isValid == true ? input.ax?.nodes ?? [] : []
        let lines = input.ocr ?? []
        let axMatches = inventory.map { field in
            // Truncation may cut text within a node. An explicit state on an
            // identified node is still usable when the rest of the tree is partial.
            if field.kind == .text && input.ax?.partial == true { return [Int]() }
            return nodes.indices.filter { index in
                let node = nodes[index]
                return node.role == field.axRole && node.bounds.map { contains(field.bounds, $0) } == true &&
                    axValue(node, kind: field.kind) != nil
            }
        }
        let ocrMatches = inventory.map { field in
            lines.indices.filter { index in
                field.kind == .text && ocrBox(lines[index]).map { contains(field.bounds, $0) } == true &&
                    nonempty(lines[index].text) != nil
            }
        }

        let fields = inventory.enumerated().map { index, field -> FieldResult in
            func missing(_ gap: Gap, _ next: Step) -> FieldResult {
                FieldResult(id: field.id, source: nil, value: nil, gap: gap, needs: next)
            }
            let axIndex = uniqueMatch(index, axMatches) { inventory[$0].kind == field.kind }
            let axText = axIndex.flatMap { axValue(nodes[$0], kind: field.kind) }
            let ocrIndex = uniqueMatch(index, ocrMatches) { _ in true }
            let ocrText = ocrIndex.flatMap { nonempty(lines[$0].text) }
            if !ocrMatches[index].isEmpty && ocrIndex == nil {
                return missing(.ambiguousText, .vlm)
            }
            if let axText, let ocrText, axText != ocrText {
                return missing(.conflictingText, .vlm)
            }
            if let axText {
                return FieldResult(id: field.id, source: .ax, value: axText, gap: nil, needs: nil)
            }
            if let ocrText {
                return FieldResult(id: field.id, source: .ocr, value: ocrText, gap: nil, needs: nil)
            }
            switch field.kind {
            case .text: return missing(.missingText, input.ocr == nil ? .ocr : .vlm)
            case .visual: return missing(.needsVisualInterpretation, .vlm)
            case .selected, .expanded, .enabled: return missing(.missingState, .vlm)
            }
        }

        let next: Step
        if fields.contains(where: { $0.needs == .ocr }) { next = .ocr }
        else if fields.contains(where: { $0.needs == .vlm }) { next = .vlm }
        else { next = fields.allSatisfy { $0.source == .ax } ? .axOnly : .done }
        return Decision(next: next, gap: nil, fields: fields)
    }

    private static func uniqueMatch(_ field: Int, _ matches: [[Int]], sameKind: (Int) -> Bool) -> Int? {
        guard matches[field].count == 1, let index = matches[field].first,
              !matches.indices.contains(where: { $0 != field && sameKind($0) && matches[$0].contains(index) }) else { return nil }
        return index
    }

    private static func axValue(_ node: DesktopAXEvidence.Node, kind: Kind) -> String? {
        switch kind {
        case .text: nonempty(node.text) ?? nonempty(node.label)
        case .selected: node.selected.map { String($0) }
        case .expanded: node.expanded.map { String($0) }
        case .enabled: node.enabled.map { String($0) }
        case .visual: nil
        }
    }

    private static func nonempty(_ text: String?) -> String? {
        guard let text, text.contains(where: { !$0.isWhitespace }) else { return nil }
        return text
    }

    private static func validBox(_ box: [Int]) -> Bool {
        box.count == 4 && box.allSatisfy { (0...1000).contains($0) } && box[0] < box[2] && box[1] < box[3]
    }

    private static func contains(_ field: [Int], _ evidence: [Int]) -> Bool {
        validBox(evidence) && field[0] <= evidence[0] && field[1] <= evidence[1] &&
            field[2] >= evidence[2] && field[3] >= evidence[3]
    }

    private static func ocrBox(_ line: DesktopOCRLine) -> [Int]? {
        guard (0...1000).contains(line.x), (0...1000).contains(line.y),
              line.width > 0, line.width <= 1000 - line.x,
              line.height > 0, line.height <= 1000 - line.y else { return nil }
        return [line.x, line.y, line.x + line.width, line.y + line.height]
    }
}
