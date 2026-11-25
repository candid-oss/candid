import CandidLocalization
import SwiftUI

struct AIWorkspaceSheet<Content: View>: View {
    @Environment(\.dismiss) private var dismiss
    let title: String
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(title).typography(.section).accessibilityAddTraits(.isHeader)
                Spacer()
                QuietButton(title: L("完成")) { dismiss() }
                    .keyboardShortcut(.cancelAction)
            }
            .padding(.horizontal, Space.page)
            .padding(.vertical, Space.m)
            HairlineDivider()
            ScrollView {
                VStack(alignment: .leading, spacing: Space.m) { content }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, Space.page)
                    .padding(.vertical, Space.m)
            }
        }
        .frame(width: 630, height: 480)
    }
}
