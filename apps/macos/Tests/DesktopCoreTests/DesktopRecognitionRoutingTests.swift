import Testing
import DesktopCore

struct DesktopRecognitionRoutingTests {
    private typealias Routing = DesktopRecognitionRouting
    private let name = Routing.Field(id: "file", kind: .text, bounds: [100, 100, 500, 150], axRole: "AXCell")
    private let selected = Routing.Field(id: "selected", kind: .selected, bounds: [100, 90, 900, 160], axRole: "AXRow")
    private let date = Routing.Field(id: "date", kind: .text, bounds: [600, 100, 850, 150], axRole: "AXCell")
    private let dateLine = DesktopOCRLine(text: "11 月 10 日", x: 610, y: 108, width: 115, height: 22)

    private func fileTree(selected: Bool? = nil, partial: Bool = false) -> DesktopAXEvidence {
        DesktopAXEvidence(nodes: [
            .init(role: "AXRow", bounds: [100, 90, 900, 160], selected: selected),
            .init(parent: 0, role: "AXCell", label: "实验报告.docx", bounds: [130, 110, 320, 137]),
            .init(role: "AXButton", label: "开通会员", bounds: [720, 20, 950, 80])
        ], partial: partial)
    }

    @Test func allRequiredFieldsHaveDirectEvidence() {
        let result = Routing.decide(.init(inventory: [name, selected], ax: fileTree(selected: false)))
        #expect(result.next == .axOnly)
        #expect(result.fields.map(\.value) == ["实验报告.docx", "false"])
    }

    @Test func missingTextUsesOCRBeforeVisualStateAndNeverIncludesPromotion() {
        let ax = fileTree()
        let first = Routing.decide(.init(inventory: [name, selected, date], ax: ax))
        #expect(first.next == .ocr && first.fields[2].needs == .ocr)
        #expect(first.fields[1].needs == .vlm)
        let second = Routing.decide(.init(inventory: [name, selected, date], ax: ax, ocr: [dateLine]))
        #expect(second.next == .vlm)
        #expect(second.fields[0].source == .ax)
        #expect(second.fields[2].value == "11 月 10 日" && second.fields[2].source == .ocr)
        #expect(!second.fields.contains { $0.value == "开通会员" })
        let complete = Routing.decide(.init(inventory: [name, date], ax: ax, ocr: [dateLine]))
        #expect(complete.next == .done)
    }

    @Test func anUnverifiedInventoryOrTruncatedTreeCannotCertifyCoverage() {
        let ax = fileTree(selected: true)
        #expect(Routing.decide(.init(inventory: nil, ax: ax)).next == .ocr)
        #expect(Routing.decide(.init(inventory: [], ax: ax, ocr: [])).next == .vlm)
        #expect(Routing.decide(.init(inventory: [name], ax: fileTree(partial: true))).next == .ocr)
    }

    @Test func conflictingSourcesAndReusedBoxesCannotFinishRecognition() {
        let otherName = DesktopOCRLine(text: "预算.xlsx", x: 130, y: 110, width: 160, height: 25)
        let conflict = Routing.decide(.init(inventory: [name], ax: fileTree(), ocr: [otherName]))
        #expect(conflict.next == .vlm && conflict.fields[0].gap == .conflictingText)
        let duplicate = Routing.Field(id: "another-file", kind: .text, bounds: name.bounds, axRole: name.axRole)
        let reused = Routing.decide(.init(inventory: [name, duplicate], ax: fileTree(), ocr: [otherName]))
        #expect(reused.next == .vlm && reused.fields.allSatisfy { $0.source == nil })
    }
}
