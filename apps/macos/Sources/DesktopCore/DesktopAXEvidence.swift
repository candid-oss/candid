import Foundation

/// Optional, private evidence attached to the screenshot whose window supplied it.
public struct DesktopAXEvidence: Codable, Sendable, Hashable {
    public static let maximumNodes = 384
    public static let maximumEncodedBytes = 49_152

    public struct Node: Codable, Sendable, Hashable {
        public let parent: Int?
        public let role: String
        public let subrole: String?
        public let label: String?
        public let text: String?
        public let detail: String?
        public let placeholder: String?
        public let identifier: String?
        /// Left, top, right, bottom, normalized to 0…1000 within the screenshot.
        public let bounds: [Int]?
        public let reportedVisible: Bool?
        public let selected: Bool?
        public var focused: Bool?
        public let expanded: Bool?
        public let enabled: Bool?
        public let destination: String?

        public init(parent: Int? = nil, role: String, subrole: String? = nil, label: String? = nil,
                    text: String? = nil, bounds: [Int]? = nil, selected: Bool? = nil, focused: Bool? = nil,
                    expanded: Bool? = nil, enabled: Bool? = nil, destination: String? = nil,
                    detail: String? = nil, placeholder: String? = nil, identifier: String? = nil,
                    reportedVisible: Bool? = nil) {
            self.parent = parent; self.role = role; self.subrole = subrole; self.label = label
            self.text = text; self.bounds = bounds; self.selected = selected; self.focused = focused
            self.expanded = expanded; self.enabled = enabled
            self.destination = destination
            self.detail = detail; self.placeholder = placeholder; self.identifier = identifier
            self.reportedVisible = reportedVisible
        }
    }

    public let nodes: [Node]
    public let partial: Bool
    public let observedAt: Date?

    public init(nodes: [Node], partial: Bool, observedAt: Date? = nil) {
        self.nodes = nodes; self.partial = partial; self.observedAt = observedAt
    }

    var isValid: Bool {
        guard nodes.count <= Self.maximumNodes, observedAt?.timeIntervalSince1970.isFinite != false,
              let data = try? JSONEncoder().encode(self), data.count <= Self.maximumEncodedBytes else { return false }
        return nodes.enumerated().allSatisfy { index, node in
            guard node.parent.map({ $0 >= 0 && $0 < index }) ?? true,
                  !node.role.isEmpty, node.role.utf8.count <= 96, (node.subrole?.utf8.count ?? 0) <= 96,
                  (node.label?.utf8.count ?? 0) <= 480, (node.text?.utf8.count ?? 0) <= 1024,
                  (node.detail?.utf8.count ?? 0) <= 256, (node.placeholder?.utf8.count ?? 0) <= 160,
                  (node.identifier?.utf8.count ?? 0) <= 128,
                  (node.destination?.utf8.count ?? 0) <= 512 else { return false }
            guard let bounds = node.bounds else { return true }
            return bounds.count == 4 && bounds.allSatisfy { (0...1000).contains($0) } &&
                bounds[0] < bounds[2] && bounds[1] < bounds[3]
        }
    }
}
