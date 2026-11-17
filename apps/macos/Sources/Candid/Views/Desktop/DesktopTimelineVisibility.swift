import SwiftUI
import AppKit
import OSLog

@MainActor
struct DesktopTimelineVisibility: NSViewRepresentable {
    var onChange: @MainActor (Bool) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(onChange: onChange) }

    func makeNSView(context: Context) -> VisibilityView {
        let view = VisibilityView()
        view.coordinator = context.coordinator
        return view
    }

    func updateNSView(_ nsView: VisibilityView, context: Context) {
        context.coordinator.onChange = onChange
        context.coordinator.observe(nsView.window)
    }

    static func dismantleNSView(_ nsView: VisibilityView, coordinator: Coordinator) {
        nsView.coordinator = nil
        coordinator.stop()
    }

    final class VisibilityView: NSView {
        weak var coordinator: Coordinator?

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            coordinator?.observe(window)
        }
    }

    /// Tracks whether the timeline window can be seen. Occlusion notifications
    /// are not always re-sent after sleep, screen lock or unlock, which used to
    /// leave the timeline paused after opening the lid. Besides window
    /// notifications it re-checks on wake/unlock/activation (now and a moment
    /// later, as occlusion lags), and polls slowly while it believes it is hidden.
    @MainActor final class Coordinator: NSObject {
        private static let logger = Logger(subsystem: "io.github.candid-oss.candid", category: "TimelineVisibility")
        var onChange: @MainActor (Bool) -> Void
        /// Reads the current visibility; replaceable in tests.
        var probe: (@MainActor () -> Bool)?
        private weak var observedWindow: NSWindow?
        private var windowClosing = false
        private var desiredVisibility = false
        private var deliveredVisibility: Bool?
        private var delivery: Task<Void, Never>?
        private var recheck: Task<Void, Never>?
        private var hiddenWatch: Task<Void, Never>?
        private var systemObservers: [(NotificationCenter, NSObjectProtocol)] = []
        let settleDelay: Duration
        let hiddenPollInterval: Duration

        init(onChange: @escaping @MainActor (Bool) -> Void, settleDelay: Duration = .seconds(1), hiddenPollInterval: Duration = .seconds(5)) {
            self.onChange = onChange
            self.settleDelay = settleDelay
            self.hiddenPollInterval = hiddenPollInterval
            super.init()
            observeSystem()
        }

        isolated deinit {
            for (center, token) in systemObservers { center.removeObserver(token) }
            recheck?.cancel(); hiddenWatch?.cancel(); delivery?.cancel()
        }

        func observe(_ window: NSWindow?) {
            if observedWindow !== window {
                NotificationCenter.default.removeObserver(self)
                observedWindow = window
                windowClosing = false
                if let window {
                    for name in [NSWindow.didChangeOcclusionStateNotification,
                                 NSWindow.didMiniaturizeNotification,
                                 NSWindow.didDeminiaturizeNotification,
                                 NSWindow.didBecomeKeyNotification,
                                 NSWindow.didBecomeMainNotification,
                                 NSWindow.didChangeScreenNotification,
                                 NSWindow.willCloseNotification] {
                        NotificationCenter.default.addObserver(self, selector: #selector(windowChanged(_:)), name: name, object: window)
                    }
                }
            }
            refreshVisibility()
        }

        func stop() {
            NotificationCenter.default.removeObserver(self)
            observedWindow = nil
            windowClosing = true
            recheck?.cancel(); hiddenWatch?.cancel()
            publish(false)
        }

        @objc private func windowChanged(_ notification: Notification) {
            if notification.name == NSWindow.willCloseNotification { windowClosing = true }
            // SwiftUI can reopen the same NSWindow and coordinator after closing it.
            if [NSWindow.didBecomeKeyNotification, NSWindow.didBecomeMainNotification,
                NSWindow.didDeminiaturizeNotification].contains(notification.name) { windowClosing = false }
            refreshVisibility()
        }

        private func observeSystem() {
            let workspace = NSWorkspace.shared.notificationCenter
            for name in [NSWorkspace.didWakeNotification, NSWorkspace.screensDidWakeNotification,
                         NSWorkspace.sessionDidBecomeActiveNotification] {
                systemObservers.append((workspace, workspace.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                    MainActor.assumeIsolated { self?.systemChanged() }
                }))
            }
            let app = NotificationCenter.default
            for name in [NSApplication.didBecomeActiveNotification, NSApplication.didUnhideNotification,
                         NSApplication.didChangeScreenParametersNotification] {
                systemObservers.append((app, app.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                    MainActor.assumeIsolated { self?.systemChanged() }
                }))
            }
        }

        /// Wake, unlock or activation: check now, and again once occlusion settles.
        func systemChanged() {
            refreshVisibility()
            recheck?.cancel()
            recheck = Task { @MainActor [weak self, settleDelay] in
                try? await Task.sleep(for: settleDelay)
                guard !Task.isCancelled else { return }
                self?.refreshVisibility()
            }
        }

        func refreshVisibility() {
            if let probe { publish(probe()); return }
            guard let window = observedWindow, !windowClosing else { publish(false); return }
            publish(window.isVisible && !window.isMiniaturized && window.occlusionState.contains(.visible))
        }

        private func publish(_ visible: Bool) {
            desiredVisibility = visible
            watchWhileHidden(!visible && !windowClosing)
            guard delivery == nil, deliveredVisibility != visible else { return }
            // Defer observable state changes until AppKit/SwiftUI has finished its update.
            delivery = Task { @MainActor [self] in
                delivery = nil
                let visible = desiredVisibility
                guard deliveredVisibility != visible else { return }
                deliveredVisibility = visible
                Self.logger.info("Timeline visible: \(visible, privacy: .public)")
                onChange(visible)
            }
        }

        /// Safety net for missed notifications; idle while visible.
        private func watchWhileHidden(_ hidden: Bool) {
            guard hidden else { hiddenWatch?.cancel(); hiddenWatch = nil; return }
            guard hiddenWatch == nil else { return }
            hiddenWatch = Task { @MainActor [weak self, hiddenPollInterval] in
                while !Task.isCancelled {
                    try? await Task.sleep(for: hiddenPollInterval)
                    guard !Task.isCancelled, let self else { return }
                    self.refreshVisibility()
                    if self.desiredVisibility { self.hiddenWatch = nil; return }
                }
            }
        }
    }
}
