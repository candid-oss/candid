import CandidClient
import CandidLocalization
import Foundation
import Observation

/// 「可能是同一人」 (data-layer plan §1.4): pairs of names the model thinks are one person but no rule backs up (merge,
/// or "not the same person"), and the merges it made on its own (undo = "not the same person"). All of it comes from the
/// core's cached verdicts (`app_people_candidates`); your answer is a correction (`app_person_correct`).
@MainActor @Observable final class PeopleCandidatesModel {
    struct Side: Decodable, Hashable { var key: String; var name: String }
    struct Item: Decodable, Identifiable, Hashable {
        var a: String
        var b: String
        var reason: String
        var a_person: Side
        var b_person: Side
        var id: String { a + "|" + b }
    }
    private struct Answer: Decodable { var candidates: [Item]; var auto_merged: [Item] }
    private struct Done: Decodable {}

    private(set) var candidates: [Item] = []
    private(set) var autoMerged: [Item] = []
    private(set) var working: Set<String> = []
    var problems: [String: String] = [:]
    private let channel: () -> LocalChannel?

    init(channel: @escaping () -> LocalChannel?) { self.channel = channel }

    func refresh() async {
        guard let channel = channel() else { return }
        // An older core without the tool simply has nothing to show.
        guard let answer: Answer = try? await channel.app("app_people_candidates", [:]) else { return }
        candidates = answer.candidates
        autoMerged = Array(answer.auto_merged.prefix(5))
    }

    /// 合并 (a merge correction) or 不是同一人 / 撤销 (a split correction).
    func answer(_ item: Item, merge: Bool) async {
        guard !working.contains(item.id), let channel = channel() else { return }
        working.insert(item.id); defer { working.remove(item.id) }
        problems[item.id] = nil
        do {
            let _: Done = try await channel.app("app_person_correct", ["action": .string(merge ? "merge" : "split"), "key": .string(item.a), "other": .string(item.b)])
        } catch {
            problems[item.id] = L("没能保存：%@", error.localizedDescription)
        }
        await refresh()
    }

    /// `handle:wechat/张三` → 「张三 · wechat」; an address or a name as it is.
    static func label(_ key: String) -> String {
        if key.hasPrefix("handle:"), let slash = key.firstIndex(of: "/") {
            let app = key.dropFirst("handle:".count).prefix(while: { $0 != "/" })
            return "\(key[key.index(after: slash)...]) · \(app)"
        }
        for prefix in ["email:", "name:", "voice:"] where key.hasPrefix(prefix) { return String(key.dropFirst(prefix.count)) }
        return key
    }
}
