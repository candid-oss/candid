import Foundation
import Testing
@testable import DesktopCore

struct DesktopRecordEvidenceTests {
    @Test func stableObjectsAndExplicitFallback() {
        #expect(DesktopRecordEvidence.object(document: "/doc", url: "https://example.com", app: "app", title: "title", window: "1").basis == "path")
        #expect(DesktopRecordEvidence.object(document: nil, url: "https://example.com", app: "app", title: "title", window: "1").basis == "url")
        #expect(DesktopRecordEvidence.normalizedTitle("(23) Conversation 12:45") == "Conversation")
        #expect(DesktopRecordEvidence.object(document: nil, url: nil, app: "app", title: "", window: "1").basis == "window")
    }
    @Test func oldPreferencesDoNotEnableInputAndRevocationRedactsIt() throws {
        let options = try JSONDecoder().decode(DesktopContentOptions.self, from: Data(#"{"windowTitleEnabled":true,"readableTextEnabled":true,"selectedTextEnabled":true,"focusEnabled":true,"systemStateEnabled":true}"#.utf8))
        #expect(options.inputValueBundleIDs == nil)
        var context = DesktopContextSnapshot()
        context.recordEvidence = .init(object: .init(key: "doc", basis: "path"), field: "composer", inputValue: "private draft", inputStatus: "captured")
        let record = DesktopRecord(kind: .context, bundleID: "app", context: context)
        let redacted = DesktopContentRedaction.apply(record, options: options)
        #expect(redacted.context?.recordEvidence?.input_value == nil)
        #expect(redacted.context?.recordEvidence?.input_status == "disabled")
        var allowed = options; allowed.inputValueBundleIDs = ["app"]
        let event = try DesktopEventMapping.event(for: DesktopContentRedaction.apply(record, options: allowed)).event
        #expect(event.payload["record_evidence"] != nil)
        #expect(event.text.contains { $0.field == "message" && $0.content == "private draft" })
    }
    @Test func unsupportedVersionAndDisabledValueAreRejected() {
        var evidence = DesktopRecordEvidence(object: .init(key: "doc", basis: "path"), field: "f", inputValue: "text", inputStatus: "captured")
        #expect(evidence.isValid)
        evidence.input_status = "disabled"; #expect(!evidence.isValid)
        evidence.input_value = nil; evidence.format = "candid-record-evidence/99"; #expect(!evidence.isValid)
    }
}

extension DesktopRecordEvidenceTests {
    @Test func inputOptInIsIndependentFromBodyAndEmptyValuesRemainExplicit() {
        var options = DesktopContentOptions(); options.readableTextEnabled = false; options.selectedTextEnabled = false
        options.inputValueBundleIDs = ["app"]
        var context = DesktopContextSnapshot(textStatus: "captured")
        context.recordEvidence = .init(object: .init(key: "doc", basis: "path"), field: "composer", inputValue: "", inputStatus: "captured")
        context.recordEvidence?.reading = [.init(text: "body should be removed")]
        let redacted = DesktopContentRedaction.apply(.init(kind: .context, bundleID: "app", context: context), options: options)
        #expect(redacted.context?.recordEvidence?.input_value == "")
        #expect(redacted.context?.recordEvidence?.reading.isEmpty == true)
        #expect(redacted.context?.textStatus == "captured")
    }
}
