import AppKit
import CandidLocalization
import SwiftUI

@MainActor final class FloatingControl {
    static let shared = FloatingControl()
    private var panel: NSPanel?
    func setVisible(_ visible: Bool, session: DesktopSession, media: MediaWorkspaceStore) {
        guard visible else { panel?.orderOut(nil); return }
        if panel == nil {
            let window = NSPanel(contentRect: NSRect(x: 180, y: 180, width: 420, height: 52), styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
            window.level = .floating; window.isMovableByWindowBackground = true
            window.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
            window.isOpaque = false; window.backgroundColor = .clear; window.hasShadow = true
            window.contentView = NSHostingView(rootView: FloatingControlsView(session: session, media: media))
            panel = window
        }
        panel?.orderFront(nil)
    }
}

private struct FloatingControlsView: View {
    let session: DesktopSession
    let media: MediaWorkspaceStore
    var body: some View {
        HStack(spacing: Space.xs) {
            CaptureControls(session: session, media: media) { NotificationCenter.default.post(name: .candidQuickAction, object: "settings") }
            ChromeButton(systemImage: "square.and.pencil", help: L("快速记一笔")) { NotificationCenter.default.post(name: .candidQuickAction, object: QuickAction.note) }
            ChromeButton(systemImage: "magnifyingglass", help: L("搜索")) { NotificationCenter.default.post(name: .candidQuickAction, object: QuickAction.search) }
            ChromeButton(systemImage: "xmark", help: L("隐藏悬浮控件")) { UserDefaults.standard.set(false, forKey: "candid.floating") }
        }.padding(Space.xs).visualStyleFromSettings()
    }
}
