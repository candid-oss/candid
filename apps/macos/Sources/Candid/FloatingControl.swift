import AppKit
import CandidLocalization
import SwiftUI

/// The floating ball, Candid's main way in from any app: a small round button that can be dragged anywhere, docks
/// half-hidden at a screen edge, and opens a small panel. It never takes focus from the app you are in (both windows
/// are non-activating panels) and shows on every desktop and over full-screen apps.
///
/// Click: open or close the panel. Hold: speak (voice input), release to stop. Drag: move; let go near the left or
/// right edge to dock.
@MainActor final class FloatingControl {
    static let shared = FloatingControl()
    static let ballSize: CGFloat = 40
    /// Room around the ball for the badge and the shadow.
    static let margin: CGFloat = 10
    static var windowSize: CGFloat { ballSize + margin * 2 }
    static let panelWidth: CGFloat = 340

    private var ball: NSPanel?
    private var host: BallHostingView?
    private var panel: FloatingKeyPanel?
    private var panelHeight: CGFloat = 200
    private var assistant: FloatingAssistant?
    private var outsideClicks: Any?
    private var hotkeyObserver: NSObjectProtocol?
    private var screenObserver: NSObjectProtocol?
    private var qaObserver: NSObjectProtocol?
    private var docked: Edge?
    private var revealed = false
    enum Edge: String { case left, right }

    var isVisible: Bool { ball?.isVisible == true }

    func setVisible(_ visible: Bool, session: DesktopSession, media: MediaWorkspaceStore) {
        guard visible else { closePanel(); ball?.orderOut(nil); return }
        let assistant = FloatingAssistant.shared(session: session, media: media)
        self.assistant = assistant
        if ball == nil { makeBall(assistant) }
        restorePosition()
        ball?.orderFrontRegardless()
        Task { await assistant.refresh() }
        watchData()
    }

    // MARK: Ball window

    private func makeBall(_ assistant: FloatingAssistant) {
        let size = Self.windowSize
        let window = FloatingBallPanel(contentRect: NSRect(x: 0, y: 0, width: size, height: size),
                                       styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        Self.configure(window)
        let host = BallHostingView(rootView: FloatingBallView(assistant: assistant))
        host.frame = NSRect(x: 0, y: 0, width: size, height: size)
        host.onClick = { [weak self] in self?.togglePanel() }
        host.onHoldBegan = { [weak self] in
            guard let self, let assistant = self.assistant, assistant.voiceEnabled else { return false }
            assistant.beginVoice(); return true
        }
        host.onHoldEnded = { [weak self] in self?.assistant?.endVoice() }
        host.onDrag = { [weak self] origin in self?.move(to: origin) }
        host.onDragEnded = { [weak self] in self?.dragEnded() }
        host.onHover = { [weak self] inside in self?.hover(inside) }
        window.contentView = host
        self.ball = window; self.host = host
        hotkeyObserver = NotificationCenter.default.addObserver(forName: .candidVoiceHotkey, object: nil, queue: .main) { [weak self] note in
            let down = note.object as? Bool ?? false
            MainActor.assumeIsolated {
                guard let self, self.isVisible, let assistant = self.assistant else { return }
                if down { assistant.beginVoice() } else { assistant.endVoice() }
            }
        }
        qaObserver = NotificationCenter.default.addObserver(forName: QASnapshot.command, object: nil, queue: .main) { [weak self] note in
            let line = note.object as? String ?? ""
            MainActor.assumeIsolated { self?.qa(line) }
        }
        screenObserver = NotificationCenter.default.addObserver(forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.restorePosition() }
        }
    }

    static func configure(_ window: NSPanel) {
        window.level = .floating
        window.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        window.isOpaque = false
        window.backgroundColor = .clear
        window.hasShadow = false
        window.hidesOnDeactivate = false
        window.isMovable = false
        window.isReleasedWhenClosed = false
    }

    // MARK: Position and docking

    private func screen(for point: NSPoint) -> NSScreen? {
        NSScreen.screens.first { $0.frame.contains(point) } ?? NSScreen.main ?? NSScreen.screens.first
    }

    private func restorePosition() {
        guard let ball else { return }
        let stored = UserDefaults.standard.string(forKey: FloatingPreferences.positionKey)?.split(separator: ",")
        var origin: NSPoint
        if let stored, stored.count >= 2, let x = Double(stored[0]), let y = Double(stored[1]),
           NSScreen.screens.contains(where: { $0.visibleFrame.insetBy(dx: -Self.windowSize, dy: -Self.windowSize).contains(NSPoint(x: x + Self.windowSize / 2, y: y + Self.windowSize / 2)) }) {
            origin = NSPoint(x: x, y: y)
            docked = stored.count > 2 ? Edge(rawValue: String(stored[2])) : nil
        } else {
            let frame = (NSScreen.main ?? NSScreen.screens.first)?.visibleFrame ?? .zero
            docked = .right
            origin = NSPoint(x: frame.maxX - Self.windowSize, y: frame.minY + frame.height * 0.3)
        }
        ball.setFrameOrigin(origin)
        if docked != nil { applyDock(animated: false) }
    }

