import AppKit
import CandidLocalization
import SwiftUI

/// A stable identity color per application: the dominant saturated color of
/// its icon, else a hue derived from the bundle ID. Never random per launch.
@MainActor enum AppTint {
    private static var cache: [String: Color] = [:]

    static func color(for bundleID: String) -> Color {
        if let cached = cache[bundleID] { return cached }
        let color = iconColor(bundleID) ?? fallback(bundleID)
        if cache.count > 256 { cache.removeAll() }
        cache[bundleID] = color
        return color
    }

    private static func iconColor(_ bundleID: String) -> Color? {
        guard let icon = DesktopIconCache.image(for: bundleID),
              let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 24, pixelsHigh: 24, bitsPerSample: 8,
                                            samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                                            bytesPerRow: 0, bitsPerPixel: 0) else { return nil }
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
        icon.draw(in: NSRect(x: 0, y: 0, width: 24, height: 24))
        NSGraphicsContext.restoreGraphicsState()
        var red = 0.0, green = 0.0, blue = 0.0, weight = 0.0
        for y in 0..<24 { for x in 0..<24 {
            guard let pixel = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB), pixel.alphaComponent > 0.5 else { continue }
            let saturation = pixel.saturationComponent, brightness = pixel.brightnessComponent
            // Saturated mid-bright pixels carry identity; white glyphs and shadows do not.
            let w = saturation * saturation * (1 - abs(brightness - 0.62)) + 0.002
            red += pixel.redComponent * w; green += pixel.greenComponent * w; blue += pixel.blueComponent * w; weight += w
        } }
        guard weight > 0 else { return nil }
        let average = NSColor(deviceRed: red / weight, green: green / weight, blue: blue / weight, alpha: 1)
        var hue: CGFloat = 0, saturation: CGFloat = 0, brightness: CGFloat = 0, alpha: CGFloat = 0
        average.getHue(&hue, saturation: &saturation, brightness: &brightness, alpha: &alpha)
        if saturation < 0.12 { return Color(white: 0.42) }
        return Color(hue: hue, saturation: min(0.82, max(0.5, saturation * 1.15)), brightness: min(0.92, max(0.62, brightness)))
    }

    private static func fallback(_ bundleID: String) -> Color {
        var hash: UInt64 = 0xcbf29ce484222325
        for byte in bundleID.utf8 { hash = (hash ^ UInt64(byte)) &* 0x100000001b3 }
        return Color(hue: Double(hash % 360) / 360, saturation: 0.58, brightness: 0.82)
    }
}

enum BackgroundStyleKey {
    static let mode = "home.background.mode"          // app | aurora | solid | plain
    static let color = "home.background.color"        // #RRGGBB for solid
    static let intensity = "home.background.intensity" // 0.3...1.8
    static let glass = "home.background.glass"        // 0...1 frosted overlay
    static let flow = "home.background.flow"          // slow drift, off by default
    static let styleTint = "home.style.tint"          // #RRGGBB, or empty for neutral (frosted and flat styles)
    static let frostTint = "home.style.frost"         // 0...1 tint strength over the blurred desktop
}

/// The single continuous surface behind every page: soft light pools under an
/// adjustable frosted layer. Static unless the user turns on slow flow.
struct AmbientBackground: View {
    let tint: Color
    @AppStorage(BackgroundStyleKey.mode) private var mode = "app"
    @AppStorage(BackgroundStyleKey.color) private var hex = "#5B6CFF"
    @AppStorage(BackgroundStyleKey.intensity) private var intensity = 1.0
    @AppStorage(BackgroundStyleKey.glass) private var glass = 0.55
    @AppStorage(BackgroundStyleKey.flow) private var flow = false
    @Environment(\.colorScheme) private var scheme

    private var colors: [Color] {
        switch mode {
        case "aurora": [Color(red: 0.36, green: 0.36, blue: 1.0), Color(red: 0.1, green: 0.78, blue: 0.82), Color(red: 0.86, green: 0.32, blue: 0.78)]
        case "solid": [Color(hex: hex) ?? .accentColor, (Color(hex: hex) ?? .accentColor).shiftedHue(0.06), Color(hex: hex) ?? .accentColor]
        case "plain": []
        default: [tint, tint.shiftedHue(0.09), tint.shiftedHue(-0.07)]
        }
    }

