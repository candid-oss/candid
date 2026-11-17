import Foundation
import CandidShared
import WidgetKit

/// Writes today's aggregates for the widget and asks WidgetKit to reload,
/// only when something visible changed (or every few minutes while live).
@MainActor enum WidgetPublisher {
    private static var last: WidgetSnapshot?

    static func publish(scene: TimelineScene, day: Date, isDemo: Bool, isRecording: Bool, isMicRecording: Bool) {
        guard Calendar.current.isDateInToday(day) else { return }
        let start = Calendar.current.startOfDay(for: day)
        var hours = Array(repeating: 0.0, count: 24)
        for span in scene.activities {
            var cursor = max(span.start, start)
            while cursor < span.end {
                let hour = Int(cursor.timeIntervalSince(start) / 3600)
                guard (0..<24).contains(hour) else { break }
                let boundary = start.addingTimeInterval(Double(hour + 1) * 3600)
                let slice = min(span.end, boundary).timeIntervalSince(cursor)
                hours[hour] += slice / 60
                cursor = min(span.end, boundary)
            }
        }
        let latest = scene.activities.last
        let snapshot = WidgetSnapshot(day: day, isDemo: isDemo, isRecording: isRecording, isMicRecording: isMicRecording,
                                      recordedSeconds: scene.recordedSeconds,
                                      appCount: Set(scene.activities.map(\.bundleID)).count, recordingCount: scene.audio.count,
                                      topApps: scene.appTotals(limit: 3).map { .init(name: $0.appName, bundleID: $0.bundleID, seconds: $0.seconds) },
                                      current: latest.map { .init(title: $0.title, appName: $0.appName, bundleID: $0.bundleID, since: $0.start) },
                                      hours: hours)
        if let last, last.sameContent(as: snapshot), Date().timeIntervalSince(last.generatedAt) < 300 { return }
        do {
            try snapshot.write()
            last = snapshot
            WidgetCenter.shared.reloadAllTimelines()
        } catch {
            // The widget simply keeps its previous state; nothing else depends on this file.
        }
    }

    /// Recording flags change without a timeline rebuild; republish with them.
    static func updateFlags(isRecording: Bool, isMicRecording: Bool) {
        guard var snapshot = last, snapshot.isRecording != isRecording || snapshot.isMicRecording != isMicRecording else { return }
        snapshot.isRecording = isRecording; snapshot.isMicRecording = isMicRecording; snapshot.generatedAt = Date()
        if (try? snapshot.write()) != nil { last = snapshot; WidgetCenter.shared.reloadAllTimelines() }
    }
}