    private func savePosition() {
        guard let ball else { return }
        let o = ball.frame.origin
        UserDefaults.standard.set("\(Int(o.x)),\(Int(o.y))" + (docked.map { "," + $0.rawValue } ?? ""), forKey: FloatingPreferences.positionKey)
    }

    private func move(to origin: NSPoint) {
        docked = nil; revealed = false
        ball?.setFrameOrigin(origin)
        if panel?.isVisible == true { placePanel() }
    }

    private func dragEnded() {
        guard let ball, let screen = screen(for: NSPoint(x: ball.frame.midX, y: ball.frame.midY)) else { return }
        let visible = screen.visibleFrame
        var origin = ball.frame.origin
        origin.y = min(max(origin.y, visible.minY), visible.maxY - Self.windowSize)
        let centerX = ball.frame.midX
        if centerX - visible.minX < Self.windowSize { docked = .left } else if visible.maxX - centerX < Self.windowSize { docked = .right } else { docked = nil }
        origin.x = min(max(origin.x, visible.minX - Self.margin), visible.maxX - Self.windowSize + Self.margin)
        ball.setFrameOrigin(origin)
        if docked != nil { applyDock(animated: true) }
        savePosition()
    }

    /// Docked: half the ball past the edge; revealed (hovered, panel open, listening): all of it on screen.
    private func applyDock(animated: Bool) {
        guard let ball, let edge = docked, let screen = screen(for: NSPoint(x: ball.frame.midX, y: ball.frame.midY)) else { return }
        let visible = screen.visibleFrame
        let show = revealed || panel?.isVisible == true || assistant?.voice.isActive == true
        let x: CGFloat
        switch edge {
        case .left: x = show ? visible.minX - Self.margin + 4 : visible.minX - Self.margin - Self.ballSize / 2
        case .right: x = show ? visible.maxX - Self.windowSize + Self.margin - 4 : visible.maxX - Self.windowSize + Self.margin + Self.ballSize / 2
        }
        let target = NSRect(x: x, y: ball.frame.minY, width: ball.frame.width, height: ball.frame.height)
        if animated {
            NSAnimationContext.runAnimationGroup { context in
                context.duration = 0.18
                ball.animator().setFrame(target, display: true)
            }
        } else {
            ball.setFrame(target, display: true)
        }
    }

    private func hover(_ inside: Bool) {
        revealed = inside
        if docked != nil { applyDock(animated: true) }
    }

    // MARK: Panel

    func togglePanel() { panel?.isVisible == true ? closePanel() : openPanel() }

    func openPanel(focus: Bool = false) {
        guard let assistant, isVisible else { return }
        if panel == nil {
            let window = FloatingKeyPanel(contentRect: NSRect(x: 0, y: 0, width: Self.panelWidth, height: panelHeight),
                                          styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
            Self.configure(window)
            window.hasShadow = true
            let hosting = NSHostingView(rootView: FloatingPanelView(assistant: assistant, resized: { [weak self] height in self?.resize(height) }).visualStyleFromSettings())
            window.contentView = hosting
            panel = window
        }
        assistant.expanded = true
        placePanel()
        if panel?.isVisible != true {
            panel?.orderFrontRegardless()
            Task { await assistant.refresh() }
        }
        if focus { panel?.makeKey(); assistant.requestFocus() }
        if docked != nil { applyDock(animated: true) }
        watchOutsideClicks()
    }

    func closePanel() {
        guard panel?.isVisible == true else { return }
        if assistant?.voice.phase == .listening { assistant?.voice.cancel() }
        assistant?.expanded = false
        assistant?.showingSettings = false
        panel?.orderOut(nil)
        if let outsideClicks { NSEvent.removeMonitor(outsideClicks) }
        outsideClicks = nil
        if docked != nil { applyDock(animated: true) }
    }

    private func resize(_ height: CGFloat) {
        let height = ceil(height)
        guard abs(height - panelHeight) > 0.5 else { return }
        panelHeight = height
        if panel?.isVisible == true { placePanel() }
    }

    /// Beside the ball, on the side with more room, top edges aligned and kept on screen.
    private func placePanel() {
        guard let ball, let panel, let screen = screen(for: NSPoint(x: ball.frame.midX, y: ball.frame.midY)) else { return }
        let visible = screen.visibleFrame
        let ballCircle = ball.frame.insetBy(dx: Self.margin, dy: Self.margin)
        let toLeft = ballCircle.midX > visible.midX
        var x = toLeft ? ballCircle.minX - Space.xs - Self.panelWidth : ballCircle.maxX + Space.xs
        x = min(max(x, visible.minX + Space.xs), visible.maxX - Self.panelWidth - Space.xs)
        var top = ballCircle.maxY
        top = min(max(top, visible.minY + panelHeight + Space.xs), visible.maxY - Space.xs)
        panel.setFrame(NSRect(x: x, y: top - panelHeight, width: Self.panelWidth, height: panelHeight), display: true)
    }

    /// A click anywhere outside the ball and the panel closes the panel (as a popover would), except while listening.
    private func watchOutsideClicks() {
        guard outsideClicks == nil else { return }
        outsideClicks = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self, self.assistant?.voice.isActive != true else { return }
                let point = NSEvent.mouseLocation
                if self.panel?.frame.contains(point) == true || self.ball?.frame.contains(point) == true { return }
                self.closePanel()
            }
        }
    }

    // MARK: Badge refresh

    /// The pending count follows the core: re-read when its data changes.
    private func watchData() {
        withObservationTracking { _ = TimelineService.shared.dataVersion } onChange: { [weak self] in
            Task { @MainActor in
                guard let self, self.isVisible else { return }
                await self.assistant?.refresh()
                self.watchData()
            }
        }
    }
}

