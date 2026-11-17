import SwiftUI
import ConsoleCore

struct WaveformView: View {
    let frames: [LevelFrame]
    let fresh: Bool
    var body: some View {
        Canvas { context, size in
            let center = size.height / 2
            var baseline = Path(); baseline.move(to: CGPoint(x: 0, y: center)); baseline.addLine(to: CGPoint(x: size.width, y: center))
            context.stroke(baseline, with: .color(.secondary.opacity(0.3)), lineWidth: 1)
            guard let last = frames.last else { return }
            let end = last.start_sample + last.buckets.count * 320
            for frame in frames {
                for (index, bucket) in frame.buckets.enumerated() where bucket.count == 2 {
                    let x = size.width * (1 - Double(end - frame.start_sample - index * 320) / 240000)
                    guard x >= 0 else { continue }
                    var path = Path()
                    let high = min(1, Double(bucket[1]) * 8 / 32768)
                    let low = max(-1, Double(bucket[0]) * 8 / 32768)
                    path.move(to: CGPoint(x: x, y: center - high * (center - 8)))
                    path.addLine(to: CGPoint(x: x, y: center - low * (center - 8)))
                    context.stroke(path, with: .color(fresh ? Palette.accent : .secondary), lineWidth: 1.5)
                }
            }
        }
        .background(Surface.wash.color, in: RoundedRectangle(cornerRadius: Radius.medium, style: .continuous))
        .overlay(alignment: .topLeading) {
            Text(frames.isEmpty ? "等待设备实际采样" : fresh ? "真实峰谷 · 最近 15 秒 · 显示增益 8×" : "最后收到的波形 · 当前无新鲜采样")
                .typography(.caption, .regular).foregroundStyle(.secondary).padding(Space.xs)
        }
        .accessibilityLabel(frames.isEmpty ? "暂无波形" : fresh ? "实时设备采样波形" : "已过期的设备波形")
    }
}
