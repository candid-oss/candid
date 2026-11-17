import Foundation

public struct CostEstimate: Sendable, Equatable {
    public var amount: Double?
    public var currency: String?
    public var source: CostSource
}

public enum CostCalculator {
    /// Provider-reported cost wins. Otherwise price × tokens from the model's `cost_json`; unknown without a price.
    /// Cache tokens fall back to the input price when the table lists no cache price (never cheaper than reality).
    public static func cost(usage: TokenUsage, model: AIModel?) -> CostEstimate {
        if let reported = usage.reportedCost {
            return CostEstimate(amount: reported, currency: usage.reportedCurrency ?? "USD", source: .provider)
        }
        guard let cost = model?.cost, let inPrice = cost.perMtok.input, let outPrice = cost.perMtok.output, !usage.isEmpty else {
            return CostEstimate(amount: nil, currency: nil, source: .unknown)
        }
        let p = cost.perMtok
        let total = Double(usage.input ?? 0) * inPrice
            + Double(usage.output ?? 0) * outPrice
            + Double(usage.cacheRead ?? 0) * (p.cacheRead ?? inPrice)
            + Double(usage.cacheWrite ?? 0) * (p.cacheWrite ?? inPrice)
        return CostEstimate(amount: total / 1_000_000, currency: cost.currency, source: .estimated)
    }
}
