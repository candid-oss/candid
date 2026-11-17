import AppKit
import DesktopCore
@testable import DesktopPlatform
import Testing

struct DesktopImageRecognitionTests {
    @Test func axPromptPreservesSelectedRowsAndTheirAncestorsWithoutUnverifiedText() {
        let outline = DesktopAXEvidence(nodes: [
            .init(role: "AXList"),
            .init(parent: 0, role: "AXRow", label: "Selected synthetic file", bounds: [10, 20, 300, 50], selected: true),
            .init(parent: 0, role: "AXStaticText", text: "Unverified offscreen text"),
            .init(parent: 0, role: "AXStaticText", text: "Visible by provider", reportedVisible: true)
        ], partial: true)
        let prompt = DesktopMarkdownEvidence(ocr: nil).axPromptLines(outline)
        #expect(prompt.contains("AX1 AXList") && prompt.contains("父节点AX1 AXRow"))
        #expect(prompt.contains("选中 Selected synthetic file"))
        #expect(prompt.contains("Visible by provider"))
        #expect(!prompt.contains("Unverified offscreen text"))
        #expect(prompt.hasPrefix("（部分）"))
    }

    @Test func visionEndpointRejectsExternalHostsAndCredentials() throws {
        for endpoint in [
            "https://api.openai.com/v1",
            "http://127.0.0.1.example.org:1234/v1",
            "http://127.0.0.1@10.0.0.1/v1",
            "http://token@127.0.0.1:1234/v1",
            "http://127.0.0.1:1234/v1?token=secret"
        ] {
            #expect(throws: DesktopRecognitionError.self) { try DesktopVisionClient(endpoint: endpoint) }
        }
        _ = try DesktopVisionClient(endpoint: "http://localhost:1234/")
        _ = try DesktopVisionClient(endpoint: "http://[::1]:1234/v1")
    }

    @Test func markdownKeepsOriginalOCRAndReportsUnmatchedLines() {
        let evidence = DesktopMarkdownEvidence(ocr: DesktopRecognitionResult(source: "synthetic", text: "版本 7319\n未出现在正文\n控制项"))
        let result = evidence.resolve("```markdown\n## 标题\n\n[[1]] · [[99]]\n```")
        #expect(result.body == "## 标题\n\n版本 7319 · ［未找到对应 OCR 行］")
        #expect(result.matchedOCRLineCount == 1)
        #expect(result.unplacedOCR == ["未出现在正文", "控制项"])
    }

    @Test func transcriptDoesNotRepeatAnOCRAnchorAfterItsLiteralText() {
        let evidence = DesktopMarkdownEvidence(ocr: DesktopRecognitionResult(source: "synthetic", text: "先跑一次\n好了\n10:52"))
        let result = evidence.resolve("**我自己**：先跑一次 [[1]]\n**对方**：好了\n10:52")
        #expect(result.body == "**我自己**：先跑一次\n**对方**：好了\n10:52")
        #expect(result.matchedOCRLineCount == 3)
        #expect(result.unplacedOCR.isEmpty)
    }

    @Test @MainActor func systemOCRReadsSyntheticChineseAndEnglish() async throws {
        let image = NSImage(size: NSSize(width: 800, height: 200))
        image.lockFocus()
        NSColor.white.setFill()
        NSBezierPath(rect: NSRect(x: 0, y: 0, width: 800, height: 200)).fill()
        ("发布成功  RELEASE 7319" as NSString).draw(at: NSPoint(x: 36, y: 70), withAttributes: [
            .font: NSFont.systemFont(ofSize: 48), .foregroundColor: NSColor.black
        ])
        image.unlockFocus()
        let tiff = try #require(image.tiffRepresentation)
        let bitmap = try #require(NSBitmapImageRep(data: tiff))
        let jpeg = try #require(bitmap.representation(using: .jpeg, properties: [:]))
        let result = try await DesktopOCR().recognize(jpeg)
        #expect(result.text.contains("发布成功"))
        #expect(result.text.contains("RELEASE"))
        #expect(result.text.contains("7319"))
        #expect(result.ocrLines?.contains { $0.width > 0 && $0.text.contains("7319") } == true)
    }
}
