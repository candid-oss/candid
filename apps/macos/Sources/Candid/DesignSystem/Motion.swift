import SwiftUI

/// One motion vocabulary for the whole app. Springs are interruptible, so a new gesture always wins over an
/// animation still in flight. Nothing moves linearly except values that track a live signal (levels, playback).
enum Motion {
    /// Page and layout changes: pages, unfolding panels, reflowing lists.
    static let spring = Animation.spring(response: 0.55, dampingFraction: 0.86)
    /// Small state changes: hover, selection, a tag flipping.
    static let snappy = Animation.spring(response: 0.32, dampingFraction: 0.8)
    /// Things the user moved on purpose: switching tabs, a card settling.
    static let bouncy = Animation.spring(response: 0.42, dampingFraction: 0.68)
    /// Follows the pointer: magnification on the timeline.
    static let magnify = Animation.interactiveSpring(response: 0.2, dampingFraction: 0.78)
    /// Background color drifting to the current content.
    static let ambient = Animation.easeInOut(duration: 1.4)

    static let all: [(String, Animation, String)] = [
        ("spring", spring, "页面与布局"), ("snappy", snappy, "悬停、选中、标签"), ("bouncy", bouncy, "切换与落位"),
        ("magnify", magnify, "跟随指针"), ("ambient", ambient, "背景换色")
    ]

    /// Content entering a page: fades in while rising 6 points.
    static var enter: AnyTransition { AnyTransition.opacity.combined(with: .offset(y: 6)) }
    /// A panel unfolding under the row that opened it.
    static var unfold: AnyTransition { .asymmetric(
        insertion: .opacity.combined(with: .offset(y: -6)).combined(with: .scale(scale: 0.985, anchor: .top)),
        removal: .opacity) }
}
