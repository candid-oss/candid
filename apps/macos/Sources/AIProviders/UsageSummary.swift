import Foundation

public struct UsageTotals: Sendable, Equatable {
    public var calls = 0
    public var failed = 0
    public var inputTokens = 0
    public var outputTokens = 0
    public var cacheTokens = 0
    /// Cost by currency, split by how it was obtained. Unknown-cost calls are counted separately, never as zero.
    public var reportedCost: [String: Double] = [:]
    public var estimatedCost: [String: Double] = [:]
    public var unknownCostCalls = 0

    public var totalTokens: Int { inputTokens + outputTokens + cacheTokens }

    mutating func add(_ c: AICallRecord) {
        calls += 1
        if c.status != .ok { failed += 1 }
        inputTokens += c.inputTokens ?? 0; outputTokens += c.outputTokens ?? 0
        cacheTokens += (c.cacheReadTokens ?? 0) + (c.cacheWriteTokens ?? 0)
        switch (c.costSource, c.cost) {
        case (.provider?, let v?): reportedCost[c.currency ?? "USD", default: 0] += v
        case (.estimated?, let v?): estimatedCost[c.currency ?? "USD", default: 0] += v
        default: if c.status == .ok { unknownCostCalls += 1 }
        }
    }
}

public struct UsageGroup: Sendable, Equatable, Identifiable {
    public var key: String
    public var totals: UsageTotals
    public var id: String { key }
}

public enum UsageGrouping: Sendable { case day, feature, model }

public enum UsageSummary {
    public static func totals(_ calls: [AICallRecord]) -> UsageTotals {
        var t = UsageTotals(); calls.forEach { t.add($0) }; return t
    }

    /// Groups sorted by day ascending for `.day`, otherwise by token volume descending.
    public static func grouped(_ calls: [AICallRecord], by grouping: UsageGrouping, calendar: Calendar = .current) -> [UsageGroup] {
        var buckets: [String: UsageTotals] = [:]
        let f = DateFormatter(); f.calendar = calendar; f.timeZone = calendar.timeZone
        f.locale = Locale(identifier: "en_US_POSIX"); f.dateFormat = "yyyy-MM-dd"
        for c in calls {
            let key: String
            switch grouping {
            case .day: key = f.string(from: Date(timeIntervalSince1970: Double(c.startedMs) / 1000))
            case .feature: key = c.feature
            case .model: key = c.providerID + "/" + c.modelID
            }
            buckets[key, default: UsageTotals()].add(c)
        }
        let groups = buckets.map { UsageGroup(key: $0.key, totals: $0.value) }
        return grouping == .day ? groups.sorted { $0.key < $1.key } : groups.sorted { $0.totals.totalTokens > $1.totals.totalTokens }
    }
}
