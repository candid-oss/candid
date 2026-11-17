import Foundation
import os

/// Home timeline diagnostics: signpost intervals for Instruments (Points of Interest) and cheap counters that the
/// performance test reads. Counters only count; nothing here changes what is drawn.
enum TimelinePerf {
    static let signposter = OSSignposter(subsystem: "io.github.candid-oss.candid", category: .pointsOfInterest)

    enum Counter: String, CaseIterable, Sendable {
        /// Body evaluations of the home page, the stage above the lanes, the lanes, the day header.
        case homeBody, stageBody, scrubberBody, headerBody
        /// Scene builds that ran, and ones whose result equalled the scene already shown.
        case sceneBuild, sceneUnchanged
        /// Image decodes finished by the thumbnail cache.
        case decode
    }

    @MainActor private static var counts: [Counter: Int] = [:]

    @MainActor static func tick(_ counter: Counter) { counts[counter, default: 0] += 1 }
    @MainActor static func count(_ counter: Counter) -> Int { counts[counter] ?? 0 }
    @MainActor static func reset() { counts.removeAll() }
    @MainActor static var snapshot: [Counter: Int] { counts }
}
