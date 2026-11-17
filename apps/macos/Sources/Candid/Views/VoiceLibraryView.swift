import SwiftUI
import CandidLocalization

/// People in the voice library (mockup ai-pages-v1 ①, lower half): name and sample count; rename, merge into
/// someone else, or delete. Deleting a person removes their samples and their recordings go back to 「说话人 N」.
struct VoiceLibraryView: View {
    @ViewState private var model = VoiceLibraryModel()
    @ViewState private var editing: String?
    @ViewState private var name = ""
    @ViewState private var pending: (action: String, person: VoiceLibraryModel.Person, target: VoiceLibraryModel.Person?)?

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if model.people.isEmpty, !model.busy {
                Text(L("还没有人。在录音的「说话人」里点「这是谁？」，确认过的声音会出现在这里。"))
                    .typography(.secondary).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            ForEach(Array(model.people.enumerated()), id: \.element.id) { index, person in row(person, tint: RecordingSpeakersView.tint(index)) }
            if model.busy { ProgressView().controlSize(.small) }
            if let message = model.cleanupMessage { Text(message).typography(.secondary).foregroundStyle(.secondary) }
            if let error = model.error { InlineMessage(kind: .error, text: error) }
            Text(L("样本只来自你在录音里的确认，只在本机。")).typography(.caption).foregroundStyle(.tertiary).padding(.top, 4)
        }
        .task { await model.refresh() }
        .overlay(alignment: .top) {
            if let pending {
                ConfirmCapsule(title: pending.action == "delete" ? L("删除「%@」？", pending.person.name) : L("把「%@」并入「%@」？", pending.person.name, pending.target?.name ?? ""),
                               message: pending.action == "delete" ? L("连同 %@ 份样本删除，以前录音里的他回到「说话人 N」。", pending.person.sample_count) : L("样本合到一起，录音里的名字跟着改。"),
                               confirmTitle: pending.action == "delete" ? L("删除") : L("合并"), onConfirm: {
                    var fields: [String: Any] = ["person_id": pending.person.id]
                    if let target = pending.target { fields["target_id"] = target.id }
                    let action = pending.action; self.pending = nil
                    Task { await model.manage(action, fields: fields) }
                }, onCancel: { self.pending = nil }).padding(Space.s)
            }
        }
    }

    private func row(_ person: VoiceLibraryModel.Person, tint: Color) -> some View {
        HStack(spacing: Space.s) {
            Text(String(person.name.first ?? "?")).typography(.secondary, .semibold).foregroundStyle(.white)
                .frame(width: 28, height: 28).background(Circle().fill(tint))
            if editing == person.id {
                TextField(L("名字"), text: $name).textFieldStyle(.plain).typography(.body)
                    .padding(.horizontal, 8).frame(height: ControlHeight.field)
                    .background(RoundedRectangle(cornerRadius: Radius.small, style: .continuous).fill(Surface.fill.color))
                    .onSubmit(save)
                QuietButton(title: L("保存"), emphasized: true, action: save)
                QuietButton(title: L("取消")) { editing = nil }
            } else {
                Text(person.name).typography(.body, .semibold).lineLimit(1)
                Spacer()
                Tag(text: L("%@ 份样本", person.sample_count))
                QuietMenu(title: "⋯") {
                    Button(L("改名")) { name = person.name; editing = person.id }
                    Menu(L("合并到…")) {
                        ForEach(model.people.filter { $0.id != person.id }) { target in
                            Button(target.name) { pending = ("merge", person, target) }
                        }
                    }
                    Button(L("删除这个人…"), role: .destructive) { pending = ("delete", person, nil) }
                }
            }
        }
        .padding(.vertical, 6)
    }

    private func save() {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let id = editing, !trimmed.isEmpty else { return }
        editing = nil
        Task { await model.manage("rename", fields: ["person_id": id, "name": trimmed]) }
    }
}
