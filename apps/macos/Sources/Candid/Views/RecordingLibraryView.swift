import SwiftUI
import ConsoleCore

struct RecordingLibraryView: View {
    @Bindable var store: ConsoleStore
    let importAction: () -> Void
    @ViewState private var query = ""
    @ViewState private var filter: RecordingFilter = .all
    @ViewState private var sort: RecordingSort = .newest
    private var items: [AudioAsset] { RecordingLibrary.recordings(in: store.snapshot, query: query, filter: filter, sort: sort) }
    var body: some View {
        HStack(alignment: .top, spacing: Space.l) {
            VStack(alignment: .leading, spacing: Space.s) {
                SearchField(prompt: "搜索文件或已有文字", text: $query).accessibilityLabel("搜索录音")
                HStack {
                    MenuChoice(options: RecordingFilter.allCases.map { ($0, $0.rawValue) }, selection: $filter)
                    Spacer()
                    MenuChoice(options: RecordingSort.allCases.map { ($0, $0.rawValue) }, selection: $sort).accessibilityLabel("录音排序")
                }
                if items.isEmpty {
                    VStack(alignment: .leading, spacing: 6) {
                        Text(store.recordings.isEmpty ? "还没有录音" : "没有匹配的录音").typography(.headline)
                        if store.recordings.isEmpty {
                            QuietButton(title: "导入 WAV", emphasized: true, action: importAction)
                                .disabled(store.connection != .online || store.service.isMock).offset(x: -9)
                        } else {
                            QuietButton(title: "清除筛选") { query = ""; filter = .all }.offset(x: -9)
                        }
                    }
                    .padding(.top, Space.s)
                    Spacer()
                } else {
                    ScrollView {
                        LazyVStack(spacing: 2) {
                            ForEach(items) { asset in
                                Button { store.selectedAssetID = asset.id } label: {
                                    LibraryRecordingRow(asset: asset, transcript: store.snapshot?.transcripts.first { $0.asset_id == asset.id })
                                        .padding(.horizontal, Space.s)
                                        .background(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous)
                                            .fill(store.selectedAssetID == asset.id ? Surface.selected.color : .clear))
                                        .contentShape(Rectangle())
                                }
                                .buttonStyle(.plain)
                                .accessibilityAddTraits(store.selectedAssetID == asset.id ? .isSelected : [])
                            }
                        }
                    }
                    .scrollIndicators(.hidden)
                }
                Text("\(items.count) 条录音").typography(.caption, .regular).foregroundStyle(.secondary)
            }
            .frame(width: 300)
            ScrollView {
                VStack(alignment: .leading, spacing: Space.m) {
                    if let asset = store.selectedAsset {
                        Text(asset.date?.formatted(date: .complete, time: .shortened) ?? "录音详情")
                            .typography(.secondary).foregroundStyle(.secondary)
                    }
                    if !items.isEmpty { RecordingDetailView(store: store) }
                }
                .frame(maxWidth: 820, alignment: .leading)
            }
            .scrollIndicators(.hidden)
            .frame(maxWidth: .infinity)
        }
        .padding(.horizontal, Space.page).padding(.bottom, Space.l)
        .animation(Motion.snappy, value: store.selectedAssetID)
        .onChange(of: items.map(\.id)) { _, ids in
            if !ids.contains(store.selectedAssetID ?? "") { store.selectedAssetID = ids.first }
        }
    }
}

private struct LibraryRecordingRow: View {
    let asset: AudioAsset
    let transcript: Transcript?
    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "waveform").foregroundStyle(MediaGlyph.tint(.audio)).frame(width: 24).padding(.top, 3)
            VStack(alignment: .leading, spacing: 7) {
                Text(asset.filename).typography(.body, .medium).lineLimit(2)
                Text("\(asset.sourceTitle) · \(asset.durationLabel)").typography(.secondary).foregroundStyle(.secondary)
                HStack {
                    Text(asset.date?.formatted(date: .abbreviated, time: .shortened) ?? "")
                    Spacer(minLength: 4)
                    if transcript?.result != nil { Image(systemName: "text.alignleft").help("已有文字") }
                    else if transcript?.status == "running" { ProgressView().controlSize(.mini) }
                    else if transcript?.status == "failed" { Image(systemName: "exclamationmark.circle").help("上次转写失败") }
                }.typography(.caption, .regular).foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
        }.padding(.vertical, 10)
    }
}
