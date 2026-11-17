import ApplicationServices
import DesktopCore
import Foundation
import Testing
@testable import DesktopPlatform

struct DesktopTextPolicyTests {
    @Test func byteBudgetPreservesUnicodeAndNeverSplitsCharacters() {
        let source = String(repeating: "证据🙂e\u{301}", count: 1500)
        for budget in [0, 1, 2, 3, 4, 7, 2048, 4096] {
            let result = DesktopTextPolicy.prefix(source, maxBytes: budget)
            #expect(result.utf8.count <= budget)
            #expect(source.hasPrefix(result))
            #expect(String(data: Data(result.utf8), encoding: .utf8) == result)
        }
    }

    @Test func ASCIIAndExactBoundaryRemainIntact() {
        #expect(DesktopTextPolicy.prefix("SYNTHETIC", maxBytes: 4) == "SYNT")
        #expect(DesktopTextPolicy.prefix("合成", maxBytes: 6) == "合成")
        #expect(DesktopTextPolicy.prefix("合成", maxBytes: 5) == "合")
        #expect(DesktopTextPolicy.prefix("safe", maxBytes: -1).isEmpty)
    }

    @Test func documentRangeRequestsAreBoundedBeforeIPC() {
        #expect(DesktopTextPolicy.boundedUTF16Length(requested: Int.max, maxBytes: 2048) == 512)
        #expect(DesktopTextPolicy.boundedUTF16Length(requested: Int.max, maxBytes: 4096) == 1024)
        #expect(DesktopTextPolicy.boundedUTF16Length(requested: 16, maxBytes: 2048) == 16)
        #expect(DesktopTextPolicy.boundedUTF16Length(requested: -1, maxBytes: 2048) == 0)
        #expect(DesktopTextPolicy.boundedUTF16Length(requested: 16, maxBytes: 0) == 0)
    }

    @Test func passwordsAndOrdinaryInputsCannotEnterValueReadPath() {
        #expect(!DesktopTextPolicy.permitsValue(role: kAXTextFieldRole as String, subrole: nil, protected: false))
        #expect(!DesktopTextPolicy.permitsValue(role: kAXTextAreaRole as String, subrole: kAXSecureTextFieldSubrole as String, protected: false))
        #expect(!DesktopTextPolicy.permitsValue(role: kAXStaticTextRole as String, subrole: nil, protected: true))
        #expect(!DesktopTextPolicy.permitsValue(role: "AXWebArea", subrole: nil, protected: false))
        #expect(DesktopTextPolicy.permitsValue(role: kAXTextAreaRole as String, subrole: nil, protected: false))
        #expect(DesktopTextPolicy.permitsValue(role: kAXStaticTextRole as String, subrole: nil, protected: false))
    }

    @Test func contextEventsKeepTypedPayloadAndOldInitializerCompatibility() {
        let context = DesktopContextSnapshot(focusedRole: "AXTextArea", selectedText: "Synthetic evidence", textStatus: "captured", isProtected: false)
        let event = DesktopSystemEvent(kind: .context, trigger: "selection", context: context)
        #expect(event.kind == .context && event.trigger == "selection")
        #expect(event.context?.selectedText == "Synthetic evidence")
        let legacy = DesktopSystemEvent(kind: .application)
        #expect(legacy.context == nil && legacy.trigger == nil)
    }
}
