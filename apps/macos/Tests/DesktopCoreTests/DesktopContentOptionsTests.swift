import Foundation
import Testing
@testable import DesktopCore

@Suite
struct DesktopContentOptionsTests {
    private func syntheticRecord(protected: Bool = false, composing: Bool = false) -> DesktopRecord {
        DesktopRecord(id: "synthetic-option-record", timestamp: Date(timeIntervalSince1970: 1_000_000),
            kind: .context, appName: "Synthetic Editor", bundleID: "test.synthetic.editor",
            windowTitle: "Synthetic window", detail: "Synthetic observation", parentID: "synthetic-parent",
            runID: "synthetic-run", trigger: "timer", context: .init(
                focusedRole: "AXTextArea", focusedLabel: "Synthetic document",
                selectedText: "Synthetic selection", readableText: "Synthetic visible body", textStatus: "captured",
                isProtected: protected, sessionActive: true, onBattery: false, lowPower: false,
                thermalState: "nominal", idleSeconds: 12, isComposing: composing), activityID: "synthetic-activity")
    }

    @Test func titleAndBodySwitchesOnlyRemoveTheirRequestedFields() throws {
        let original = syntheticRecord()
        var options = DesktopContentOptions(); options.windowTitleEnabled = false
        var expected = original; expected.windowTitle = ""
        #expect(DesktopContentRedaction.apply(original, options: options) == expected)

        options = DesktopContentOptions(); options.readableTextEnabled = false
        expected = original; expected.context?.readableText = nil
        #expect(DesktopContentRedaction.apply(original, options: options) == expected)
        #expect(original.windowTitle == "Synthetic window")
        #expect(original.context?.readableText == "Synthetic visible body")
    }

    @Test func selectionFocusAndSystemSwitchesAreIndependent() {
        let original = syntheticRecord()
        var options = DesktopContentOptions(); options.selectedTextEnabled = false
        var expected = original; expected.context?.selectedText = nil
        #expect(DesktopContentRedaction.apply(original, options: options) == expected)

        options = DesktopContentOptions(); options.focusEnabled = false
        expected = original; expected.context?.focusedRole = nil; expected.context?.focusedLabel = nil
        #expect(DesktopContentRedaction.apply(original, options: options) == expected)

        options = DesktopContentOptions(); options.systemStateEnabled = false
        expected = original; expected.context?.sessionActive = nil; expected.context?.onBattery = nil
        expected.context?.lowPower = nil; expected.context?.thermalState = nil; expected.context?.idleSeconds = nil
        #expect(DesktopContentRedaction.apply(original, options: options) == expected)
        #expect(original.context?.selectedText == "Synthetic selection" && original.context?.focusedRole == "AXTextArea")
        #expect(original.context?.idleSeconds == 12)
    }

    @Test func allContentDisabledPreservesProvenanceProtectionAndCompositionWithoutMutatingSource() throws {
        var options = DesktopContentOptions()
        options.windowTitleEnabled = false; options.readableTextEnabled = false; options.selectedTextEnabled = false
        options.focusEnabled = false; options.systemStateEnabled = false
        for original in [syntheticRecord(protected: true), syntheticRecord(composing: true)] {
            let result = DesktopContentRedaction.apply(original, options: options)
            var expected = original
            expected.windowTitle = ""
            expected.context = .init(textStatus: "captured", isProtected: original.context?.isProtected,
                                     isComposing: original.context?.isComposing)
            #expect(result == expected)
            #expect(result.id == original.id && result.parentID == original.parentID)
            #expect(result.runID == original.runID && result.activityID == original.activityID)
            #expect(original.context?.readableText == "Synthetic visible body" && original.windowTitle == "Synthetic window")
        }
    }

