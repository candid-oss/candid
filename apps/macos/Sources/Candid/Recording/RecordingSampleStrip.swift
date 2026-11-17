import SwiftUI
import CandidLocalization

/// A sample coverage strip, not an invented waveform. Empty spans are gaps.
struct RecordingSampleStrip: View {
    let parts: [MediaLibraryRecord.RecordingPart]
    let gaps: [MediaLibraryRecord.Gap]
    let duration: Double
    let position: Double
    var peaks: [Float] = []
    let seek: (Double) -> Void

    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                Rectangle().fill(.quaternary)
                ForEach(parts, id: \.id) { part in
                    let start = Double(part.sampleStart) / Double(part.sampleRate)
                    let length = Double(part.sampleCount) / Double(part.sampleRate)
                    Rectangle().fill(.tint.opacity(0.55))
                        .frame(width: geometry.size.width * length / max(1, duration))
                        .offset(x: geometry.size.width * start / max(1, duration))
                }
                Canvas { context, size in
                    for (index, peak) in peaks.enumerated() {
                        let width: CGFloat = size.width / CGFloat(max(1, peaks.count))
                        let height: CGFloat = max(1, CGFloat(peak) * size.height)
                        let y: CGFloat = (size.height - height) / 2
                        let rect = CGRect(x: CGFloat(index) * width, y: y, width: max(1, width - 1), height: height)
                        context.fill(Path(rect), with: .foreground)
                    }
                }
                Rectangle().fill(.primary).frame(width: 1)
                    .offset(x: geometry.size.width * min(position, duration) / max(1, duration))
            }
            .contentShape(Rectangle())
            .gesture(DragGesture(minimumDistance: 0).onEnded { value in
                seek(max(0, min(duration, value.location.x / max(1, geometry.size.width) * duration)))
            })
        }
        .frame(height: 44)
        .accessibilityLabel(gaps.isEmpty ? L("完整录音") : L("录音有 %@ 处缺口", gaps.count))
        .accessibilityAdjustableAction { direction in
            seek(max(0, min(duration, position + (direction == .increment ? 5 : -5))))
        }
    }
}
