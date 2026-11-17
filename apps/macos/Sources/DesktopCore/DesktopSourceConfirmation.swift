import Foundation
import CandidLocalization

/// Source-specific proof policy. Ordinary AX text has no message identity and
/// cannot confirm a send. State is bounded and cleared when capture is paused
/// or its authorization changes. Existing messages on first read are baseline.
public struct DesktopSourceConfirmation: Sendable {
    public static let fixtureBundleID = "ai.candid.CaptureChatFixture"
    private var seen: [String: Set<String>] = [:]
    private var fields: [String: String] = [:]
    public init() {}
    public mutating func reset() { seen.removeAll(); fields.removeAll() }
    public mutating func confirmations(for record: DesktopRecord) -> [DesktopRecord] {
        guard [DesktopRecordKind.context, .window].contains(record.kind), record.bundleID == Self.fixtureBundleID,
              let evidence = record.context?.recordEvidence, evidence.active,
              evidence.input_status != "disabled", record.context?.isProtected != true,
              record.context?.isComposing != true else { return [] }
        let source = "\(record.bundleID)|\(evidence.object.key)"
        if let field = evidence.field { fields[source] = field }
        let outgoing = evidence.reading.filter { $0.occurrence?.hasPrefix("candid.outgoing.") == true && !$0.text.isEmpty }
        guard let previous = seen[source] else {
            if seen.count >= 128 { reset() }
            seen[source] = Set(outgoing.compactMap(\.occurrence)); return []
        }
        var result: [DesktopRecord] = []
        for message in outgoing {
            guard let id = message.occurrence, !previous.contains(id), seen[source]?.contains(id) != true else { continue }
            seen[source, default: []].insert(id)
            var proof = record; proof.kind = .context; proof.id = UUID().uuidString; proof.trigger = "send_confirmed"
            proof.detail = L("来源消息已确认发出")
            var e = evidence
            e.field = fields[source] ?? "source-outgoing"
            e.input_value = message.text; e.input_status = "captured"
            e.confirmation = .init(observation: proof.id, messageID: id, text: message.text, basis: "outgoing_message")
            proof.context?.recordEvidence = e
            result.append(proof)
        }
        // Keep only live message identities, with a hard source budget. An
        // adapter must use globally unique IDs; repeated text has distinct IDs.
        if (seen[source]?.count ?? 0) > 4096 { seen[source] = Set(outgoing.compactMap(\.occurrence)) }
        return result
    }
}
