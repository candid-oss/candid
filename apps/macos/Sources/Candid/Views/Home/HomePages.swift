import ConsoleCore
import CandidLocalization
import SwiftUI

/// Top navigation. AI carries its own gradient so it reads as the place
/// where the day gets understood, not as one more list.
struct HomeNav: View {
    @Binding var page: HomePage
    @Namespace private var space

    static let aiGradient = Palette.ai

    var body: some View {
        HStack(spacing: 2) {
            tab(.timeline) { Text(L("时间线")) }
            tab(.library) { Text(L("资料")) }
            tab(.ai) {
                HStack(spacing: 4) {
                    Image(systemName: "sparkles").typography(.caption, .bold)
                    Text("AI").typography(.body, .bold)
                }
                .foregroundStyle(Self.aiGradient)
            }
            tab(.connections) { Text(L("连接")) }
        }
    }

    private func tab<Label: View>(_ target: HomePage, @ViewBuilder label: () -> Label) -> some View {
        let on = page == target || (target == .connections && page == .device)
        return Button { withAnimation(Motion.bouncy) { page = target } } label: {
            label()
                .typography(.body, on ? .semibold : .medium)
                .foregroundStyle(on ? Color.primary : Color.secondary)
                .padding(.horizontal, 12).frame(height: 28)
                .background {
                    if on { Capsule().fill(Surface.hover.color).matchedGeometryEffect(id: "tab", in: space) }
                }
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}
