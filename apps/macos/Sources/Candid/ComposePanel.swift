import AppKit
import ApplicationServices
import CandidClient
import CandidLocalization
import SwiftUI

/// 「帮我写」 (docs/plans/ai-v1-plan.md §4, M3): a shortcut opens a small panel over the app you are in; you say
/// what to write, the core drafts it from that conversation and your own past wording (`/local/compose`), and
/// 「插入」 pastes the draft where your cursor was. Nothing is sent: you read, edit and send it yourself.
///
/// The panel never activates Candid, so the app you were typing in stays frontmost and gets the paste back.
@MainActor final class ComposePanel {
    static let shared = ComposePanel()
    private var panel: NSPanel?
    private let model = ComposeModel()

    func show() {
        model.reset(snapshot: ComposeSnapshot.capture())
        if panel == nil {
            let window = KeyPanel(contentRect: NSRect(x: 0, y: 0, width: 520, height: 260), styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
            window.level = .floating
            window.isMovableByWindowBackground = true
            window.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
            window.isOpaque = false
            window.backgroundColor = .clear
            window.hasShadow = true
            window.contentView = NSHostingView(rootView: ComposeView(model: model, close: { [weak self] in self?.hide() }, insert: { [weak self] text in self?.insert(text) }))
            panel = window
        }
        if let screen = NSScreen.main ?? NSScreen.screens.first, let panel {
            let f = screen.visibleFrame
            panel.setFrameTopLeftPoint(NSPoint(x: f.midX - 260, y: f.maxY - f.height * 0.18))
        }
        panel?.makeKeyAndOrderFront(nil)
    }

    func hide() { panel?.orderOut(nil) }

    /// Puts the draft on the pasteboard, gives focus back and presses ⌘V; the previous pasteboard text returns after.
    private func insert(_ text: String) {
        let board = NSPasteboard.general
        let previous = board.string(forType: .string)
        board.clearContents()
        board.setString(text, forType: .string)
        hide()
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) {
            let source = CGEventSource(stateID: .combinedSessionState)
            for down in [true, false] {
                let event = CGEvent(keyboardEventSource: source, virtualKey: 9, keyDown: down) // V
                event?.flags = .maskCommand
                event?.post(tap: .cghidEventTap)
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) {
                guard let previous, board.string(forType: .string) == text else { return }
                board.clearContents()
                board.setString(previous, forType: .string)
            }
        }
    }
}

/// A borderless panel that can still take typing.
private final class KeyPanel: NSPanel {
    override var canBecomeKey: Bool { true }
}

/// What was on screen when the shortcut was pressed. Read through Accessibility, which capture already uses.
struct ComposeSnapshot {
    var app = ""
    var title = ""
    var selected = ""
    var field = ""

    static func capture() -> ComposeSnapshot {
        var s = ComposeSnapshot()
        guard let front = NSWorkspace.shared.frontmostApplication, front.bundleIdentifier != Bundle.main.bundleIdentifier else { return s }
        s.app = front.localizedName ?? ""
        let app = AXUIElementCreateApplication(front.processIdentifier)
        func attribute(_ element: AXUIElement, _ name: String) -> AnyObject? {
            var value: AnyObject?
            return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
        }
        if let window = attribute(app, kAXFocusedWindowAttribute) {
            s.title = attribute(window as! AXUIElement, kAXTitleAttribute) as? String ?? ""
        }
        if let focused = attribute(app, kAXFocusedUIElementAttribute) {
            let element = focused as! AXUIElement
            s.selected = String((attribute(element, kAXSelectedTextAttribute) as? String ?? "").prefix(4000))
            let role = attribute(element, kAXRoleAttribute) as? String ?? ""
            if [kAXTextAreaRole, kAXTextFieldRole].contains(role) {
                s.field = String((attribute(element, kAXValueAttribute) as? String ?? "").prefix(2000))
            }
        }
        return s
    }
}

@MainActor @Observable final class ComposeModel {
    enum Phase { case asking, drafting, ready, failed }
    var instruction = ""
    var draft = ""
    var context: [String] = []
    var phase = Phase.asking
    var error = ""
    private(set) var snapshot = ComposeSnapshot()
    private var task: Task<Void, Never>?

    func reset(snapshot: ComposeSnapshot) {
        task?.cancel()
        self.snapshot = snapshot
        instruction = ""; draft = ""; context = []; error = ""; phase = .asking
    }

    func submit() {
        let text = instruction.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, phase != .drafting else { return }
        phase = .drafting
        let snapshot = snapshot
        task = Task {
            do {
                try await AIRuntime.shared.syncCore()
                guard let channel = TimelineService.shared.channel else { throw CoreError.unreachable(L("尚未就绪")) }
                let body = try JSONSerialization.data(withJSONObject: ["instruction": text, "app": snapshot.app, "title": snapshot.title, "selected": snapshot.selected, "field": snapshot.field])
                let data = try await channel.send("POST", "/local/compose", body: body, timeout: 150).checked()
                struct Response: Decodable { var draft: String; var context: [Item]; struct Item: Decodable { var label: String } }
                let response = try JSONDecoder().decode(Response.self, from: data)
                guard !Task.isCancelled else { return }
                draft = response.draft
                context = response.context.map(\.label)
                phase = .ready
            } catch {
                guard !Task.isCancelled else { return }
                self.error = L("没写出来：%@", error.localizedDescription)
                phase = .failed
            }
        }
    }
}

private struct ComposeView: View {
    @Bindable var model: ComposeModel
    let close: () -> Void
    let insert: (String) -> Void
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: Space.s) {
            HStack {
                Text(L("帮我写")).typography(.headline)
                if !model.snapshot.app.isEmpty { Text(model.snapshot.app).typography(.secondary).foregroundStyle(.secondary) }
                Spacer()
                Button(action: close) { Image(systemName: "xmark") }.buttonStyle(.plain).keyboardShortcut(.cancelAction)
            }
            TextField(L("要写什么？例如：回他周四三点可以，报价晚点发"), text: $model.instruction, axis: .vertical)
                .textFieldStyle(.plain).typography(.body).focused($focused)
                .onSubmit { model.submit() }
                .disabled(model.phase == .drafting)
            switch model.phase {
            case .asking:
                Text(L("按回车开始。会参考这段对话和你以前的说法，不会替你发送。")).typography(.secondary).foregroundStyle(.secondary)
            case .drafting:
                ProgressView().controlSize(.small)
            case .failed:
                Text(model.error).typography(.secondary).foregroundStyle(.red)
            case .ready:
                if !model.context.isEmpty {
                    Text(L("参考了：%@", model.context.joined(separator: " · "))).typography(.secondary).foregroundStyle(.secondary)
                }
                TextEditor(text: $model.draft).typography(.body).frame(minHeight: 60, maxHeight: 160).scrollContentBackground(.hidden)
                HStack {
                    Spacer()
                    Button(L("复制")) { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(model.draft, forType: .string); close() }
                    Button(L("插入")) { insert(model.draft) }.keyboardShortcut(.defaultAction)
                }
            }
        }
        .padding(Space.m)
        .frame(width: 520)
        .visualStyleFromSettings()
        .onAppear { focused = true }
        .onChange(of: model.phase) { _, phase in if phase == .asking { focused = true } }
    }
}
