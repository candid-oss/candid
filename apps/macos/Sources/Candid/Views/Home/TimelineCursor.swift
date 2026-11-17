import CoreGraphics
import Foundation
import Observation

/// Where the home timeline is looking: the playhead and the visible stretch of the lanes. Both change every frame
/// while the user drags, wheels or plays, so they live in their own observable object: only the views that draw
/// them read them, and the page around them (header, stage frame, detail chrome) is not re-evaluated per frame.
@MainActor @Observable final class TimelineCursor {
    var playhead: Date { didSet { latestPlayhead = playhead } }
    var viewport: TimelineViewport
    var viewportReady = false
    /// Hover position over the lanes; read only by the hover line and the hover card.
    var hover: CGPoint? { didSet { latestHover = hover } }
    @ObservationIgnored private(set) var latestHover: CGPoint?
    /// The playhead without registering a dependency, for menu labels and other values read while building a view
    /// that must not redraw when the playhead moves.
    @ObservationIgnored private(set) var latestPlayhead: Date

    init(playhead: Date = Date(), viewport: TimelineViewport = TimelineViewport(start: Date().addingTimeInterval(-9000), secondsPerPoint: 9)) {
        self.playhead = playhead
        self.latestPlayhead = playhead
        self.viewport = viewport
    }
}
