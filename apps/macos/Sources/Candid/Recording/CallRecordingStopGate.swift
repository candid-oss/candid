import Foundation

/// Microphone activity, not silence, defines a call boundary. Brief route changes
/// and mute transitions get a grace period. A failed OS query never means call-ended.
struct CallRecordingStopGate {
    static let grace: TimeInterval = 12
    private var target: String?
    private var absentSince: TimeInterval?

    mutating func shouldStop(target process: CallAudioProcess?, active: [CallAudioProcess]?, now: TimeInterval, callWindowPresent: Bool? = nil) -> Bool {
        let identity = process?.family
        if identity != target { target = identity; absentSince = nil }
        guard let identity else { absentSince = nil; return false }
        if callWindowPresent == true {absentSince = nil;return false}
        if callWindowPresent == nil {
            guard let active else { absentSince = nil; return false }
            if active.contains(where: { $0.family == identity }) { absentSince = nil; return false }
        }
        if absentSince == nil { absentSince = now }
        return now - absentSince! >= Self.grace
    }
}
