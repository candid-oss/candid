import Foundation
import Observation
import CandidClient

/// The voice library is a list of people (agent-platform-v1 §2.6). Samples come only from 「这是谁？」 in a
/// recording; here you rename, merge or delete people.
@MainActor @Observable final class VoiceLibraryModel {
    struct Person: Decodable, Identifiable { let id: String; let name: String; let sample_count: Int }
    struct People: Decodable { let people: [Person]; let cleanup_message: String? }
    @ObservationIgnored private var transport: (@MainActor ([String: Any]) async throws -> Data)?
    init(transport: (@MainActor ([String: Any]) async throws -> Data)? = nil) { self.transport = transport }
    var cleanupMessage: String?
    var people: [Person] = []
    var error: String?
    var busy = false
    func call(_ values: [String: Any]) async throws -> Data {
        if let transport { return try await transport(values) }
        guard let channel = TimelineService.shared.channel else { throw MediaWorkspaceError.configuration }
        return try await channel.send("POST", "/local/voiceprints", body: JSONSerialization.data(withJSONObject: values), timeout: 130).checked()
    }
    func refresh() async {
        guard !busy else { return }; busy = true; defer { busy = false }
        do {
            if transport == nil { try await AIRuntime.shared.syncCore() }
            let snapshot = try JSONDecoder().decode(People.self, from: await call(["action": "list"]))
            people = snapshot.people; cleanupMessage = snapshot.cleanup_message; error = nil
        } catch { self.error = error.localizedDescription }
    }
    /// rename (person_id, name), merge (person_id → target_id) or delete (person_id); the core keeps recordings in step.
    func manage(_ action: String, fields: [String: Any]) async {
        guard !busy else { return }; busy = true; defer { busy = false }
        do {
            var body = fields; body["action"] = action; body["confirmed"] = true
            people = try JSONDecoder().decode(People.self, from: await call(body)).people
            error = nil
        } catch { self.error = error.localizedDescription }
    }
}