    var body: some View {
        ZStack {
            Color(nsColor: .windowBackgroundColor)
            if !colors.isEmpty {
                if flow {
                    TimelineView(.animation(minimumInterval: 1 / 12)) { context in
                        pools(phase: context.date.timeIntervalSinceReferenceDate / 18)
                    }
                } else {
                    pools(phase: 0)
                }
                if glass > 0 {
                    Rectangle().fill(.ultraThinMaterial).opacity(glass)
                }
            }
        }
        .animation(Motion.ambient, value: tint)
        .animation(Motion.ambient, value: mode)
        .ignoresSafeArea()
        .accessibilityHidden(true)
    }

    private func pools(phase: Double) -> some View {
        GeometryReader { geo in
            let w = geo.size.width, h = geo.size.height
            let strength = (scheme == .dark ? 0.34 : 0.22) * intensity
            let anchors: [(CGFloat, CGFloat, CGFloat)] = [(0.16, 0.12, 0.46), (0.9, 0.78, 0.4), (0.62, 0.2, 0.3)]
            ZStack {
                ForEach(Array(colors.prefix(3).enumerated()), id: \.offset) { index, color in
                    let (ax, ay, radius) = anchors[index]
                    let drift = CGFloat(sin(phase + Double(index) * 2.1)) * 0.05
                    // The gradient fades out exactly at the circle's edge, so no pool ever shows a hard rim;
                    // the stretch keeps the soft oval look.
                    Circle()
                        .fill(RadialGradient(colors: [color.opacity(index == 0 ? strength : strength * 0.7), .clear],
                                             center: .center, startRadius: 0, endRadius: w * radius))
                        .frame(width: w * radius * 2, height: w * radius * 2)
                        .scaleEffect(x: 1.1, y: max(0.6, h / w))
                        .position(x: w * (ax + drift), y: h * (ay - drift * 0.8))
                }
            }
        }
    }
}

extension Color {
    init?(hex: String) {
        var text = hex.trimmingCharacters(in: .whitespaces)
        if text.hasPrefix("#") { text.removeFirst() }
        guard text.count == 6, let value = UInt32(text, radix: 16) else { return nil }
        self.init(red: Double((value >> 16) & 0xff) / 255, green: Double((value >> 8) & 0xff) / 255, blue: Double(value & 0xff) / 255)
    }

    var hexString: String {
        guard let rgb = NSColor(self).usingColorSpace(.deviceRGB) else { return "#5B6CFF" }
        return String(format: "#%02X%02X%02X", Int(rgb.redComponent * 255), Int(rgb.greenComponent * 255), Int(rgb.blueComponent * 255))
    }

    func shiftedHue(_ amount: Double) -> Color {
        guard let rgb = NSColor(self).usingColorSpace(.deviceRGB) else { return self }
        var hue: CGFloat = 0, saturation: CGFloat = 0, brightness: CGFloat = 0, alpha: CGFloat = 0
        rgb.getHue(&hue, saturation: &saturation, brightness: &brightness, alpha: &alpha)
        return Color(hue: (Double(hue) + amount + 1).truncatingRemainder(dividingBy: 1), saturation: Double(saturation), brightness: Double(brightness))
    }
}

extension TimeInterval {
    /// 「1 小时 8 分」 / 「42 分钟」 / 「不到 1 分钟」
    var spokenDuration: String {
        let minutes = Int((self / 60).rounded())
        guard minutes > 0 else { return L("不到 1 分钟") }
        let hours = minutes / 60, rest = minutes % 60
        if hours == 0 { return L("%@ 分钟", rest) }
        return rest == 0 ? L("%@ 小时", hours) : L("%@ 小时 %@ 分", hours, rest)
    }
}

extension Date {
    var clockText: String { formatted(.dateTime.hour(.twoDigits(amPM: .omitted)).minute(.twoDigits)) }
}
