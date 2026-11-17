import AppKit
import CandidLocalization
import SwiftUI

/// The window's overall look. Every style shares the same type, spacing, radii and components;
/// only the backdrop and how panels separate from it change.
enum VisualStyle: String, CaseIterable, Identifiable {
    /// Soft light pools that follow the content, under adjustable frosted glass.
    case ambient
    /// The desktop blurred behind the window, under one even tint.
    case frosted
    /// One flat color; panels lift off it with a soft shadow.
    case flat

    var id: String { rawValue }

    var title: String {
        switch self { case .ambient: L("氛围"); case .frosted: L("毛玻璃"); case .flat: L("纯色") }
    }

    static let key = "home.style"

    /// Reads the stored style; the old 「无」 background (`plain`) becomes the flat style.
    static func stored(_ raw: String, mode: String) -> VisualStyle {
        VisualStyle(rawValue: raw) ?? (mode == "plain" ? .flat : .ambient)
    }
}

private struct VisualStyleKey: EnvironmentKey {
    static let defaultValue = VisualStyle.ambient
}

extension EnvironmentValues {
    var visualStyle: VisualStyle {
        get { self[VisualStyleKey.self] }
        set { self[VisualStyleKey.self] = newValue }
    }
}

extension Palette {
    /// Background of the flat style: a quiet neutral, or the user's color mixed in lightly.
    static func flatBackground(_ tint: Color?) -> Color {
        let mixed = tint.map { NSColor($0) }
        return Color(nsColor: NSColor(name: nil) { appearance in
            let base = appearance.isDark ? NSColor(white: 0.115, alpha: 1) : NSColor(white: 0.955, alpha: 1)
            guard let mixed else { return base }
            return base.blended(withFraction: appearance.isDark ? 0.14 : 0.1, of: mixed) ?? base
        })
    }

    /// A panel raised off a flat background.
    static let raised = Color(nsColor: NSColor(name: nil) { $0.isDark ? NSColor(white: 0.17, alpha: 1) : .white })
}

extension NSAppearance {
    var isDark: Bool { bestMatch(from: [.darkAqua, .aqua]) == .darkAqua }
}

/// The surface behind every page, drawn in the chosen style.
struct WindowBackdrop: View {
    /// What the ambient style drifts toward (the current app on the timeline).
    let tint: Color
    @AppStorage(VisualStyle.key) private var styleRaw = ""
    @AppStorage(BackgroundStyleKey.mode) private var mode = "app"
    @AppStorage(BackgroundStyleKey.styleTint) private var styleTint = ""
    @AppStorage(BackgroundStyleKey.frostTint) private var frostTint = 0.35

    private var chosen: Color? { styleTint.isEmpty ? nil : Color(hex: styleTint) }

    var body: some View {
        Group {
            switch VisualStyle.stored(styleRaw, mode: mode) {
            case .ambient:
                AmbientBackground(tint: tint)
            case .frosted:
                ZStack {
                    BehindWindowBlur()
                    (chosen ?? Color(nsColor: .windowBackgroundColor)).opacity(chosen == nil ? frostTint * 0.6 : frostTint * 0.28)
                }
            case .flat:
                Palette.flatBackground(chosen)
            }
        }
        .ignoresSafeArea()
        .accessibilityHidden(true)
        .animation(Motion.ambient, value: styleRaw)
    }
}

/// The desktop behind the window, blurred by the window server. Stays frosted when the window is inactive.
struct BehindWindowBlur: NSViewRepresentable {
    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = .underWindowBackground
        view.blendingMode = .behindWindow
        view.state = .active
        return view
    }

    func updateNSView(_ view: NSVisualEffectView, context: Context) {}
}

extension View {
    /// Applies the stored style to everything inside (panels, peeks and notices read it).
    func visualStyleFromSettings() -> some View { modifier(StoredVisualStyle()) }
}

private struct StoredVisualStyle: ViewModifier {
    @AppStorage(VisualStyle.key) private var styleRaw = ""
    @AppStorage(BackgroundStyleKey.mode) private var mode = "app"
    func body(content: Content) -> some View {
        content.environment(\.visualStyle, VisualStyle.stored(styleRaw, mode: mode))
    }
}
