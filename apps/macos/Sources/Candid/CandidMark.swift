import AppKit

/// Candid's mark: an open "c" with a dot in its opening. The dot is the capture state wherever the mark appears.
enum CandidMark {
    enum State: Equatable {
        /// Screen capture or the microphone is recording: coral dot.
        case recording
        /// Nothing is being recorded: muted dot.
        case paused
        /// Something needs the user (a permission, a failure): hollow ring where the dot sits.
        case attention
    }

    static let coral = NSColor(srgbRed: 1.0, green: 0.42, blue: 0.29, alpha: 1)

    static func state(recordingScreen: Bool, recordingAudio: Bool, alerting: Bool) -> State {
        if recordingAudio || (recordingScreen && !alerting) { return .recording }
        return alerting ? .attention : .paused
    }

    /// Menu bar image (18 pt). Drawn at display time so the arc follows the menu bar's light or dark appearance.
    static func menuBarImage(_ state: State) -> NSImage {
        let size = NSSize(width: 18, height: 18)
        let image = NSImage(size: size, flipped: false) { _ in
            let center = NSPoint(x: 8.2, y: 9), radius = 5.6, gap = 36.0
            let arc = NSBezierPath()
            arc.appendArc(withCenter: center, radius: radius, startAngle: gap, endAngle: 360 - gap, clockwise: false)
            arc.lineWidth = 2.1
            arc.lineCapStyle = .round
            NSColor.labelColor.setStroke()
            arc.stroke()
            let dot = NSRect(x: center.x + radius - 1.2, y: center.y - 2.4, width: 4.8, height: 4.8)
            switch state {
            case .recording:
                coral.setFill(); NSBezierPath(ovalIn: dot).fill()
            case .paused:
                NSColor.secondaryLabelColor.setFill(); NSBezierPath(ovalIn: dot).fill()
            case .attention:
                let ring = NSBezierPath(ovalIn: dot.insetBy(dx: 0.6, dy: 0.6))
                ring.lineWidth = 1.2
                NSColor.labelColor.setStroke(); ring.stroke()
            }
            return true
        }
        image.isTemplate = false
        return image
    }
}
