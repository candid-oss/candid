import Foundation

// Wire types for Rust-backed agent connectors (timeline-v1 §6.2). Swift does not open transcript files.

struct FormatFailure: Error { let code: String }

/// Finite error codes; never contain source content.
public enum AgentIssueCode {
    public static let invalidEnvelope = "invalid_envelope"
    public static let unsupportedShape = "unsupported_shape"
    public static let malformedJSON = "malformed_json"
    public static let unknownFormat = "unknown_format"
    public static let unreadable = "unreadable"
}

public struct UsageTotals: Codable, Equatable, Sendable, Hashable {
    public var input: Int64 = 0, output: Int64 = 0, cacheRead: Int64 = 0, cacheWrite: Int64 = 0, reasoning: Int64 = 0
    public init(input: Int64 = 0, output: Int64 = 0, cacheRead: Int64 = 0, cacheWrite: Int64 = 0, reasoning: Int64 = 0) {
        self.input = input; self.output = output; self.cacheRead = cacheRead; self.cacheWrite = cacheWrite; self.reasoning = reasoning
    }
    mutating func add(_ o: UsageTotals) {
        input += o.input; output += o.output; cacheRead += o.cacheRead; cacheWrite += o.cacheWrite; reasoning += o.reasoning
    }
}
