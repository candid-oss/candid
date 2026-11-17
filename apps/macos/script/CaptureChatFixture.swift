import AppKit

/// Local synthetic chat. Every outgoing bubble has a unique AX identifier;
/// draft edits, clearing and failed sends deliberately create no bubble.
@MainActor final class Composer: NSTextView {
    var send: (() -> Void)?
    override func keyDown(with event: NSEvent) {
        if [36, 76].contains(event.keyCode), !event.modifierFlags.contains(.shift) { send?() }
        else { super.keyDown(with: event) }
    }
}
@MainActor final class ChatFixture: NSObject, NSApplicationDelegate {
    var window: NSWindow!
    let composer = Composer(frame: NSRect(x: 0, y: 0, width: 680, height: 90))
    let messages = NSStackView()
    var fail = false
    var task: Task<Void, Never>?
    func applicationDidFinishLaunching(_ notification: Notification) {
        window = NSWindow(contentRect: NSRect(x: 180, y: 200, width: 760, height: 600), styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.title = "合成聊天验收"
        window.representedURL = URL(string: "file:///SYNTHETIC/candid-chat")
        let stack = NSStackView(); stack.orientation = .vertical; stack.alignment = .leading; stack.spacing = 10
        stack.edgeInsets = .init(top: 20, left: 20, bottom: 20, right: 20)
        stack.addArrangedSubview(NSTextField(labelWithString: "仅合成内容 · 无网络请求"))
        messages.orientation = .vertical; messages.alignment = .leading
        let scroll = NSScrollView(); scroll.hasVerticalScroller = true; scroll.documentView = messages
        messages.translatesAutoresizingMaskIntoConstraints = false
        messages.widthAnchor.constraint(equalToConstant: 680).isActive = true
        scroll.heightAnchor.constraint(equalToConstant: 300).isActive = true
        scroll.widthAnchor.constraint(equalToConstant: 700).isActive = true
        stack.addArrangedSubview(scroll)
        composer.isRichText = false; composer.font = .systemFont(ofSize: 16)
        composer.setAccessibilityIdentifier("candid.composer")
        composer.setAccessibilityLabel("合成输入")
        composer.heightAnchor.constraint(equalToConstant: 90).isActive = true
        composer.widthAnchor.constraint(equalToConstant: 680).isActive = true
        composer.send = { [weak self] in self?.send() }
        stack.addArrangedSubview(composer)
        let buttons = NSStackView()
        for (title, action, id) in [("发送", #selector(send), "send"), ("清空", #selector(clear), "clear"), ("发送失败开关", #selector(toggleFailure), "failure"), ("持续接收", #selector(stream), "stream"), ("快速消息", #selector(fast), "fast")] {
            let b = NSButton(title: title, target: self, action: action); b.setAccessibilityIdentifier(id); buttons.addArrangedSubview(b)
        }
        stack.addArrangedSubview(buttons)
        window.contentView = stack; window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
        window.makeFirstResponder(composer)
    }
    @objc func send() {
        guard !fail, !composer.string.isEmpty else { return }
        let message = NSTextField(labelWithString: composer.string)
        message.setAccessibilityIdentifier("candid.outgoing.\(UUID().uuidString)")
        messages.addArrangedSubview(message); clear()
    }
    @objc func clear() { composer.string = ""; composer.didChangeText(); NSAccessibility.post(element: composer, notification: .valueChanged); window.makeFirstResponder(composer) }
    @objc func toggleFailure() { fail.toggle(); window.makeFirstResponder(composer) }
    @objc func fast() {
        task?.cancel()
        task = Task {
            for n in 1...10 {
                let message = NSTextField(labelWithString: n == 5 ? "短" : "合成消息\(n)")
                message.setAccessibilityIdentifier("candid.incoming.fast.\(n)")
                messages.addArrangedSubview(message)
                NSAccessibility.post(element: composer, notification: .valueChanged)
                try? await Task.sleep(for: .milliseconds(100))
            }
        }
        window.makeFirstResponder(composer)
    }
    @objc func stream() {
        task?.cancel()
        task = Task {
            let message = NSTextField(labelWithString: "合成接收：")
            message.setAccessibilityIdentifier("candid.incoming.stream")
            messages.addArrangedSubview(message)
            for n in 1...60 {
                do { try await Task.sleep(for: .milliseconds(400)) } catch { return }
                message.stringValue += " \(n)"
                NSAccessibility.post(element: composer, notification: .valueChanged)
            }
        }
        window.makeFirstResponder(composer)
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}
MainActor.assumeIsolated {
    let delegate = ChatFixture(); let app = NSApplication.shared
    app.setActivationPolicy(.regular); app.delegate = delegate
    withExtendedLifetime(delegate) { app.run() }
}
