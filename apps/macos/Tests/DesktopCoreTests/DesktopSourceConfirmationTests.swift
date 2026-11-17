import Foundation
import Testing
@testable import DesktopCore

struct DesktopSourceConfirmationTests {
    func record(_ ids: [(String, String)], bundle: String = DesktopSourceConfirmation.fixtureBundleID) -> DesktopRecord {
        var e = DesktopRecordEvidence(object: .init(key: "chat", basis: "path"), field: "composer", inputValue: "draft", inputStatus: "captured")
        e.reading = ids.map { .init(text: $0.1, occurrence: "candid.outgoing." + $0.0) }
        var c = DesktopContextSnapshot(); c.recordEvidence = e
        return .init(kind: .context, bundleID: bundle, context: c)
    }
    @Test func confirmsOnlyNewSourceMessagesAndKeepsTwoIdenticalSends() throws {
        var state = DesktopSourceConfirmation()
        #expect(state.confirmations(for: record([("old", "hello")])).isEmpty)
        let first = try #require(state.confirmations(for: record([("old", "hello"), ("1", "same")])).first)
        #expect(first.context?.recordEvidence?.confirmation?.observation == first.id)
        #expect(first.context?.recordEvidence?.confirmation?.text == "same")
        #expect(first.context?.recordEvidence?.isValid == true)
        let next = state.confirmations(for: record([("1", "same"), ("2", "same"), ("3", "short")]))
        #expect(next.count == 2)
        #expect(next[0].context?.recordEvidence?.confirmation?.message_id != first.context?.recordEvidence?.confirmation?.message_id)
        #expect(state.confirmations(for: record([("2", "same"), ("3", "short")])).isEmpty)
        #expect(try DesktopEventMapping.event(for: first).event.source_key == first.id)
    }
    @Test func newlineClearClickFailureAndGenericAXNeverConfirm() {
        var state = DesktopSourceConfirmation()
        for trigger in ["return", "mouse", "content", "scroll"] {
            var r = record([]); r.trigger = trigger
            r.context?.recordEvidence?.input_value = trigger == "content" ? "" : "draft\nnewline"
            #expect(state.confirmations(for: r).isEmpty)
        }
        #expect(state.confirmations(for: record([("1", "text")], bundle: "some.real.app")).isEmpty)
        var revoked = record([("2", "text")]); revoked.context?.recordEvidence?.input_status = "disabled"
        #expect(state.confirmations(for: revoked).isEmpty)
        state.reset()
        #expect(state.confirmations(for: record([("3", "existing after resume")])).isEmpty)
    }
}
