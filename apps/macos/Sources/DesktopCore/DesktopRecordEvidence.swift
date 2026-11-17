import Foundation

/// Persisted L0 evidence; send candidates never populate confirmation.
public struct DesktopRecordEvidence: Codable, Sendable, Hashable {
    public struct ObjectIdentity: Codable, Sendable, Hashable {
        public var key: String
        public var basis: String
        public init(key: String, basis: String) { self.key = key; self.basis = basis }
    }
    public struct Reading: Codable, Sendable, Hashable {
        public var text: String
        public var occurrence: String?
        public init(text: String, occurrence: String? = nil) { self.text = text; self.occurrence = occurrence }
    }
    public struct Confirmation: Codable, Sendable, Hashable {
        public var source_hash: String? = nil
        public var observation: String
        public var message_id: String
        public var text: String
        public var basis: String
        public init(observation: String, messageID: String, text: String, basis: String) {
            self.observation = observation; self.message_id = messageID; self.text = text; self.basis = basis
        }
        public var isValid: Bool { !observation.isEmpty && !message_id.isEmpty && !text.isEmpty && ["source_send", "outgoing_message"].contains(basis) }
    }
    public var confirmation: Confirmation?
    public var format = "candid-record-evidence/1"
    public var object: ObjectIdentity
    public var field: String?
    public var input_value: String?
    public var input_status: String
    public var active = true
    public var reading: [Reading] = []
    // Source adapters must provide the outgoing message ID and the referenced
    // observation before setting confirmation. Generic AX supplies none.
    public init(object: ObjectIdentity, field: String? = nil, inputValue: String? = nil, inputStatus: String = "disabled") {
        self.object = object; self.field = field; self.input_value = inputValue; self.input_status = inputStatus
    }
    public var isValid: Bool {
        ["candid-record-evidence/1", "candid-record-evidence/2"].contains(format) && !object.key.isEmpty
        && ["path", "url", "sidebar", "title", "window"].contains(object.basis)
        && ["captured", "disabled", "unavailable", "protected", "composing"].contains(input_status)
        && (input_value == nil || (input_status == "captured" && !(field ?? "").isEmpty && active))
        && (confirmation?.isValid ?? true) && (confirmation == nil || active)
        && (confirmation?.source_hash == nil || format == "candid-record-evidence/2")
        && (format != "candid-record-evidence/2" || confirmation?.basis != "source_send" || (confirmation?.source_hash?.count == 64 && confirmation?.source_hash?.allSatisfy(\.isHexDigit) == true))
        && (active || reading.isEmpty)
    }
    public static func object(document: String?, url: String?, app: String, title: String, window: String) -> ObjectIdentity {
        if let document, !document.isEmpty { return .init(key: document, basis: "path") }
        if let url, !url.isEmpty { return .init(key: url, basis: "url") }
        let title = normalizedTitle(title)
        return title.isEmpty ? .init(key: "\(app):\(window)", basis: "window") : .init(key: "\(app):\(title)", basis: "title")
    }
    public static func normalizedTitle(_ title: String) -> String {
        title.replacingOccurrences(of: #"^\(\d+\)\s*"#, with: "", options: .regularExpression)
            .replacingOccurrences(of: #"\s+\d{1,2}:\d{2}(:\d{2})?\s*$"#, with: "", options: .regularExpression)
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