    @Test func disablingBothTextSourcesReportsDisabledAndNeverInventsMissingContext() {
        var options = DesktopContentOptions(); options.readableTextEnabled = false; options.selectedTextEnabled = false
        let original = syntheticRecord()
        let result = DesktopContentRedaction.apply(original, options: options)
        #expect(result.context?.textStatus == "disabled")
        #expect(result.context?.readableText == nil && result.context?.selectedText == nil)
        #expect(result.context?.focusedRole == original.context?.focusedRole)
        var absent = original; absent.context = nil
        #expect(DesktopContentRedaction.apply(absent, options: options) == absent)
    }

    @Test func newPreferencesRoundTripAndOldJSONRetainsAllContentDefaults() throws {
        let old = Data(#"{"screenshotEnabled":true,"textEnabled":true,"allowedBundleIDs":["test.synthetic.editor"],"minimumCaptureInterval":30,"retentionDays":14}"#.utf8)
        let decoded = try JSONDecoder().decode(DesktopPreferences.self, from: old)
        #expect(decoded.contentOptions == DesktopContentOptions())
        #expect(decoded.imageOptions == DesktopImageOptions())
        #expect(decoded.screenshotEnabled && decoded.textEnabled && decoded.retentionDays == 14)
        #expect(decoded.allowedBundleIDs == ["test.synthetic.editor"] && decoded.minimumCaptureInterval == 30)
        var custom = decoded
        custom.contentOptions.focusEnabled = false; custom.contentOptions.selectedTextEnabled = false
        custom.contentOptions.systemStateEnabled = false; custom.contentOptions.windowTitleEnabled = false
        custom.imageOptions.maximumEdge = 2560; custom.imageOptions.jpegQuality = 0.7
        custom.imageOptions.reduceOversizedImages = false
        #expect(try JSONDecoder().decode(DesktopPreferences.self, from: JSONEncoder().encode(custom)) == custom)
    }

    @Test func imageOptionsRejectUnsupportedSizesAndNonFiniteOrOutOfRangeQuality() {
        for edge in [1280, 1920, 2560, 3840] {
            for quality in [0.5, 0.8, 0.95] {
                var options = DesktopImageOptions(); options.maximumEdge = edge; options.jpegQuality = quality
                #expect(options.isValid)
            }
        }
        for edge in [0, -1, 1279, 2000, 3841, Int.max] {
            var options = DesktopImageOptions(); options.maximumEdge = edge
            #expect(!options.isValid)
        }
        for quality in [Double.nan, .infinity, -.infinity, 0, 0.499, 0.951, 1] {
            var options = DesktopImageOptions(); options.jpegQuality = quality
            #expect(!options.isValid)
        }
    }

    @Test func preferencesFilePersistsContentAndImageOptionsAndRefusesInvalidOnes() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("desktop-content-options-synthetic-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let file = DesktopPreferencesFile(directory: root)
        #expect(!file.exists)
        #expect(try file.load() == DesktopPreferences())
        var preferences = DesktopPreferences()
        preferences.contentOptions.readableTextEnabled = false; preferences.contentOptions.focusEnabled = false
        preferences.imageOptions.maximumEdge = 3840; preferences.imageOptions.jpegQuality = 0.95
        preferences.imageOptions.reduceOversizedImages = false
        try file.save(preferences)
        var invalid = preferences; invalid.imageOptions.maximumEdge = 2000
        #expect(throws: DesktopStoreError.self) { try file.save(invalid) }
        invalid = preferences; invalid.imageOptions.jpegQuality = 1
        #expect(throws: DesktopStoreError.self) { try file.save(invalid) }
        invalid = preferences; invalid.retentionDays = 0
        #expect(throws: DesktopStoreError.self) { try file.save(invalid) }
        #expect(try DesktopPreferencesFile(directory: root).load() == preferences)
        // Settings an older version kept in the ledger decode through the same checks.
        let legacy = try JSONEncoder().encode(preferences)
        #expect(try DesktopPreferencesFile.decodeLegacy(legacy) == preferences)
        #expect(throws: (any Error).self) { try DesktopPreferencesFile.decodeLegacy(Data("{\"retentionDays\": 0}".utf8)) }
    }
}
