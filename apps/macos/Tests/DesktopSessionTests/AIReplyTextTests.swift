import AppKit
import Foundation
import SwiftUI
import Testing
@testable import Candid

/// AI text from a session file shown on the home timeline: compact joining and Markdown blocks. Synthetic text.
@Suite struct AIReplyTextTests {
    @Test func toolOnlyMessagesAndBlankRunsLeaveNoGaps() {
        // A reply split over several assistant messages, some carrying only a tool call (empty or whitespace text).
        let parts = ["\n\n先看一下文件。  \n", "", "  \n\n", "改好了。\r\n\r\n\r\n\r\n下一步：\u{00A0}\n\n\n跑测试。\n\n"]
        #expect(AIReplyText.normalize(parts) == "先看一下文件。\n\n改好了。\n\n下一步：\n\n跑测试。")
        #expect(AIReplyText.normalize(["", " ", "\n"]) == "")
        #expect(!AIReplyText.normalize(parts).contains("\n\n\n"))
    }

    @Test func fencedCodeKeepsItsBlankLinesAndIndentation() {
        let text = "看这段：\n\n```swift\nlet a = 1  \n\n\n    let b = 2\n```\n\n\n\n完成。"
        #expect(AIReplyText.normalize(text) == "看这段：\n\n```swift\nlet a = 1  \n\n\n    let b = 2\n```\n\n完成。")
        #expect(AIReplyText.blocks(text) == [.paragraph("看这段："), .code("let a = 1  \n\n\n    let b = 2"), .paragraph("完成。")])
    }

    @Test func markdownBecomesBlocks() {
        let text = "## 结论\n\n改动有三处：\n\n- 第一 **处**\n- 第二处\n  - 子项\n\n1. 先跑\n2. 再看\n\n> 引用一句\n\n普通段落\n第二行"
        #expect(AIReplyText.blocks(text) == [
            .heading(level: 2, text: "结论"),
            .paragraph("改动有三处："),
            .list(["• 第一 **处**", "• 第二处", "   • 子项"]),
            .list(["1. 先跑", "2. 再看"]),
            .quote("引用一句"),
            .paragraph("普通段落\n第二行"),
        ])
        // Inline Markdown is rendered (no literal asterisks); text that does not parse stays as written.
        #expect(String(AIReplyText.inline("第一 **处**").characters) == "第一 处")
        #expect(String(AIReplyText.inline("#不是标题").characters) == "#不是标题")
        #expect(AIReplyText.blocks("#不是标题") == [.paragraph("#不是标题")])
    }

    /// Offscreen render of a synthetic reply (written to `$CANDID_RENDER_DIR` when set): no tall blank gaps.
    @MainActor @Test func rendersCompactly() throws {
        _ = NSApplication.shared
        let reply = AIReplyText.normalize(["\n\n先看了 `TimelineScrubber.swift`（合成样例）。\n", "", "\n\n\n",
                                           "## 改动\n\n- 播放头单独重绘\n- 舞台按内容跳过\n\n\n\n```swift\nlet cursor = TimelineCursor()\n```\n\n测试 **全部通过**。"])
        let view = NSHostingView(rootView: AIReplyView(text: reply).padding(16).frame(width: 520).background(Color.white))
        view.frame = NSRect(origin: .zero, size: view.fittingSize)
        #expect(view.fittingSize.height < 320, "\(view.fittingSize)")
        guard let dir = ProcessInfo.processInfo.environment["CANDID_RENDER_DIR"] else { return }
        let window = NSWindow(contentRect: view.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = view; view.layoutSubtreeIfNeeded()
        let bitmap = try #require(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        try bitmap.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: dir).appendingPathComponent("ai-reply.png"))
        window.close()
    }
}
