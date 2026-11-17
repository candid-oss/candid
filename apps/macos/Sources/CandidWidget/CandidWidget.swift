import AppKit
import CandidShared
import SwiftUI
import WidgetKit

struct DayEntry: TimelineEntry {
    let date: Date
    let snapshot: WidgetSnapshot?
}

struct DayProvider: TimelineProvider {
    func placeholder(in context: Context) -> DayEntry { DayEntry(date: Date(), snapshot: Self.sample) }
    func getSnapshot(in context: Context, completion: @escaping (DayEntry) -> Void) {
        completion(DayEntry(date: Date(), snapshot: WidgetSnapshot.read() ?? (context.isPreview ? Self.sample : nil)))
    }
    func getTimeline(in context: Context, completion: @escaping (Timeline<DayEntry>) -> Void) {
        // The app asks for a reload whenever the day changes; this is only a fallback.
        completion(Timeline(entries: [DayEntry(date: Date(), snapshot: WidgetSnapshot.read())], policy: .after(Date().addingTimeInterval(15 * 60))))
    }

    /// Gallery preview only; clearly synthetic.
    static let sample = WidgetSnapshot(day: Date(), isDemo: true, isRecording: true, isMicRecording: false, recordedSeconds: 4 * 3600 + 60,
                                       appCount: 7, recordingCount: 2,
                                       topApps: [.init(name: "Xcode", bundleID: "com.apple.dt.Xcode", seconds: 8600),
                                                 .init(name: "备忘录", bundleID: "com.apple.Notes", seconds: 3060),
                                                 .init(name: "Safari", bundleID: "com.apple.Safari", seconds: 2900)],
                                       current: .init(title: "合成样例", appName: "Xcode", bundleID: "com.apple.dt.Xcode", since: Date()),
                                       hours: (0..<24).map { (9...17).contains($0) ? Double(20 + ($0 * 7) % 40) : 0 })
}

struct DayWidgetView: View {
    let entry: DayEntry
    @Environment(\.widgetFamily) private var family

    var body: some View {
        Group {
            if let snapshot = entry.snapshot, Calendar.current.isDateInToday(snapshot.day) {
                if family == .systemSmall { small(snapshot) } else { medium(snapshot) }
            } else {
                VStack(alignment: .leading, spacing: 6) {
                    logo
                    Spacer()
                    Text("打开 Candid").font(.system(size: 13, weight: .semibold))
                    Text("今天还没有记录").font(.system(size: 11)).foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            }
        }
        .containerBackground(for: .widget) {
            ZStack {
                LinearGradient(colors: [Color(red: 0.07, green: 0.09, blue: 0.22), Color(red: 0.11, green: 0.16, blue: 0.36)], startPoint: .top, endPoint: .bottom)
                RadialGradient(colors: [Color(red: 0.36, green: 0.42, blue: 1, opacity: 0.6), .clear], center: .topLeading, startRadius: 0, endRadius: 180)
                RadialGradient(colors: [Color(red: 0.15, green: 0.8, blue: 0.85, opacity: 0.35), .clear], center: .bottomTrailing, startRadius: 0, endRadius: 160)
            }
        }
        .foregroundStyle(.white)
    }

    private var logo: some View {
        HStack(spacing: 5) {
            Circle().trim(from: 0, to: 0.8).stroke(Color.white, style: StrokeStyle(lineWidth: 2.2, lineCap: .round))
                .frame(width: 11, height: 11).rotationEffect(.degrees(90))
            Text("Candid").font(.system(size: 11, weight: .semibold)).opacity(0.8)
        }
    }

    private func status(_ s: WidgetSnapshot) -> some View {
        HStack(spacing: 5) {
            Circle().fill(s.isMicRecording || s.isRecording ? Color(red: 1, green: 0.42, blue: 0.38) : Color.white.opacity(0.4)).frame(width: 6, height: 6)
            Text(s.isDemo ? "合成样例" : s.isMicRecording ? "录音中" : s.isRecording ? "记录中" : "已暂停").font(.system(size: 11, weight: .semibold))
        }
    }

    private func small(_ s: WidgetSnapshot) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack { logo; Spacer(); status(s) }
            Spacer(minLength: 2)
            Text(duration(s.recordedSeconds)).font(.system(size: 26, weight: .semibold, design: .rounded)).minimumScaleFactor(0.7).lineLimit(1)
            Text(s.current.map { "\($0.appName) · \($0.title)" } ?? "\(s.appCount) 个应用").font(.system(size: 11)).opacity(0.75).lineLimit(1)
            hours(s).frame(height: 22).padding(.top, 4)
        }
    }

    private func medium(_ s: WidgetSnapshot) -> some View {
        HStack(spacing: 16) {
            small(s)
            VStack(alignment: .leading, spacing: 7) {
                Text("今天最多").font(.system(size: 11, weight: .semibold)).opacity(0.7)
                ForEach(s.topApps.prefix(3), id: \.bundleID) { app in
                    VStack(alignment: .leading, spacing: 3) {
                        HStack {
                            Text(app.name).font(.system(size: 12, weight: .medium)).lineLimit(1)
                            Spacer()
                            Text(duration(app.seconds)).font(.system(size: 11).monospacedDigit()).opacity(0.75)
                        }
                        GeometryReader { geo in
                            Capsule().fill(Color.white.opacity(0.15))
                                .overlay(alignment: .leading) {
                                    Capsule().fill(Color.white.opacity(0.85))
                                        .frame(width: geo.size.width * CGFloat(app.seconds / max(1, s.topApps.first?.seconds ?? 1)))
                                }
                        }
                        .frame(height: 4)
                    }
                }
                Spacer(minLength: 0)
                if s.recordingCount > 0 { Text("\(s.recordingCount) 段录音").font(.system(size: 11)).opacity(0.7) }
            }
        }
    }

    private func hours(_ s: WidgetSnapshot) -> some View {
        let peak = max(1, s.hours.max() ?? 1)
        let now = Calendar.current.component(.hour, from: Date())
        return HStack(alignment: .bottom, spacing: 1.5) {
            ForEach(0..<24, id: \.self) { hour in
                RoundedRectangle(cornerRadius: 1)
                    .fill(hour == now ? Color(red: 1, green: 0.42, blue: 0.38) : Color.white.opacity(s.hours[hour] > 0 ? 0.85 : 0.18))
                    .frame(height: max(2, CGFloat(s.hours[hour] / peak) * 22))
            }
        }
    }

    private func duration(_ seconds: Double) -> String {
        let minutes = Int((seconds / 60).rounded())
        let h = minutes / 60, m = minutes % 60
        return h == 0 ? "\(m) 分钟" : (m == 0 ? "\(h) 小时" : "\(h) 小时 \(m) 分")
    }
}

struct DayWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "CandidDay", provider: DayProvider()) { entry in
            DayWidgetView(entry: entry)
        }
        .configurationDisplayName("今天")
        .description("记录状态、今天的时长和用得最多的应用。")
        .supportedFamilies([.systemSmall, .systemMedium])
    }
}

@main
struct CandidWidgets: WidgetBundle {
    var body: some Widget { DayWidget() }
}
