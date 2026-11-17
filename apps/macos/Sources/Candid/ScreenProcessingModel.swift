import Foundation
import Observation
import CandidClient

@MainActor @Observable final class ScreenProcessingModel {
    struct Episode: Decodable, Identifiable { var id: String; var revision: Int; var start_ms: Int64; var end_ms: Int64; var title: String?; var summary: String?; var model: String?; var status: String? }
    struct Report: Decodable { var raw_chars: Int; var input_chars: Int; var episodes: Int; var ended: Int; var bounded: Bool }
    struct Snapshot: Decodable { var report: Report?; var episodes: [Episode] }
    struct Detail: Decodable {
        struct Projection: Decodable {
            struct Line: Decodable { var event_id: String; var start: Int; var end: Int; var text: String; var family: String; var weight: Double; var is_new: Bool }
            var lines: [Line]
        }
        var episode: Projection
    }
    var episodes: [Episode] = []
    var report: Report?
    var selected: String?
    var lines: [Detail.Projection.Line] = []
    var error: String?
    var busy = false
    var editing = false
    var title = ""
    var summary = ""
    private let channel: () -> LocalChannel?
    init(channel: @escaping () -> LocalChannel? = { TimelineService.shared.channel }) { self.channel = channel }
    private func call<T: Decodable>(_ body: [String: Any]) async throws -> T {
        guard let channel = channel() else { throw PiAgentError.noCore }
        let data = try await channel.send("POST", "/local/screen", body: JSONSerialization.data(withJSONObject: body)).checked()
        return try JSONDecoder().decode(T.self, from: data)
    }
    func refresh() async {
        do { let result: Snapshot = try await call(["action": "status"]); report = result.report; episodes = result.episodes; error = nil }
        catch { self.error = AIPageModel.explain(error) }
    }
    func select(_ episode: Episode) async {
        if selected == episode.id { selected = nil; return }
        selected = episode.id; title = episode.title ?? ""; summary = episode.summary ?? ""; editing = false
        do { let detail: Detail = try await call(["action": "detail", "id": episode.id]); lines = detail.episode.lines; error = nil }
        catch { self.error = AIPageModel.explain(error) }
    }
    func command(_ action: String, id: String? = nil, family: String? = nil) async {
        busy = true; defer { busy = false }
        var body: [String: Any] = ["action": action]
        if let id { body["id"] = id }
        if let family { body["family"] = family }
        if action == "edit" { body["title"] = title; body["summary"] = summary }
        do {
            let _: AutomationAck = try await call(body)
            if action == "important", let family { lines = lines.map { var line = $0; if line.family == family { line.weight = 1 }; return line } }
            editing = false; await refresh()
        }
        catch { self.error = AIPageModel.explain(error) }
    }
}
