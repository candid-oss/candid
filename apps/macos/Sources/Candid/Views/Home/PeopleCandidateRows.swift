import CandidLocalization
import SwiftUI

/// 待你处理 → 「可能是同一人」 cards, and a line per merge the model made for you with 撤销.
struct PeopleCandidateRows: View {
    @Bindable var model: PeopleCandidatesModel

    var body: some View {
        ForEach(model.candidates) { item in
            VStack(alignment: .leading, spacing: Space.xxs) {
                HStack(alignment: .firstTextBaseline, spacing: Space.xs) {
                    Text("?").typography(.body, .bold).foregroundStyle(.tertiary).frame(width: 16).accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(L("%@ 和 %@ 可能是同一个人", PeopleCandidatesModel.label(item.a), PeopleCandidatesModel.label(item.b))).typography(.body)
                        if !item.reason.isEmpty { Text(item.reason).typography(.caption).foregroundStyle(.tertiary) }
                    }
                }
                HStack(spacing: Space.xxs) {
                    Spacer(minLength: 0)
                    SecondaryButton(title: L("不是同一人")) { Task { await model.answer(item, merge: false) } }
                    SecondaryButton(title: L("合并")) { Task { await model.answer(item, merge: true) } }
                }
                .disabled(model.working.contains(item.id))
                if let problem = model.problems[item.id] { InlineMessage(kind: .error, text: problem) }
            }
        }
        ForEach(model.autoMerged) { item in
            HStack(alignment: .firstTextBaseline, spacing: Space.xs) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(L("已自动合并：%@ = %@", PeopleCandidatesModel.label(item.a), PeopleCandidatesModel.label(item.b))).typography(.secondary).foregroundStyle(.secondary)
                    if !item.reason.isEmpty { Text(item.reason).typography(.caption).foregroundStyle(.tertiary) }
                }
                Spacer(minLength: Space.xs)
                ChipButton(title: L("撤销")) { Task { await model.answer(item, merge: false) } }
                    .disabled(model.working.contains(item.id))
            }
        }
    }
}
