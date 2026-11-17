import AppKit
import SwiftUI

/// Two-finger trackpad swipes over a view, locked to one axis per gesture.
/// Momentum after the fingers lift is ignored so one swipe moves one step.
struct SwipeUpdate: Equatable {
    enum Phase: Equatable { case changed, ended }
    enum Axis: Equatable { case horizontal, vertical }
    var phase: Phase
    var axis: Axis
    var translation: CGFloat
}

/// Pure accumulator, kept separate from AppKit so thresholds are testable.
struct SwipeAccumulator {
    static let lockDistance: CGFloat = 6
    private var dx: CGFloat = 0
    private var dy: CGFloat = 0
    private var axis: SwipeUpdate.Axis?

    mutating func began() { dx = 0; dy = 0; axis = nil }

    mutating func changed(dx deltaX: CGFloat, dy deltaY: CGFloat) -> SwipeUpdate? {
        dx += deltaX; dy += deltaY
        if axis == nil, max(abs(dx), abs(dy)) >= Self.lockDistance {
            axis = abs(dx) >= abs(dy) ? .horizontal : .vertical
        }
        guard let axis else { return nil }
        return SwipeUpdate(phase: .changed, axis: axis, translation: axis == .horizontal ? dx : dy)
    }

    mutating func ended() -> SwipeUpdate? {
        defer { began() }
        guard let axis else { return nil }
        return SwipeUpdate(phase: .ended, axis: axis, translation: axis == .horizontal ? dx : dy)
    }
}

private struct SwipeTrackingModifier: ViewModifier {
    let onUpdate: (SwipeUpdate) -> Void
    @ViewState private var hovering = false
    @ViewState private var monitor: Any?
    @ViewState private var accumulator = SwipeAccumulator()

    func body(content: Content) -> some View {
        content
            .onHover { inside in
                hovering = inside
                inside ? install() : remove()
            }
            .onDisappear(perform: remove)
    }

    private func install() {
        guard monitor == nil else { return }
        monitor = NSEvent.addLocalMonitorForEvents(matching: .scrollWheel) { event in
            // Only trackpad gestures carry phases; momentum is not a new swipe.
            guard event.hasPreciseScrollingDeltas, event.momentumPhase.isEmpty else { return event }
            let began = event.phase.contains(.began), moved = began || event.phase.contains(.changed)
            let finished = event.phase.contains(.ended) || event.phase.contains(.cancelled)
            // Normalize to finger direction whatever the natural-scrolling setting is.
            let sign: CGFloat = event.isDirectionInvertedFromDevice ? 1 : -1
            let dx = event.scrollingDeltaX * sign, dy = event.scrollingDeltaY * sign
            let consumed = MainActor.assumeIsolated { () -> Bool in
                guard hovering else { return false }
                if began { accumulator.began() }
                if moved, let update = accumulator.changed(dx: dx, dy: dy) { onUpdate(update) }
                if finished, let update = accumulator.ended() { onUpdate(update) }
                return true
            }
            return consumed ? nil : event
        }
    }

    private func remove() {
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
    }
}

extension View {
    /// Natural-scrolling sign: fingers moving left give a negative horizontal
    /// translation; fingers moving down give a positive vertical translation.
    func onTrackpadSwipe(_ onUpdate: @escaping (SwipeUpdate) -> Void) -> some View {
        modifier(SwipeTrackingModifier(onUpdate: onUpdate))
    }
}
