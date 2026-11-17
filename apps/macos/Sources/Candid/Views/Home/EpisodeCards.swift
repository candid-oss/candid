import SwiftUI

/// Glyph for library items on cards, the timeline and details.
struct MediaGlyph: View {
    let kind: TimelineEpisode.Kind
    var size: CGFloat = 26

    static func tint(_ kind: TimelineEpisode.Kind) -> Color {
        kind == .image ? Color(red: 0.12, green: 0.68, blue: 0.72) : Color(red: 0.45, green: 0.36, blue: 0.98)
    }

    var body: some View {
        let tint = Self.tint(kind)
        Image(systemName: kind == .image ? "photo" : "waveform")
            .font(.system(size: size * 0.46, weight: .semibold))  // style-exempt: glyph scales with its container
            .foregroundStyle(.white)
            .frame(width: size, height: size)
            .background(RoundedRectangle(cornerRadius: size * 0.23, style: .continuous)
                .fill(LinearGradient(colors: [tint.shiftedHue(0.04), tint], startPoint: .top, endPoint: .bottom)))
            .accessibilityHidden(true)
    }
}

/// A recording as a card: speech drawn as a waveform that fills as the
/// playhead passes, with the line being spoken in large type.
struct AudioEpisodeCard: View {
    let episode: TimelineEpisode
    let playhead: Date
    let focused: Bool

    var body: some View {
        let tint = MediaGlyph.tint(.audio)
        let audio = episode.audio
        let progress = min(1, max(0, playhead.timeIntervalSince(episode.start) / max(1, episode.duration)))
        let spoken = audio?.line(at: playhead) ?? audio?.lines.first.map { ($0, 0) }
        ZStack(alignment: .bottomLeading) {
            LinearGradient(colors: [tint.opacity(0.9), tint.shiftedHue(0.08).opacity(0.75), Color(red: 0.08, green: 0.08, blue: 0.2)],
                           startPoint: .topLeading, endPoint: .bottomTrailing)
            VStack(alignment: .leading, spacing: 18) {
                Canvas { context, size in
                    let bars = Int(size.width / 5)
                    for index in 0..<bars {
                        let fraction = Double(index) / Double(max(1, bars - 1))
                        let moment = episode.start.addingTimeInterval(fraction * episode.duration)
                        let speaking = audio?.speech.contains { $0.contains(moment) } ?? true
                        let h = speaking ? 10 + 34 * abs(sin(Double(index) * 0.61) * cos(Double(index) * 0.23)) : 3
                        let rect = CGRect(x: CGFloat(index) * 5, y: (size.height - h) / 2, width: 2.5, height: h)
                        context.fill(Path(roundedRect: rect, cornerRadius: 1.25),  // style-exempt: timeline content geometry
                                     with: .color(.white.opacity(fraction <= progress ? 0.95 : 0.35)))
                    }
                }
                .frame(height: 56)
                if let spoken {
                    VStack(alignment: .leading, spacing: 6) {
                        if let speaker = spoken.0.speaker { Text(speaker).typography(.secondary, .semibold).opacity(0.7) }
                        Text(spoken.0.text).typography(.section).lineLimit(3)
                            .contentTransition(.opacity)
                    }
                    .animation(.easeOut(duration: 0.25), value: spoken.0.text)
                } else {
                    Text("还没有转写").typography(.section).opacity(0.7)
                }
                Spacer(minLength: 0)
                HStack(alignment: .bottom) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(episode.title).typography(.section).lineLimit(1)
                        Text("\(episode.appName) · \(episode.start.clockText) – \(episode.end.clockText) · \(episode.duration.spokenDuration)")
                            .typography(.secondary).monospacedDigit().opacity(0.8)
                    }
                    Spacer()
                    MediaGlyph(kind: .audio, size: 26)
                }
            }
            .foregroundStyle(.white)
            .padding(18)
        }
        .clipShape(RoundedRectangle(cornerRadius: Radius.card, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: Radius.card, style: .continuous).strokeBorder(Color.white.opacity(0.1), lineWidth: 0.5))
        .shadow(color: .black.opacity(focused ? 0.22 : 0.1), radius: focused ? 30 : 14, y: focused ? 18 : 8)
        .contentShape(RoundedRectangle(cornerRadius: Radius.card, style: .continuous))
    }
}

/// A saved screenshot or imported image as a card.
struct ImageEpisodeCard: View {
    let episode: TimelineEpisode
    let cache: TimelineMediaCache
    let focused: Bool

    var body: some View {
        let image = episode.mediaID.flatMap(cache.mediaImage(id:))
        ZStack(alignment: .bottomLeading) {
            LinearGradient(colors: [MediaGlyph.tint(.image).opacity(0.6), MediaGlyph.tint(.image).opacity(0.2)], startPoint: .topLeading, endPoint: .bottomTrailing)
            if let image {
                Image(nsImage: image).resizable().interpolation(.high).aspectRatio(contentMode: .fit)
                    .padding(20).shadow(color: .black.opacity(0.25), radius: 12, y: 6)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                MediaGlyph(kind: .image, size: 64).frame(maxWidth: .infinity, maxHeight: .infinity).offset(y: -20)
            }
            LinearGradient(stops: [.init(color: .clear, location: 0.55), .init(color: .black.opacity(0.55), location: 1)], startPoint: .top, endPoint: .bottom)
            VStack(alignment: .leading, spacing: 4) {
                Text(episode.title).typography(.section).lineLimit(1)
                Text("\(episode.appName) · \(episode.start.clockText)").typography(.secondary).monospacedDigit().opacity(0.85)
            }
            .foregroundStyle(.white).padding(16)
        }
        .clipShape(RoundedRectangle(cornerRadius: Radius.card, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: Radius.card, style: .continuous).strokeBorder(Color.white.opacity(0.08), lineWidth: 0.5))
        .shadow(color: .black.opacity(focused ? 0.22 : 0.1), radius: focused ? 30 : 14, y: focused ? 18 : 8)
        .contentShape(RoundedRectangle(cornerRadius: Radius.card, style: .continuous))
    }
}
