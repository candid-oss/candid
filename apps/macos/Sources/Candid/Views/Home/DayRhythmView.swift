import SwiftUI

/// The day as a 24-hour ring: empty night is information too. Hovering an app
/// in the ring or the list isolates it; the hand follows the playhead.
struct DayRhythmView: View {
    let scene: TimelineScene
    let cursor: TimelineCursor
    private var playhead: Date { cursor.playhead }
    @ViewState private var focusBundle: String?
    @ViewState private var drawn = false

    private let diameter: CGFloat = 340
    private let lineWidth: CGFloat = 20

    var body: some View {
        let totals = scene.appTotals()
        HStack(spacing: 72) {
            ring.frame(width: diameter + 80, height: diameter + 80)
            legend(totals).frame(width: 280, alignment: .leading)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear { withAnimation(.easeOut(duration: 1.1)) { drawn = true } }
    }

    private var ring: some View {
        let radius = diameter / 2
        return ZStack {
            Circle().stroke(Surface.hover.color, lineWidth: lineWidth).frame(width: diameter, height: diameter)
            ForEach(scene.activities) { span in
                let on = focusBundle == nil || focusBundle == span.bundleID
                RingArc(start: fraction(span.start), end: fraction(span.end))
                    .trim(from: 0, to: drawn ? 1 : 0)
                    .stroke(AppTint.color(for: span.bundleID), style: StrokeStyle(lineWidth: focusBundle == span.bundleID ? lineWidth + 8 : lineWidth, lineCap: .butt))
                    .opacity(on ? 1 : 0.18)
                    .frame(width: diameter, height: diameter)
                    .contentShape(RingBand(start: fraction(span.start), end: fraction(span.end), width: lineWidth + 6))
                    .onHover { inside in withAnimation(Motion.snappy) { focusBundle = inside ? span.bundleID : nil } }
            }
            ForEach(scene.audio) { audio in
                RingArc(start: fraction(audio.start), end: fraction(audio.end))
                    .stroke(Color.accentColor, style: StrokeStyle(lineWidth: 4, lineCap: .round))
                    .frame(width: diameter - 52, height: diameter - 52)
            }
            Canvas { context, size in
                let c = CGPoint(x: size.width / 2, y: size.height / 2)
                for hour in 0..<24 {
                    let major = hour % 6 == 0
                    let a = Angle.degrees(Double(hour) * 15 - 90).radians
                    let r0 = radius + lineWidth / 2 + 6, r1 = r0 + (major ? 7 : 4)
                    var p = Path(); p.move(to: CGPoint(x: c.x + r0 * cos(a), y: c.y + r0 * sin(a))); p.addLine(to: CGPoint(x: c.x + r1 * cos(a), y: c.y + r1 * sin(a)))
                    context.stroke(p, with: .color(.secondary.opacity(major ? 0.8 : 0.4)), lineWidth: major ? 1.4 : 0.8)
                    if major {
                        let rl = r1 + 12
                        context.draw(Text("\(hour)").typography(.caption).foregroundStyle(.tertiary),
                                     at: CGPoint(x: c.x + rl * cos(a), y: c.y + rl * sin(a)))
                    }
                }
                let a = Angle.degrees(fraction(playhead) * 360 - 90).radians
                var hand = Path()
                hand.move(to: CGPoint(x: c.x + 64 * cos(a), y: c.y + 64 * sin(a)))
                hand.addLine(to: CGPoint(x: c.x + (radius + 12) * cos(a), y: c.y + (radius + 12) * sin(a)))
                context.stroke(hand, with: .color(.primary), style: StrokeStyle(lineWidth: 1.5, lineCap: .round))
                let knob = CGRect(x: c.x + radius * cos(a) - 5, y: c.y + radius * sin(a) - 5, width: 10, height: 10)
                context.fill(Path(ellipseIn: knob), with: .color(Color(nsColor: .controlBackgroundColor)))
                context.stroke(Path(ellipseIn: knob), with: .color(.primary), lineWidth: 1.5)
            }
            .allowsHitTesting(false)
            center
        }
    }

    @ViewBuilder private var center: some View {
        VStack(spacing: 4) {
            if let focusBundle, let total = scene.appTotals(limit: 99).first(where: { $0.bundleID == focusBundle }) {
                Text("\(Int((total.seconds / max(1, scene.recordedSeconds) * 100).rounded()))%")
                    .typography(.display).contentTransition(.numericText())
                Text("\(total.appName) · \(total.seconds.spokenDuration)").typography(.secondary).foregroundStyle(.secondary)
            } else {
                Text(playhead.clockText).typography(.display, digits: true)
                    .contentTransition(.numericText(value: playhead.timeIntervalSince1970))
                Text(scene.activity(at: playhead)?.appName ?? (scene.gap(at: playhead) != nil ? "暂停" : " "))
                    .typography(.secondary).foregroundStyle(.secondary)
            }
        }
        .animation(Motion.snappy, value: focusBundle)
    }

    private func legend(_ totals: [TimelineAppTotal]) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(scene.recordedSeconds.spokenDuration).typography(.title)
            if let longest = scene.longestEpisode {
                Text("最长一段是 \(longest.appName)，\(longest.start.clockText) 起 \(longest.duration.spokenDuration)。")
                    .typography(.body).foregroundStyle(.secondary).padding(.top, 4).padding(.bottom, 18)
            }
            ForEach(totals) { total in
                HStack(spacing: 10) {
                    DesktopApplicationIcon(bundleID: total.bundleID, fallback: "app", size: 22)
                    Text(total.appName).lineLimit(1)
                    Spacer(minLength: 8)
                    Text(total.seconds.spokenDuration).foregroundStyle(.secondary).monospacedDigit()
                }
                .typography(.body)
                .padding(.vertical, 8)
                .opacity(focusBundle == nil || focusBundle == total.bundleID ? 1 : 0.35)
                .contentShape(Rectangle())
                .onHover { inside in withAnimation(Motion.snappy) { focusBundle = inside ? total.bundleID : nil } }
                if total.id != totals.last?.id { HairlineDivider() }
            }
        }
    }

    private func fraction(_ date: Date) -> Double {
        let start = Calendar.current.startOfDay(for: date)
        return date.timeIntervalSince(start) / 86_400
    }
}

/// Clockwise arc between two fractions of a full turn, starting at 12 o'clock.
struct RingArc: Shape {
    var start: Double
    var end: Double
    func path(in rect: CGRect) -> Path {
        var path = Path()
        let gap = min(0.0004, max(0, (end - start) / 4))
        path.addArc(center: CGPoint(x: rect.midX, y: rect.midY), radius: min(rect.width, rect.height) / 2,
                    startAngle: .degrees((start + gap) * 360 - 90), endAngle: .degrees((max(start + gap, end - gap)) * 360 - 90), clockwise: false)
        return path
    }
}

/// Hit area for a ring segment: only the band itself, never the whole square.
struct RingBand: Shape {
    var start: Double
    var end: Double
    var width: CGFloat
    func path(in rect: CGRect) -> Path {
        RingArc(start: start, end: end).path(in: rect).strokedPath(StrokeStyle(lineWidth: width))
    }
}
