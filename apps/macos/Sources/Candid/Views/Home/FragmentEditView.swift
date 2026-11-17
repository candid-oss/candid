import SwiftUI
import CandidLocalization

struct FragmentEditView: View {
    @Bindable var model: FragmentEditModel
    let close: () -> Void
    var body: some View {
        VStack(alignment: .leading, spacing: Space.m) {
            Text(L("编辑标题与备注")).typography(.headline)
            if model.loading { ProgressView() }
            TextField(model.automaticTitle ? L("自动标题") : L("标题（可留空）"), text: Binding(get: { model.title }, set: { model.title = $0; model.automaticTitle = false }))
                .textFieldStyle(.plain).typography(.body)
            TextField(model.automaticNotes ? L("添加备注") : L("备注（可留空）"), text: Binding(get: { model.notes }, set: { model.notes = $0; model.automaticNotes = false }), axis: .vertical)
                .textFieldStyle(.plain).typography(.body).lineLimit(3...8)
            HStack {
                QuietButton(title: L("清空标题与备注")) { model.clear() }
                QuietButton(title: L("恢复自动值")) { model.reset() }
            }
            if model.automaticTitle && model.automaticNotes { Text(L("保存后使用自动值")).typography(.secondary).foregroundStyle(.secondary) }
            if let error = model.error {
                Text(error).typography(.secondary).foregroundStyle(.red)
                if !model.saving { QuietButton(title: L("重新读取")) { Task { await model.load() } } }
            }
            HStack {
                QuietButton(title: L("取消"), action: close)
                Spacer()
                QuietButton(title: model.saving ? L("保存中…") : L("保存")) { Task { if await model.save() { close() } } }
                    .disabled(model.loading || model.saving)
            }
        }
        .padding(Space.m).frame(width: 420).floatingSurface()
        .task { await model.load() }
    }
}
