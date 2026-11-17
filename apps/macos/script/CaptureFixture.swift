import AppKit

/// Synthetic local-only document for explicit capture/AX acceptance. Every field
/// is artificial; the secure field intentionally exercises the exclusion path.
@MainActor final class FixtureDelegate: NSObject, NSApplicationDelegate {
    var window: NSWindow!
    var page = 1
    var titleLabel: NSTextField!
    var textView: NSTextView!
    var secureField: NSSecureTextField!
    var typingTask: Task<Void, Never>?
    func applicationDidFinishLaunching(_ notification: Notification) {
        window = NSWindow(contentRect: NSRect(x: 180, y: 180, width: 1080, height: 690), styleMask: [.titled, .closable, .resizable, .miniaturizable], backing: .buffered, defer: false)
        window.title = "合成验收页面 · 1"
        let stack = NSStackView(); stack.orientation = .vertical; stack.alignment = .leading; stack.spacing = 14; stack.edgeInsets = NSEdgeInsets(top: 32, left: 42, bottom: 32, right: 42)
        titleLabel = NSTextField(labelWithString: "桌面记录器 · 合成验收页面 1")
        titleLabel.font = .systemFont(ofSize: 30, weight: .semibold); stack.addArrangedSubview(titleLabel)
        let sub = NSTextField(wrappingLabelWithString: "此窗口仅含合成内容。可选中或编辑正文，验证焦点、选区、内容变化；安全框应阻止文字和截图。")
        sub.font = .systemFont(ofSize: 17); sub.textColor = .secondaryLabelColor; stack.addArrangedSubview(sub)
        let scroll = NSScrollView(); scroll.hasVerticalScroller = true; scroll.borderType = .bezelBorder
        textView = NSTextView(frame: NSRect(x: 0, y: 0, width: 940, height: 220))
        textView.isEditable = true; textView.isSelectable = true; textView.isRichText = false
        textView.font = .systemFont(ofSize: 18); textView.textContainerInset = NSSize(width: 14, height: 14)
        textView.autoresizingMask = [.width]; textView.textContainer?.widthTracksTextView = true
        textView.setAccessibilityLabel("合成正文，可选中和编辑")
        textView.string = body()
        scroll.documentView = textView
        scroll.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([scroll.widthAnchor.constraint(equalToConstant: 980), scroll.heightAnchor.constraint(equalToConstant: 220)])
        stack.addArrangedSubview(scroll)
        let ordinary = NSTextField(string: "SYNTHETIC INPUT — ordinary one-line value must not be recorded")
        ordinary.setAccessibilityLabel("合成单行输入框，值不应记录"); ordinary.font = .systemFont(ofSize: 15)
        stack.addArrangedSubview(ordinary)
        secureField = NSSecureTextField(string: "SYNTHETIC SECRET MUST NOT BE RECORDED")
        secureField.setAccessibilityLabel("合成安全密码框"); secureField.placeholderString = "仅用于安全焦点拦截验收"
        secureField.widthAnchor.constraint(equalToConstant: 520).isActive = true; stack.addArrangedSubview(secureField)
        let buttons = NSStackView(); buttons.orientation = .horizontal; buttons.spacing = 14
        for (title, action) in [("切换合成页面", #selector(nextPage)), ("选择合成句子", #selector(selectSentence)), ("聚焦安全密码框", #selector(focusSecure)), ("连续输入测试", #selector(continuousTyping))] {
            let button = NSButton(title: title, target: self, action: action); button.bezelStyle = .rounded; buttons.addArrangedSubview(button)
        }
        stack.addArrangedSubview(buttons)
        let note = NSTextField(labelWithString: "没有网络请求 · 没有音频 · 没有真实用户资料")
        note.textColor = .secondaryLabelColor; stack.addArrangedSubview(note)
        window.contentView = stack; window.center(); window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
        window.makeFirstResponder(textView)
    }
    private func body() -> String {
        """
        合成页面 \(page)：09:30 整理事件与屏幕证据的关联。
        10:15 验证本地保存、重启恢复和来源校验。
        11:00 检查资源预算，关闭不必要的后台工作。
        English sample: A reliable memory begins with evidence.
        可选中这段合成文字，验证 selection 事件；也可编辑本正文验证 content 事件。
        """
    }
    @objc func nextPage() {
        typingTask?.cancel(); typingTask = nil
        NSApp.activate(ignoringOtherApps: true); window.makeKeyAndOrderFront(nil)
        page += 1; window.title = "合成验收页面 · \(page)"
        titleLabel.stringValue = "桌面记录器 · 合成验收页面 \(page)"; textView.string = body()
        window.makeFirstResponder(textView); textView.setSelectedRange(NSRange(location: 0, length: 0))
    }
    @objc func selectSentence() {
        window.makeFirstResponder(textView)
        let range = (textView.string as NSString).range(of: "A reliable memory begins with evidence.")
        if range.location != NSNotFound { textView.setSelectedRange(range); textView.scrollRangeToVisible(range) }
    }
    @objc func continuousTyping() {
        typingTask?.cancel()
        window.makeFirstResponder(textView)
        textView.string = body() + "\n连续输入测试（约8秒）："
        textView.didChangeText()
        typingTask = Task { [weak self] in
            guard let self else { return }
            for index in 1...20 {
                do { try await Task.sleep(for: .milliseconds(400)) } catch { return }
                guard !Task.isCancelled else { return }
                self.textView.textStorage?.append(NSAttributedString(string: "合成\(index) "))
                let end = (self.textView.string as NSString).length
                self.textView.setSelectedRange(NSRange(location: end, length: 0))
                self.textView.scrollRangeToVisible(NSRange(location: end, length: 0))
                self.textView.didChangeText()
            }
            self.textView.textStorage?.append(NSAttributedString(string: "\n合成连续输入结束 · FINAL STABLE TEXT"))
            self.textView.didChangeText()
            self.typingTask = nil
        }
    }
    @objc func focusSecure() { window.makeFirstResponder(secureField) }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

MainActor.assumeIsolated {
    let fixtureDelegate = FixtureDelegate()
    let fixtureApplication = NSApplication.shared
    fixtureApplication.setActivationPolicy(.regular)
    fixtureApplication.delegate = fixtureDelegate
    withExtendedLifetime(fixtureDelegate) { fixtureApplication.run() }
}
