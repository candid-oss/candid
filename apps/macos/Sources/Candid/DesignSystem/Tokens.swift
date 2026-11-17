import SwiftUI

// The single source for Candid's visual values. Pages compose components and use these names; they do not
// pick their own sizes, radii or tints. Rules and rationale live in docs/design-system.md; values live here only.

/// Seven text levels. Chinese renders in PingFang through the system font, so sizes are whole points.
enum TypeScale: CaseIterable {
    /// Clock and large numbers.
    case display
    /// The one title of a page.
    case title
    /// A section inside a page.
    case section
    /// Row titles, navigation items, card titles.
    case headline
    /// Reading text, fields, buttons.
    case body
    /// Row details, help next to a decision, metadata.
    case secondary
    /// Tags, counts, axis labels.
    case caption

    var size: CGFloat {
        switch self {
        case .display: 34
        case .title: 26
        case .section: 17
        case .headline: 14
        case .body: 13
        case .secondary: 12
        case .caption: 11
        }
    }

    var weight: Font.Weight {
        switch self {
        case .display, .title, .section, .headline: .semibold
        case .body, .secondary: .regular
        case .caption: .medium
        }
    }

    func font(_ weight: Font.Weight? = nil, design: Font.Design = .default) -> Font {
        .system(size: size, weight: weight ?? self.weight, design: design)
    }
}

extension View {
    /// `Text("…").typography(.secondary)`; numbers that change in place pass `digits: true`.
    func typography(_ scale: TypeScale, _ weight: Font.Weight? = nil, digits: Bool = false) -> some View {
        font(digits ? scale.font(weight).monospacedDigit() : scale.font(weight))
    }
}

extension Text {
    /// Same as the view modifier, but stays a `Text` so it can be concatenated or drawn in a `Canvas`.
    func typography(_ scale: TypeScale, _ weight: Font.Weight? = nil, digits: Bool = false) -> Text {
        font(digits ? scale.font(weight).monospacedDigit() : scale.font(weight))
    }
}

/// Spacing on a 4-point grid.
enum Space {
    static let xxs: CGFloat = 4
    static let xs: CGFloat = 8
    static let s: CGFloat = 12
    static let m: CGFloat = 16
    static let l: CGFloat = 24
    static let xl: CGFloat = 32
    static let xxl: CGFloat = 48
    /// Left and right inset of page content.
    static let page: CGFloat = 28
    /// Vertical padding of a list row.
    static let row: CGFloat = 12
    static let all: [(String, CGFloat)] = [("xxs", xxs), ("xs", xs), ("s", s), ("m", m), ("l", l), ("xl", xl), ("xxl", xxl)]
}

/// Four corner radii, always continuous; anything round-ended is a `Capsule`.
enum Radius {
    /// Fields, thumbnails inside rows.
    static let small: CGFloat = 8
    /// Tiles, editors, selectable choices.
    static let medium: CGFloat = 12
    /// Inline panels and floating peeks.
    static let large: CGFloat = 16
    /// Media cards and large previews.
    static let card: CGFloat = 22
    static let all: [(String, CGFloat)] = [("small", small), ("medium", medium), ("large", large), ("card", card)]
}

/// Heights of interactive things, so rows line up.
enum ControlHeight {
    /// Tags and small chips.
    static let chip: CGFloat = 22
    /// Choices, text buttons, primary buttons.
    static let button: CGFloat = 26
    /// Fields and round chrome buttons.
    static let field: CGFloat = 30
}

/// Depth by tint, never by border: every level is the primary color at a low opacity over the page background.
enum Surface: CaseIterable {
    /// Resting panels that unfold in place.
    case wash
    /// Fields and idle choices.
    case fill
    /// Hover.
    case hover
    /// Selected or pressed.
    case selected

    var opacity: Double {
        switch self {
        case .wash: 0.035
        case .fill: 0.05
        case .hover: 0.07
        case .selected: 0.1
        }
    }

    var color: Color { Color.primary.opacity(opacity) }

    static let hairline = Color.primary.opacity(0.08)
    static let hairlineWidth: CGFloat = 0.5
}

/// Colors with a meaning. Decoration comes from the ambient background and app tints, not from here.
enum Palette {
    /// Interaction color: follows the system accent (user decision 2026-11-10).
    static let accent = Color.accentColor
    static let success = Color.green
    static let warning = Color.orange
    static let danger = Color.red
    /// Live microphone or screen capture.
    static let recording = Color.red
    /// Marks AI-made content and the AI page; never used for ordinary controls.
    static let ai = LinearGradient(colors: [Color(red: 0.42, green: 0.4, blue: 1), Color(red: 0.86, green: 0.36, blue: 0.86),
                                            Color(red: 1, green: 0.55, blue: 0.4)], startPoint: .leading, endPoint: .trailing)
}
