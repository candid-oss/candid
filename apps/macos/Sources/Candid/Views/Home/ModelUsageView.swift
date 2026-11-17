import AIProviders
import CandidLocalization
import SwiftUI

/// Calls, tokens and cost by day, feature or model. Cost is split by how it was obtained; an estimate is never
/// presented as a bill.
struct ModelUsageView: View {
    let model: ModelSettingsModel
    @ViewState private var days = 7
    @ViewState private var grouping = "day"

    private static let featureNames = [
        "agent_chat": L("对话 Agent"), "daily_review": L("每日回顾"), "episode": L("片段整理"), "recognition": L("屏幕识别"),
        "embedding": L("文本嵌入"), "summary": L("摘要"), "relevance": L("相关性初筛"), "vision": L("图像理解"), "ocr": L("文字识别")
    ]

    private var totals: UsageTotals { UsageSummary.totals(model.calls) }
    private var groups: [UsageGroup] {
        UsageSummary.grouped(model.calls, by: grouping == "day" ? .day : grouping == "feature" ? .feature : .model)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 22) {
            PillSwitcher(items: [(1, L("今天")), (7, L("7 天")), (30, L("30 天"))], selection: $days, scale: .secondary).offset(x: -13)
            HStack(alignment: .top, spacing: 40) {
                figure(L("调用"), "\(totals.calls)", totals.failed > 0 ? L("%@ 次失败", totals.failed) : nil)
                figure(L("令牌"), compact(totals.totalTokens), L("输入 %@ · 输出 %@", compact(totals.inputTokens), compact(totals.outputTokens)) + (totals.cacheTokens > 0 ? L(" · 缓存 %@", compact(totals.cacheTokens)) : ""))
                costFigure
            }
            if model.calls.isEmpty {
                Text(L("这段时间还没有模型调用。")).typography(.body).foregroundStyle(.secondary)
            } else {
                PillSwitcher(items: [("day", L("按日")), ("feature", L("按功能")), ("model", L("按模型"))], selection: $grouping, scale: .secondary).offset(x: -13)
                let peak = max(1, groups.map(\.totals.totalTokens).max() ?? 1)
                VStack(spacing: 0) {
                    ForEach(groups) { g in
                        row(g, peak: peak)
                        HairlineDivider()
                    }
                }
            }
            Text(L("「估算」按价格表乘以令牌数，不是账单；「未知」多为本机模型或没有填价格。外部工具（Codex、Claude Code）的用量不在这里。"))
                .typography(.secondary).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
        .task(id: days) { await model.loadUsage(days: days) }
        .animation(Motion.spring, value: model.calls.count)
    }

    private func figure(_ title: String, _ value: String, _ note: String?) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(title).typography(.secondary).foregroundStyle(.secondary)
            Text(value).typography(.display, digits: true).contentTransition(.numericText())
            if let note { Text(note).typography(.secondary).foregroundStyle(.secondary) }
        }
    }

    private var costFigure: some View {
        let reported = totals.reportedCost, estimated = totals.estimatedCost
        let all = reported.merging(estimated, uniquingKeysWith: +)
        return VStack(alignment: .leading, spacing: 3) {
            Text(L("费用")).typography(.secondary).foregroundStyle(.secondary)
            Text(all.isEmpty ? "—" : money(all)).typography(.display, digits: true).contentTransition(.numericText())
            VStack(alignment: .leading, spacing: 1) {
                if !reported.isEmpty { Text(L("服务商返回 %@", money(reported))) }
                if !estimated.isEmpty { Text(L("估算 %@", money(estimated))) }
                if totals.unknownCostCalls > 0 { Text(L("%@ 次费用未知", totals.unknownCostCalls)) }
            }.typography(.secondary).foregroundStyle(.secondary)
        }
    }

    private func row(_ g: UsageGroup, peak: Int) -> some View {
        let t = g.totals
        return HStack(spacing: 14) {
            Text(label(g.key)).typography(.body, .medium).lineLimit(1).frame(width: 190, alignment: .leading)
            GeometryReader { geo in
                Capsule().fill(Color.accentColor.opacity(0.55))
                    .frame(width: max(3, geo.size.width * CGFloat(t.totalTokens) / CGFloat(peak)), height: 5)
                    .frame(maxHeight: .infinity, alignment: .center)
            }.frame(height: 18)
            Text(L("%@ 次", t.calls)).typography(.secondary, digits: true).foregroundStyle(.secondary).frame(width: 52, alignment: .trailing)
            Text(compact(t.totalTokens)).typography(.secondary, digits: true).foregroundStyle(.secondary).frame(width: 58, alignment: .trailing)
            costCell(t).frame(width: 96, alignment: .trailing)
        }
        .padding(.vertical, 9)
    }

    @ViewBuilder private func costCell(_ t: UsageTotals) -> some View {
        let all = t.reportedCost.merging(t.estimatedCost, uniquingKeysWith: +)
        if all.isEmpty { Text("—").typography(.secondary).foregroundStyle(.secondary) }
        else {
            HStack(spacing: 4) {
                if t.reportedCost.isEmpty { Text(L("估")).typography(.caption, .semibold).foregroundStyle(.orange) }
                Text(money(all)).typography(.secondary, digits: true)
            }
        }
    }

    private func label(_ key: String) -> String {
        grouping == "feature" ? (Self.featureNames[key] ?? key) : key
    }

    private func compact(_ n: Int) -> String {
        n >= 1_000_000 ? String(format: "%.1fM", Double(n) / 1e6) : n >= 10_000 ? String(format: "%.1fk", Double(n) / 1e3) : "\(n)"
    }

    private func money(_ by: [String: Double]) -> String {
        by.sorted { $0.key < $1.key }.map { cur, v in
            let symbol = cur == "USD" ? "$" : cur == "CNY" ? "¥" : cur + " "
            return symbol + (v < 0.01 && v > 0 ? String(format: "%.4f", v) : String(format: "%.2f", v))
        }.joined(separator: " + ")
    }
}
