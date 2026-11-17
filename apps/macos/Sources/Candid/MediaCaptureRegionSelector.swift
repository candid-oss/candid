import AppKit
import CandidLocalization

/// Selects a rectangle in display-local, top-left-origin points.
@MainActor
final class MediaCaptureRegionSelector: NSObject, NSWindowDelegate {
    private var panel: RegionPanel?
    private var continuation: CheckedContinuation<CGRect?, Never>?

    func select(on screen: NSScreen) async -> CGRect? {
        await withTaskCancellationHandler {
            await withCheckedContinuation { continuation in
                guard !Task.isCancelled else {
                    continuation.resume(returning: nil)
                    return
                }
                self.continuation = continuation
                let frame = screen.frame
                let panel = RegionPanel(contentRect: frame, styleMask: [.borderless],
                                        backing: .buffered, defer: false, screen: screen)
                let view = RegionSelectionView(frame: CGRect(origin: .zero, size: frame.size))
                view.finish = { [weak self] in self?.finish($0) }
                panel.contentView = view
                panel.setFrame(frame, display: false)
                panel.backgroundColor = .clear
                panel.isOpaque = false
                panel.hasShadow = false
                panel.level = .statusBar
                panel.collectionBehavior = [.moveToActiveSpace, .fullScreenAuxiliary]
                panel.hidesOnDeactivate = false
                panel.delegate = self
                self.panel = panel
                panel.makeKeyAndOrderFront(nil)
                panel.makeFirstResponder(view)
            }
        } onCancel: {
            Task { @MainActor in self.finish(nil) }
        }
    }

    func windowDidResignKey(_ notification: Notification) {
        finish(nil)
    }

    private func finish(_ selection: CGRect?) {
        guard let continuation else { return }
        self.continuation = nil
        panel?.delegate = nil
        panel?.orderOut(nil)
        panel = nil
        continuation.resume(returning: selection)
    }
}

private final class RegionPanel: NSPanel {
    override var canBecomeKey: Bool { true }
}

private final class RegionSelectionView: NSView {
    var finish: ((CGRect?) -> Void)?
    private var anchor: CGPoint?
    private var selection: CGRect?

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }

    override func resetCursorRects() {
        addCursorRect(bounds, cursor: .crosshair)
    }

    override func mouseDown(with event: NSEvent) {
        anchor = convert(event.locationInWindow, from: nil)
        selection = nil
        needsDisplay = true
    }

    override func mouseDragged(with event: NSEvent) {
        updateSelection(event)
    }

    override func mouseUp(with event: NSEvent) {
        updateSelection(event)
        anchor = nil
        guard let selection, selection.width >= 4, selection.height >= 4 else {
            selection = nil
            needsDisplay = true
            return
        }
        finish?(selection)
    }

    override func cancelOperation(_ sender: Any?) {
        finish?(nil)
    }

    override func rightMouseDown(with event: NSEvent) {
        finish?(nil)
    }

    override func draw(_ dirtyRect: CGRect) {
        let shade = NSBezierPath(rect: bounds)
        shade.windingRule = .evenOdd
        if let selection, !selection.isEmpty {
            shade.appendRect(selection)
        }
        NSColor.black.withAlphaComponent(0.36).setFill()
        shade.fill()

        if let selection, !selection.isEmpty {
            NSColor.white.setStroke()
            let border = NSBezierPath(rect: selection)
            border.lineWidth = 1
            border.stroke()
        } else {
            let label = NSAttributedString(string: L("拖动选择截图区域，按 Esc 取消"), attributes: [
                .font: NSFont.systemFont(ofSize: 15, weight: .medium),
                .foregroundColor: NSColor.white
            ])
            let size = label.size()
            let point = CGPoint(x: bounds.midX - size.width / 2, y: 36)
            NSColor.black.withAlphaComponent(0.75).setFill()
            NSBezierPath(roundedRect: CGRect(origin: point, size: size).insetBy(dx: -14, dy: -8),
                         xRadius: 8, yRadius: 8).fill()
            label.draw(at: point)
        }
    }

    private func updateSelection(_ event: NSEvent) {
        guard let anchor else { return }
        let point = convert(event.locationInWindow, from: nil)
        let rect = CGRect(x: min(anchor.x, point.x), y: min(anchor.y, point.y),
                          width: abs(point.x - anchor.x), height: abs(point.y - anchor.y))
        selection = rect.integral.intersection(bounds)
        needsDisplay = true
    }
}
