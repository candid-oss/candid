import AIProviders
import CandidLocalization
import SwiftUI

struct ModelSourceSummary: View {
    let model: ModelSettingsModel
    private var configured: [Destination] {
        ModelRoleInfo.catalog.compactMap { info in
            let assignment = model.config.assignment(info.id)
            if let ref = assignment?.model { return model.config.destination(of: ref) }
            return (info.engines.first { $0.id == assignment?.engine } ?? info.defaultEngine)?.destination
        }
    }
    private var costs: [String: Double] {
        let today = Calendar.current.startOfDay(for: Date()).timeIntervalSince1970 * 1000
        let totals = UsageSummary.totals(model.calls.filter { Double($0.startedMs) >= today })
        return totals.reportedCost.merging(totals.estimatedCost, uniquingKeysWith: +)
    }
    var body: some View {
        LazyVGrid(columns: Array(repeating: GridItem(.flexible(), alignment: .topLeading), count: 4), spacing: Space.m) {
            metric(L("来源"), value: String(model.config.providers.count)) {
                let online = model.probes.values.filter { if case .ok = $0 { true } else { false } }.count
                Text(model.probes.isEmpty ? L("%@ 已启用", model.config.providers.filter(\.enabled).count) : L("%@ 在线", online)).typography(.caption).foregroundStyle(.secondary)
            }
            metric(L("已配好的角色"), value: "\(configured.count) / \(ModelRoleInfo.catalog.count)") {
                ProportionMeter(segments: [(Double(configured.count) / Double(max(1, ModelRoleInfo.catalog.count)), Palette.accent)])
            }
            metric(L("内容去处"), value: nil) {
                let counts = Destination.allCases.map { destination in configured.filter { $0 == destination }.count }
                ProportionMeter(segments: Array(zip(counts, Destination.allCases)).map { (Double($0.0) / Double(max(1, configured.count)), $0.1.tint) })
                Text(zip(counts, Destination.allCases).filter { $0.0 > 0 }.map { "\($0.1.label) \($0.0)" }.joined(separator: " · "))
                    .typography(.caption).foregroundStyle(.secondary)
            }
            metric(L("今天花费"), value: costs.isEmpty ? "—" : costs.sorted { $0.key < $1.key }.map { $0.key + " " + String(format: "%.2f", $0.value) }.joined(separator: " / ")) {
                let todayCalls = model.calls.filter { Calendar.current.isDateInToday(Date(timeIntervalSince1970: Double($0.startedMs) / 1000)) }
                Text(L("%@ 次调用", todayCalls.count) + (todayCalls.contains { $0.costSource == .estimated } ? L(" · 含估算") : costs.isEmpty && !todayCalls.isEmpty ? L(" · 费用未知") : "")).typography(.caption).foregroundStyle(.secondary)
            }
        }.padding(.vertical, Space.s)
    }
    private func metric<Content: View>(_ title: String, value: String?, @ViewBuilder detail: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: Space.s) {
            Text(title).typography(.caption).foregroundStyle(.secondary)
            if let value { Text(value).typography(.section, digits: true).lineLimit(1).minimumScaleFactor(0.8) }
            detail()
        }.frame(maxWidth: .infinity, alignment: .leading)
    }
}
