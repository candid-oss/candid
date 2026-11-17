import Foundation

enum RecordingInput: String, CaseIterable, Identifiable, Codable, Sendable {
    case microphone, systemAndMicrophone
    var id: Self { self }
    var label: String { self == .microphone ? "麦克风" : "系统声音与麦克风" }
    var source: String { self == .microphone ? "mac_microphone" : "mac_system_audio" }
    static var systemAvailable: Bool {
        if #available(macOS 15, *) { return true }
        return false
    }
}
