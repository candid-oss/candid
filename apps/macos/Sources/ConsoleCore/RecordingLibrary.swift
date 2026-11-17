import Foundation
import CandidLocalization

public enum RecordingFilter: String, CaseIterable, Identifiable, Sendable {
    case all = "全部来源", device = "设备录音", imported = "本机导入", synthetic = "合成样例"
    public var id: String { rawValue }
    public func includes(_ asset: AudioAsset) -> Bool {
        switch self {
        case .all: true
        case .device: ["serial", "serial_opus", "http", "http_opus"].contains(asset.source)
        case .imported: asset.source == "local_file"
        case .synthetic: asset.isSynthetic
        }
    }
}
public enum RecordingSort: String, CaseIterable, Identifiable, Sendable {
    case newest = "最新优先", oldest = "最早优先", longest = "时长优先"
    public var id: String { rawValue }
}
public enum RecordingLibrary {
    public static func recordings(in state: BenchState?, query: String, filter: RecordingFilter, sort: RecordingSort) -> [AudioAsset] {
        let term = query.trimmingCharacters(in: .whitespacesAndNewlines)
        let items = (state?.audioAssets ?? []).filter { asset in
            filter.includes(asset) && (term.isEmpty || asset.filename.localizedCaseInsensitiveContains(term) || asset.id.localizedCaseInsensitiveContains(term) || (state?.transcripts.contains { $0.asset_id == asset.id && ($0.result?.text?.localizedCaseInsensitiveContains(term) ?? false) } ?? false))
        }
        return items.sorted { lhs, rhs in
            if sort == .longest, lhs.metadata.duration_s != rhs.metadata.duration_s { return (lhs.metadata.duration_s ?? 0) > (rhs.metadata.duration_s ?? 0) }
            if sort == .oldest { return lhs.created_at < rhs.created_at }
            return lhs.created_at > rhs.created_at
        }
    }
}
public extension AudioAsset {
    var sourceTitle: String {
        switch source {
        case "synthetic": L("合成样例")
        case "local_file": L("本机导入")
        case "serial", "serial_opus": L("USB 设备")
        case "http", "http_opus": L("Wi-Fi 设备")
        default: source
        }
    }
    var durationLabel: String {
        let seconds = Int(metadata.duration_s ?? 0)
        return seconds >= 60 ? L("%@ 分 %@ 秒", seconds / 60, seconds % 60) : String(format: L("%.1f 秒"), metadata.duration_s ?? 0)
    }
}
