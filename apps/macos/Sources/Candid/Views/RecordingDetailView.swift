import SwiftUI
import CandidLocalization
import UniformTypeIdentifiers
import ConsoleCore

struct RecordingDetailView: View {
    @Bindable var store: ConsoleStore
    @StateObject private var playback = PlaybackController()
    @ViewState private var exportDocument: RecordingExport?
    @ViewState private var exporting = false
    @ViewState private var preparingExport = false
    @ViewState private var exportType: UTType = .wav
    @ViewState private var exportFilename = "recording.wav"
    private var transcript: Transcript? { store.snapshot?.transcripts.first { $0.asset_id == store.selectedAsset?.id } }
    var body: some View {
        Group {
            if let asset = store.selectedAsset {
                VStack(alignment: .leading, spacing: Space.l) {
                    VStack(alignment: .leading, spacing: 6) {
                        Text(asset.filename).typography(.section).textSelection(.enabled)
                        Text("\(asset.sourceTitle) · \(asset.durationLabel) · \(ByteCountFormatter.string(fromByteCount: Int64(asset.size), countStyle: .file))")
                            .typography(.secondary).foregroundStyle(.secondary)
                        if asset.isSynthetic { Tag(text: L("合成样例"), tint: Palette.warning) }
                    }
                    SettingsGroup(title: L("回听")) {
                        VStack(alignment: .leading, spacing: Space.s) {
                            HStack(spacing: 6) {
                                PrimaryButton(title: playback.loading ? L("载入中") : playback.isPlaying ? L("暂停") : L("播放")) {
                                    Task { await playback.toggle(asset: asset, service: store.service) }
                                }
                                .disabled(playback.loading || store.connection != .online).accessibilityIdentifier("audio.play")
                                QuietButton(title: L("停止")) { playback.stop() }
                                    .disabled(playback.duration == 0).accessibilityLabel(L("停止播放"))
                                Spacer()
                                Text(String(format: "%.1f / %.1f s", playback.position, playback.duration > 0 ? playback.duration : asset.metadata.duration_s ?? 0))
                                    .typography(.secondary, digits: true).foregroundStyle(.secondary)
                            }
                            Slider(value: Binding(get: { playback.position }, set: { playback.seek($0) }), in: 0...max(1, playback.duration))
                                .controlSize(.small)
                                .disabled(playback.duration == 0).accessibilityLabel(L("录音播放位置"))
                            if let error = playback.error { InlineMessage(kind: .error, text: error) }
                        }
                        .padding(.top, Space.xs)
                    } accessory: {
                        QuietButton(title: preparingExport ? L("准备中") : L("导出 WAV")) { exportAudio(asset) }
                            .disabled(preparingExport || store.connection != .online)
                    }
                    SettingsGroup(title: L("文字")) {
                        VStack(alignment: .leading, spacing: Space.s) {
                            transcriptContent
                            HStack(spacing: 6) {
                                if store.snapshot?.asr.busy == true {
                                    ProgressView().controlSize(.small); Text(L("本机正在处理")).typography(.secondary).foregroundStyle(.secondary)
                                }
                                Spacer()
                                QuietButton(title: transcript?.result == nil ? L("转写这段录音") : L("重新转写"), emphasized: transcript?.result == nil) {
                                    Task { await store.transcribe() }
                                }
                                .disabled(!store.canTranscribe).accessibilityIdentifier("asr.start")
                            }
                        }
                        .padding(.top, Space.xs)
                    } accessory: {
                        if let text = transcript?.result?.text, !text.isEmpty {
                            QuietButton(title: L("导出文字")) {
                                exportType = .plainText; exportFilename = (asset.filename as NSString).deletingPathExtension + ".txt"
                                exportDocument = RecordingExport(data: Data(text.utf8)); exporting = true
                            }
                        }
                    }
                    FoldRow(title: L("文件信息")) {
                        info(L("传输校验"), asset.transfer_hash_verified ? L("已验证") : L("未提供")); HairlineDivider()
                        info(L("采样率"), "\(asset.metadata.sample_rate ?? 0) Hz"); HairlineDivider()
                        info(L("声道"), "\(asset.metadata.channels ?? 0)"); HairlineDivider()
                        info(L("削波"), String(format: "%.2f%%", asset.metadata.clipped_percent ?? 0)); HairlineDivider()
                        if let variant = store.snapshot?.codec_variants?.first(where: { $0.decoded_asset_id == asset.id }) {
                            info(L("原件"), L("Opus %@ · 回听和导出为解码 WAV", variant.bitrate.map { "\($0 / 1000) kbps" } ?? "")); HairlineDivider()
                        }
                        info("SHA256", asset.id, mono: true)
                    }
                }
            } else {
                Text(L("选择一段录音")).typography(.headline).foregroundStyle(.secondary)
            }
        }
        .onChange(of: store.selectedAsset?.id) { _, _ in playback.stop() }
        .onChange(of: store.connectionRevision) { _, _ in playback.stop() }
        .onDisappear { playback.stop() }
        .fileExporter(isPresented: $exporting, document: exportDocument, contentType: exportType, defaultFilename: exportFilename) { result in
            if case let .failure(error) = result { store.reportError(error.localizedDescription) }
        }
    }
    @ViewBuilder private var transcriptContent: some View {
        if let error = transcript?.error, !error.isEmpty { InlineMessage(kind: .error, text: error) }
        if let result = transcript?.result {
            Text(result.text?.isEmpty == false ? result.text! : L("该段未识别到文字"))
                .typography(.body).lineSpacing(6).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
            Text("\(result.model) · \(result.provider)\(result.timestamps_available ? "" : L(" · 无逐字时间戳"))")
                .typography(.caption).foregroundStyle(.secondary)
            if let revision = result.model_revision { Text(L("模型版本 %@", revision)).typography(.caption).foregroundStyle(.secondary).textSelection(.enabled) }
        } else {
            Text(transcript?.status == "running" ? L("正在转写…") : L("尚未转写"))
                .typography(.secondary).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
    }
    private func info(_ title: String, _ value: String, mono: Bool = false) -> some View {
        SettingRow(title: title) {
            Text(value).typography(mono ? .caption : .body, .regular).fontDesign(mono ? .monospaced : .default)
                .foregroundStyle(.secondary).textSelection(.enabled).lineLimit(2).multilineTextAlignment(.trailing)
        }
    }

    private func exportAudio(_ asset: AudioAsset) {
        preparingExport = true
        let revision = store.connectionRevision
        Task {
            defer { preparingExport = false }
            do {
                let data = try await store.service.media(assetID: asset.id)
                guard revision == store.connectionRevision else { return }
                exportDocument = RecordingExport(data: data); exportType = .wav; exportFilename = asset.filename; exporting = true
            } catch { store.reportError(error.localizedDescription) }
        }
    }
}

private struct RecordingExport: FileDocument {
    static var readableContentTypes: [UTType] { [.wav, .plainText] }
    var data: Data
    init(data: Data) { self.data = data }
    init(configuration: ReadConfiguration) throws { data = configuration.file.regularFileContents ?? Data() }
    func fileWrapper(configuration: WriteConfiguration) throws -> FileWrapper { FileWrapper(regularFileWithContents: data) }
}
