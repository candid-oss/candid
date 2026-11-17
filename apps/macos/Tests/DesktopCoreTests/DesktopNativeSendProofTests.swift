import Foundation
import Testing
@testable import DesktopCore

struct DesktopNativeSendProofTests {
    let at = Date(timeIntervalSince1970: 1_800_000_000)
    func draft(_ text: String = "原话 👩🏽‍💻\n最后的尾部", session: String = "agent:codex:session-a") -> DesktopRecord {
        var c = DesktopContextSnapshot()
        c.recordEvidence = .init(object: .init(key: session, basis: "url"), field: "window-1:composer", inputValue: text, inputStatus: "captured")
        return .init(timestamp: at, kind: .context, bundleID: "com.openai.codex", context: c)
    }
    func message(_ id: String = "1", session: String = "agent:codex:session-a", text: String = "原话 👩🏽‍💻\n最后的尾部") -> DesktopNativeSendProof.Message {
        .init(session: session, reference: session + "#" + id, at: at.addingTimeInterval(1), text: text)
    }
    @Test func requiresIdentityTimeCompleteExactTextAndUniqueCandidate() throws {
        let d = draft(); let now = at.addingTimeInterval(2)
        let proof = try #require(DesktopNativeSendProof.confirmation(draft: d, messages: [message()], now: now))
        #expect(proof.context?.recordEvidence?.confirmation?.text == d.context?.recordEvidence?.input_value)
        #expect(proof.context?.recordEvidence?.isValid == true)
        #expect(proof.id == DesktopNativeSendProof.confirmation(draft: d, messages: [message()], now: now)?.id)
        #expect(DesktopNativeSendProof.confirmation(draft: d, messages: [message(session: "agent:codex:background")], now: now) == nil)
        #expect(DesktopNativeSendProof.confirmation(draft: d, messages: [message(), message("2")], now: now) == nil)
        #expect(DesktopNativeSendProof.confirmation(draft: d, messages: [message(text: "原话")], now: now) == nil)
        var incomplete = message(); incomplete.complete = false
        #expect(DesktopNativeSendProof.confirmation(draft: d, messages: [incomplete], now: now) == nil)
        var old = message(); old.at = at.addingTimeInterval(-1)
        #expect(DesktopNativeSendProof.confirmation(draft: d, messages: [old], now: now) == nil)
        #expect(DesktopNativeSendProof.confirmation(draft: d, messages: [message()], now: at.addingTimeInterval(31)) == nil)
    }
    @Test func missingIdentityClearNewlineFailureAndRevocationStayUnconfirmed() {
        let now = at.addingTimeInterval(2)
        for trigger in ["mouse", "return", "content"] {
            var d = draft(); d.trigger = trigger
            #expect(DesktopNativeSendProof.confirmation(draft: d, messages: [], now: now) == nil)
            d.context?.recordEvidence?.object.basis = "title"
            #expect(DesktopNativeSendProof.confirmation(draft: d, messages: [message()], now: now) == nil)
        }
        var d = draft(); d.context?.recordEvidence?.input_status = "disabled"
        #expect(DesktopNativeSendProof.confirmation(draft: d, messages: [message()], now: now) == nil)
        #expect(DesktopNativeSendProof.confirmation(draft: draft(""), messages: [message()], now: now) == nil)
        #expect(DesktopNativeSendProof.confirmation(draft: draft("first\nsecond"), messages: [message(text: "first")], now: now) == nil)
    }
    @Test func twoSeparateSameTextSendsKeepDistinctStableIdentities() throws {
        let first = try #require(DesktopNativeSendProof.confirmation(draft: draft(), messages: [message()], now: at.addingTimeInterval(2)))
        var secondDraft = draft(); secondDraft.timestamp = at.addingTimeInterval(3)
        var secondMessage = message("2"); secondMessage.at = at.addingTimeInterval(4)
        let second = try #require(DesktopNativeSendProof.confirmation(draft: secondDraft, messages: [message(), secondMessage], now: at.addingTimeInterval(5)))
        #expect(first.id != second.id)
    }
}
