import Foundation
import Testing
@testable import Candid
@Suite struct RecordingProcessingTests {
    @Test @MainActor func confirmationUsesTheRightClickedItemBeforeClearingIt() {
        var target: String? = "SYNTHETIC-right-clicked"
        var deleted: String?
        confirmThenDismiss(action: { deleted = target }, dismiss: { target = nil })
        #expect(deleted == "SYNTHETIC-right-clicked" && target == nil)
    }
    @Test func waitingAndCancellableProgressAreDistinct() throws {
        let decoder = JSONDecoder(); decoder.keyDecodingStrategy = .convertFromSnakeCase
        let raw = """
        {"state":"queued","kind":"speech.recognize","reason":"waiting_feature_source","created_ms":1,"updated_ms":2,"elapsed_seconds":120}
        """
        let status = try decoder.decode(RecordingProcessing.self, from: Data(raw.utf8))
        #expect(status.isWaiting && status.recoverySettings && status.label.contains("来源"))
        let running = raw.replacingOccurrences(of: "queued", with: "running").replacingOccurrences(of: "\"reason\":\"waiting_feature_source\"", with: "\"reason\":null,\"stage\":\"loading_model\",\"cancellation_supported\":true")
        let model = try decoder.decode(RecordingProcessing.self, from: Data(running.utf8))
        #expect(model.label == "正在加载模型" && model.cancellationSupported == true)
    }
}