// MARK: Self-test (QASnapshot)

extension FloatingControl {
    /// `ball open|close|settings|hold|release|continue|draft <text>|ask <text>`.
    func qa(_ line: String) {
        guard line.hasPrefix("ball "), let assistant else { return }
        let command = String(line.dropFirst(5))
        switch command {
        case "open": openPanel()
        case "close": closePanel()
        case "settings": openPanel(); assistant.showingSettings = true
        case "hold": assistant.beginVoice()
        case "release": assistant.endVoice()
        case "continue": assistant.continueInAI()
        default:
            if command.hasPrefix("draft ") { openPanel(); assistant.draft = String(command.dropFirst(6)); assistant.draftFromVoice = true }
            if command.hasPrefix("ask ") { openPanel(); assistant.draft = String(command.dropFirst(4)); Task { await assistant.ask() } }
        }
    }

    /// The ball and the panel as they sit on screen, drawn from the app's own windows (no screen capture permission).
    func snapshot(to url: URL) {
        let windows = [ball, panel].compactMap { $0 }.filter(\.isVisible)
        guard let first = windows.first else { return }
        let union = windows.dropFirst().reduce(first.frame) { $0.union($1.frame) }.insetBy(dx: -12, dy: -12)
        let image = NSImage(size: union.size)
        image.lockFocus()
        NSColor.windowBackgroundColor.setFill()
        NSRect(origin: .zero, size: union.size).fill()
        for window in windows {
            guard let view = window.contentView, let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { continue }
            view.cacheDisplay(in: view.bounds, to: rep)
            rep.draw(in: NSRect(x: window.frame.minX - union.minX, y: window.frame.minY - union.minY, width: window.frame.width, height: window.frame.height))
        }
        image.unlockFocus()
        guard let tiff = image.tiffRepresentation, let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:]) else { return }
        try? png.write(to: url)
    }
}

/// The ball's window never becomes key, so clicking it leaves focus where it was.
final class FloatingBallPanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

/// The panel can take typing without activating Candid.
final class FloatingKeyPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
    override func cancelOperation(_ sender: Any?) { FloatingControl.shared.closePanel() }
}

/// Mouse handling for the ball: click, hold (voice), drag, hover. SwiftUI only draws it.
final class BallHostingView: NSHostingView<FloatingBallView> {
    var onClick: () -> Void = {}
    /// Returns whether holding started something (voice); otherwise the release counts as a click.
    var onHoldBegan: () -> Bool = { false }
    var onHoldEnded: () -> Void = {}
    var onDrag: (NSPoint) -> Void = { _ in }
    var onDragEnded: () -> Void = {}
    var onHover: (Bool) -> Void = { _ in }
    static let holdDelay: Duration = .milliseconds(350)

    private var pressStart: NSPoint?
    private var originStart: NSPoint?
    private var dragging = false
    private var holding = false
    private var holdTask: Task<Void, Never>?

    required init(rootView: FloatingBallView) { super.init(rootView: rootView) }
    @available(*, unavailable) required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.filter { $0.owner === self }.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
    }
    override func mouseEntered(with event: NSEvent) { onHover(true) }
    override func mouseExited(with event: NSEvent) { if pressStart == nil { onHover(false) } }

    override func mouseDown(with event: NSEvent) {
        pressStart = NSEvent.mouseLocation
        originStart = window?.frame.origin
        dragging = false; holding = false
        holdTask?.cancel()
        holdTask = Task { @MainActor [weak self] in
            try? await Task.sleep(for: Self.holdDelay)
            guard let self, !Task.isCancelled, self.pressStart != nil, !self.dragging else { return }
            self.holding = self.onHoldBegan()
        }
    }

    override func mouseDragged(with event: NSEvent) {
        guard let start = pressStart, let origin = originStart, !holding else { return }
        let now = NSEvent.mouseLocation
        if !dragging, hypot(now.x - start.x, now.y - start.y) > 4 { dragging = true; holdTask?.cancel() }
        if dragging { onDrag(NSPoint(x: origin.x + now.x - start.x, y: origin.y + now.y - start.y)) }
    }

    override func mouseUp(with event: NSEvent) {
        holdTask?.cancel()
        defer { pressStart = nil; dragging = false; holding = false }
        if holding { onHoldEnded() } else if dragging { onDragEnded() } else { onClick() }
        if let window, !window.frame.contains(NSEvent.mouseLocation) { onHover(false) }
    }

    override func rightMouseDown(with event: NSEvent) { onClick() }
}
