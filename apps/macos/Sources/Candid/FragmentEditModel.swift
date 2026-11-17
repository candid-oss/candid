import Foundation
import Observation
import CandidClient

extension Notification.Name { static let candidEditFragment = Notification.Name("Candid.editFragment") }
struct FragmentEditTarget {
    var activities: [String]
    var screenshotID: String? = nil
    var mediaID: UUID? = nil
}

@MainActor @Observable final class FragmentEditModel: Identifiable {
    let id = UUID()
    var title = "" { didSet { if title != oldValue { changed() } } }
    var notes = "" { didSet { if notes != oldValue { changed() } } }
    var automaticTitle = false { didSet { if automaticTitle != oldValue { changed() } } }
    var automaticNotes = false { didSet { if automaticNotes != oldValue { changed() } } }
    var loading = true
    var saving = false
    var error: String?
    private var requestID = UUID().uuidString
    private var eventIDs: [String] = []
    private let target: FragmentEditTarget
    private let suppliedChannel: LocalChannel?
    init(target: FragmentEditTarget, channel: LocalChannel? = nil) {
        self.target = target; suppliedChannel = channel
    }
    private func changed() { requestID = UUID().uuidString }
    private func channel() throws -> LocalChannel {
        guard let channel = suppliedChannel ?? TimelineService.shared.channel else { throw CoreError.unreachable("尚未就绪") }
        return channel
    }
    func load() async {
        loading = true; defer { loading = false }
        do {
            let channel = try channel()
            var records: [AppRecord] = []
            struct ActivityAnswer: Decodable { var activity: AppActivity? }
            for id in target.activities {
                let answer: ActivityAnswer = try await channel.app("app_activity", ["id": .string(id), "limit": .int(10000)])
                guard let activity = answer.activity else { throw CoreError.badResponse }
                if let shot = target.screenshotID {
                    guard let record = activity.screenshots.first(where: { $0.id == shot }) else { throw CoreError.badResponse }
                    records.append(record)
                } else { records += activity.observations }
            }
            eventIDs = Array(Set(records.map(\.event_id))).sorted()
            guard !eventIDs.isEmpty else { throw CoreError.badResponse }
            struct Answer: Decodable { var annotation: Annotation? }
            struct Annotation: Decodable { var title: String?; var notes: String? }
            let answer: Answer = try await channel.post("/local/records", body: ["action": CoreJSON.string("annotation_get"), "event_ids": .array(eventIDs.map(CoreJSON.string))])
            title = answer.annotation?.title ?? ""; notes = answer.annotation?.notes ?? ""
            automaticTitle = answer.annotation?.title == nil; automaticNotes = answer.annotation?.notes == nil
            error = nil
        } catch { self.error = "片段暂不可用，可重试" }
    }
    func clear() { title = ""; notes = ""; automaticTitle = false; automaticNotes = false }
    func reset() { title = ""; notes = ""; automaticTitle = true; automaticNotes = true }
    func save() async -> Bool {
        guard !loading, !saving, !eventIDs.isEmpty else { return false }
        let key = requestID, submittedTitle = title, submittedNotes = notes
        let autoTitle = automaticTitle, autoNotes = automaticNotes
        saving = true; defer { saving = false }
        do {
            let _: CoreJSON = try await channel().post("/local/records", body: ["action": CoreJSON.string("annotation_save"), "event_ids": .array(eventIDs.map(CoreJSON.string)), "request_id": .string(key), "title": autoTitle ? .null : .string(submittedTitle), "notes": autoNotes ? .null : .string(submittedNotes)])
            error = nil
            return key == requestID
        } catch { self.error = "未保存，可重试"; return false }
    }
}
