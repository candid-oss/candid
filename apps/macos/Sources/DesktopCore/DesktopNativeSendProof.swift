import Foundation
import CandidClient

/// Matches an already authorised, indexed source message. Titles and text are
/// never used to infer a session: a platform adapter must expose its exact ref.
public enum DesktopNativeSendProof {
    public struct Message: Sendable {
        public var session: String
        public var reference: String
        public var at: Date
        public var text: String
        public var sourceHash: String? = nil
        public var complete: Bool
        public init(session: String, reference: String, at: Date, text: String, complete: Bool = true) {
            self.session = session; self.reference = reference; self.at = at; self.text = text; self.complete = complete
        }
    }
    public static func session(for record: DesktopRecord) -> String? {
        guard let e = record.context?.recordEvidence, ["path", "url", "sidebar"].contains(e.object.basis) else { return nil }
        let key = e.object.key
        guard ["codex", "claude_code", "pi"].contains(where: { key.hasPrefix("agent:\($0):") }),
              !key.contains("#"), !key.contains("/"), !key.contains(".."), key.split(separator: ":").count == 3 else { return nil }
        return key
    }
    public static func confirmation(draft: DesktopRecord, messages: [Message], now: Date = Date()) -> DesktopRecord? {
        guard let session = session(for: draft), let e = draft.context?.recordEvidence,
              e.active, e.input_status == "captured", let text = e.input_value, !text.isEmpty,
              e.field?.isEmpty == false, draft.context?.isProtected != true, draft.context?.isComposing != true,
              now.timeIntervalSince(draft.timestamp) <= 30, now >= draft.timestamp else { return nil }
        let candidates = messages.filter {
            $0.session == session && $0.reference.hasPrefix(session + "#") && $0.reference.count > session.count + 1 &&
            $0.complete && $0.text == text && $0.at >= draft.timestamp.addingTimeInterval(-0.25) &&
            $0.at <= now && $0.at <= draft.timestamp.addingTimeInterval(30)
        }
        // Two same-text sends or duplicate candidate identities are ambiguous.
        guard candidates.count == 1, let message = candidates.first else { return nil }
        var result = draft
        result.id = "native-send-" + DesktopEventMapping.sha256(Data(message.reference.utf8))
        result.timestamp = message.at; result.kind = .context; result.trigger = "send_confirmed"
        result.detail = "会话来源已确认发出"
        result.context?.recordEvidence?.confirmation = .init(observation: draft.id, messageID: message.reference, text: text, basis: "source_send")
        if let hash = message.sourceHash {
            result.context?.recordEvidence?.format = "candid-record-evidence/2"
            result.context?.recordEvidence?.confirmation?.source_hash = hash
        }
        return result
    }

    /// The index locates candidates; the core rereads the exact authorised
    /// source reference because indexed paragraphs normalise whitespace.
    /// No session enumeration or private file parsing happens in Swift.
    public static func messages(channel: LocalChannel, since: Date, now: Date, sessions: Set<String> = [], excluding: Set<String> = []) async throws -> [Message] {
        struct SourceMessage: Decodable { var session: String; var reference: String; var at_ms: Int64; var text: String; var source_hash: String }
        struct Page: Decodable { var messages: [SourceMessage]; var next_cursor: String?; var incomplete: Bool }
        var args: [String: CoreJSON] = ["action": .string("candidates"), "start_ms": .int(Int64(since.addingTimeInterval(-0.25).timeIntervalSince1970 * 1000)), "end_ms": .int(Int64(now.addingTimeInterval(0.001).timeIntervalSince1970 * 1000)), "sessions": .array(sessions.sorted().map(CoreJSON.string))]
        var result: [Message] = []
        for _ in 0..<8 {
            try Task.checkCancellation()
            let bytes = try await channel.tool("source_query", arguments: JSONEncoder().encode(args))
            let page = try JSONDecoder().decode(Page.self, from: bytes)
            guard !page.incomplete else { return [] }
            result += page.messages.filter { !excluding.contains($0.reference) }.map {
                var message = Message(session: $0.session, reference: $0.reference, at: Date(timeIntervalSince1970: Double($0.at_ms) / 1000), text: $0.text)
                message.sourceHash = $0.source_hash
                return message
            }
            if let next = page.next_cursor { args["cursor"] = .string(next) } else { return result }
        }
        return [] // An incomplete index is never proof of a unique send.
    }
}
