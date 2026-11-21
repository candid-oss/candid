import CandidLocalization
import Foundation

/// How full the screenshot storage limit is (data model v1 §6, §8). Over the limit the core steps the oldest
/// screenshots (older than 7 days) down to text only until usage is back under `warningRatio`; this says what the
/// user should see meanwhile, so a full limit never stops capture silently.
public struct DesktopStorageBudget: Sendable, Equatable {
    public enum Level: Sendable, Equatable { case ok, nearlyFull, full }

    /// Usage at or above this share of the limit is shown as a warning, and is where freeing space stops.
    public static let warningRatio = 0.9
    /// The choices the settings page offers for the limit.
    public static let limitOptions: [Int64] = [1, 2, 5, 10, 20, 50, 100].map { Int64($0) * 1024 * 1024 * 1024 }

    public var usedBytes: Int64
    public var limitBytes: Int64
    /// A screenshot was refused for the limit since the last one was saved (freeing space did not help).
    public var refused: Bool
    /// Screenshot originals stepped down to text only so far, in total.
    public var freedBytes: Int64
    public var freedScreenshots: Int

    public init(usedBytes: Int64, limitBytes: Int64, refused: Bool = false, freedBytes: Int64 = 0, freedScreenshots: Int = 0) {
        self.usedBytes = usedBytes; self.limitBytes = limitBytes; self.refused = refused
        self.freedBytes = freedBytes; self.freedScreenshots = freedScreenshots
    }

    /// Where freeing space stops: ~90% of the limit.
    public static func target(limit: Int64) -> Int64 { Int64(Double(limit) * warningRatio) }

    public var level: Level {
        if refused || (limitBytes > 0 && usedBytes >= limitBytes) { return .full }
        if limitBytes > 0, Double(usedBytes) >= Double(limitBytes) * Self.warningRatio { return .nearlyFull }
        return .ok
    }

    /// The next larger limit the settings offer, or nil at the largest.
    public var raisedLimit: Int64? { Self.limitOptions.first { $0 > limitBytes } }

    /// What to tell the user, or nil while there is room.
    public var message: String? {
        let used = ByteCountFormatter.string(fromByteCount: usedBytes, countStyle: .file)
        let limit = ByteCountFormatter.string(fromByteCount: limitBytes, countStyle: .file)
        switch level {
        case .ok: return nil
        case .nearlyFull: return L("截图空间已用 %@ / %@，接近上限。超过后最早的截图只留文字（7 天内的不动）。", used, limit)
        case .full: return refused
            ? L("截图空间已满（%@ / %@），新截图没有保存：可改为只留文字的旧截图（7 天前）不够腾出空间。请提高上限。", used, limit)
            : L("截图空间已满（%@ / %@），正在把最早的截图改为只留文字。", used, limit)
        }
    }

    /// A few words for a status line, or nil while there is room.
    public var shortLabel: String? {
        switch level {
        case .ok: return nil
        case .nearlyFull: return L("截图空间接近上限")
        case .full: return refused ? L("截图空间已满，新截图未保存") : L("截图空间已满，正在腾出空间")
        }
    }

    /// What freeing space has done so far, or nil before it ran.
    public var freedSummary: String? {
        guard freedScreenshots > 0 else { return nil }
        return L("已腾出 %@：%@ 张旧截图只留文字（事件、OCR 和辅助功能文字都在）。",
                 ByteCountFormatter.string(fromByteCount: freedBytes, countStyle: .file), freedScreenshots)
    }
}
