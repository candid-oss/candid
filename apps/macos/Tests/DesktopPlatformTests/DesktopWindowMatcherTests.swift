import CoreGraphics
import DesktopCore
import Foundation
import Testing
@testable import DesktopPlatform

struct DesktopWindowMatcherTests {
    let frame = CGRect(x: -1200, y: 40, width: 1000, height: 700)

    @MainActor @Test func compatibilityOnlyAcceptsExplicitlyAbsentControlFocus() throws {
        func focus(absent: Bool = true, protected: Bool = false, composing: Bool = false) -> DesktopFocusReader.Focus {
            .init(application: .init(bundleID: "test.synthetic.editor", name: "Synthetic", pid: 11),
                  title: "", frame: frame, context: .init(isProtected: protected, isComposing: composing),
                  focusIdentity: 0, windowIdentity: 1, controlFocusAbsent: absent)
        }
        try DesktopCaptureProvider.validateControlFocus(focus(), allowsMissingControlFocus: true, secureInputEnabled: { false })
        #expect(throws: DesktopCaptureError.noVerifiedFocus) {
            try DesktopCaptureProvider.validateControlFocus(focus(), allowsMissingControlFocus: false)
        }
        #expect(throws: DesktopCaptureError.noVerifiedFocus) {
            try DesktopCaptureProvider.validateControlFocus(focus(absent: false), allowsMissingControlFocus: true)
        }
        #expect(throws: DesktopCaptureError.protectedContent) {
            try DesktopCaptureProvider.validateControlFocus(focus(protected: true), allowsMissingControlFocus: true)
        }
        #expect(throws: DesktopCaptureError.protectedContent) {
            try DesktopCaptureProvider.validateControlFocus(focus(), allowsMissingControlFocus: true, secureInputEnabled: { true })
        }
        #expect(throws: DesktopCaptureError.contentChanging) {
            try DesktopCaptureProvider.validateControlFocus(focus(composing: true), allowsMissingControlFocus: true)
        }
    }

    @Test func uniqueWindowMatchesPIDTitleAndBounds() throws {
        let candidates = [
            DesktopWindowCandidate(id: 1, pid: 11, title: "Synthetic document", frame: frame),
            DesktopWindowCandidate(id: 2, pid: 12, title: "Synthetic document", frame: frame),
            DesktopWindowCandidate(id: 3, pid: 11, title: "Different document", frame: frame)
        ]
        #expect(try DesktopWindowMatcher.match(pid: 11, title: "Synthetic document", frame: frame, candidates: candidates) == 1)
    }

    @Test func overlappingSameTitleWindowsFailClosed() {
        let candidates = [1, 2].map { DesktopWindowCandidate(id: UInt32($0), pid: 11, title: "Same", frame: frame) }
        #expect(throws: DesktopCaptureError.ambiguousWindow) {
            try DesktopWindowMatcher.match(pid: 11, title: "Same", frame: frame, candidates: candidates)
        }
    }

    @Test func offscreenWrongPIDAndTitleMismatchAreRejected() {
        let candidates = [
            DesktopWindowCandidate(id: 1, pid: 11, title: "Private", frame: frame, isOnScreen: false),
            DesktopWindowCandidate(id: 2, pid: 12, title: "Private", frame: frame),
            DesktopWindowCandidate(id: 3, pid: 11, title: "Other", frame: frame)
        ]
        #expect(throws: DesktopCaptureError.noMatchingWindow) {
            try DesktopWindowMatcher.match(pid: 11, title: "Private", frame: frame, candidates: candidates)
        }
    }

    @Test func emptyTitleDoesNotResolveAmbiguousGeometry() {
        let candidates = [1, 2].map { DesktopWindowCandidate(id: UInt32($0), pid: 11, title: "", frame: frame) }
        #expect(throws: DesktopCaptureError.ambiguousWindow) {
            try DesktopWindowMatcher.match(pid: 11, title: "", frame: frame, candidates: candidates)
        }
    }

    @Test func toleranceDoesNotCaptureDifferentSizedWindow() {
        let candidate = DesktopWindowCandidate(id: 1, pid: 11, title: "Synthetic", frame: frame.insetBy(dx: 10, dy: 10))
        #expect(throws: DesktopCaptureError.noMatchingWindow) {
            try DesktopWindowMatcher.match(pid: 11, title: "Synthetic", frame: frame, candidates: [candidate])
        }
    }

    @Test func uniqueWindowServerIdentityAcceptsRealGeometryWithDifferentTitles() throws {
        // Synthetic titles preserve the observed failure's byte lengths without
        // copying real window content: AX 112 B, SCK 90 B, zero geometry delta.
        let observedFrame = CGRect(x: 0, y: 39, width: 1028, height: 1227)
        let axTitle = String(repeating: "A", count: 112)
        let sckTitle = String(repeating: "S", count: 90)
        let cg = DesktopWindowCandidate(id: 44, pid: 501, title: "Synthetic CG title", frame: observedFrame)
        let sck = DesktopWindowCandidate(id: 44, pid: 501, title: sckTitle, frame: observedFrame)
        #expect(try DesktopWindowMatcher.resolveWindowServer(pid: 501, frame: observedFrame, candidates: [cg]) == 44)
        #expect(try DesktopWindowMatcher.match(pid: 501, title: axTitle, frame: observedFrame, candidates: [sck], windowServerCandidates: [cg]) == 44)
        #expect(throws: DesktopCaptureError.noMatchingWindow) {
            try DesktopWindowMatcher.match(pid: 501, title: axTitle, frame: observedFrame, candidates: [sck])
        }
    }

    @Test func verifiedWindowIDJoinsDifferentSCKFrameRepresentations() throws {
        let axFrame = CGRect(x: 0, y: 39, width: 1028, height: 1227)
        let cg = DesktopWindowCandidate(id: 44, pid: 501, title: "Synthetic CG", frame: axFrame)
        let sck = DesktopWindowCandidate(id: 44, pid: 501, title: "Synthetic SCK", frame: CGRect(x: 0, y: 0, width: 1028, height: 1266))
        // A different SCK window fits AX title/bounds exactly. It must never
        // replace the ID proved by the unique WindowServer geometry match.
        let unrelated = DesktopWindowCandidate(id: 45, pid: 501, title: "Synthetic AX", frame: axFrame)
        #expect(try DesktopWindowMatcher.match(pid: 501, title: "Synthetic AX", frame: axFrame, candidates: [sck, unrelated], windowServerCandidates: [cg]) == 44)
    }

    @Test func duplicateWindowServerGeometryCannotBeResolvedByUniqueTitle() {
        let first = DesktopWindowCandidate(id: 1, pid: 11, title: "Unique AX title", frame: frame)
        let second = DesktopWindowCandidate(id: 2, pid: 11, title: "Different title", frame: frame)
        #expect(throws: DesktopCaptureError.ambiguousWindow) {
            try DesktopWindowMatcher.resolveWindowServer(pid: 11, frame: frame, candidates: [first, second])
        }
        #expect(throws: DesktopCaptureError.ambiguousWindow) {
            try DesktopWindowMatcher.match(pid: 11, title: "Unique AX title", frame: frame, candidates: [first], windowServerCandidates: [first, second])
        }
    }

    @Test func missingWrongProcessOrOffscreenWindowServerEvidenceRejectsFallback() {
        let sck = DesktopWindowCandidate(id: 1, pid: 11, title: "Synthetic AX", frame: frame)
        let unavailable: [[DesktopWindowCandidate]] = [
            [],
            [DesktopWindowCandidate(id: 1, pid: 12, title: "Synthetic AX", frame: frame)],
            [DesktopWindowCandidate(id: 1, pid: 11, title: "Synthetic AX", frame: frame, isOnScreen: false)],
            [DesktopWindowCandidate(id: 1, pid: 11, title: "Synthetic AX", frame: frame.offsetBy(dx: 20, dy: 0))]
        ]
        for cg in unavailable {
            #expect(throws: DesktopCaptureError.noMatchingWindow) {
                try DesktopWindowMatcher.resolveWindowServer(pid: 11, frame: frame, candidates: cg)
            }
            #expect(throws: DesktopCaptureError.noMatchingWindow) {
                try DesktopWindowMatcher.match(pid: 11, title: "Synthetic AX", frame: frame, candidates: [sck], windowServerCandidates: cg)
            }
        }
    }

    @Test func resolvedWindowServerIDMustExistInSCKForSameVisibleProcess() {
        let cg = DesktopWindowCandidate(id: 44, pid: 11, title: "Synthetic AX", frame: frame)
        let unavailable: [[DesktopWindowCandidate]] = [
            [],
            [DesktopWindowCandidate(id: 45, pid: 11, title: "Synthetic AX", frame: frame)],
            [DesktopWindowCandidate(id: 44, pid: 12, title: "Synthetic AX", frame: frame)],
            [DesktopWindowCandidate(id: 44, pid: 11, title: "Synthetic AX", frame: frame, isOnScreen: false)]
        ]
        for sck in unavailable {
            #expect(throws: DesktopCaptureError.noMatchingWindow) {
                try DesktopWindowMatcher.match(pid: 11, title: "Synthetic AX", frame: frame, candidates: sck, windowServerCandidates: [cg])
            }
        }
    }

    @Test func invalidGeometryCannotProduceWindowTarget() {
        #expect(throws: DesktopCaptureError.noFocusedWindow) {
            try DesktopWindowMatcher.match(pid: 11, title: "", frame: .zero, candidates: [])
        }
    }

    @Test func scaleAndLongSideStayBoundedWithoutUpscaling() {
        let custom = DesktopWindowMatcher.outputSize(width: 2000, height: 1000, scale: 2, maximumEdge: 3840)
        #expect(custom.width == 3840 && custom.height == 1920)
        let invalid = DesktopWindowMatcher.outputSize(width: 100, height: 100, maximumEdge: .nan)
        #expect(invalid.width == 1 && invalid.height == 1)
        let retina = DesktopWindowMatcher.outputSize(width: 2000, height: 1000, scale: 2)
        #expect(retina.width == 1920 && retina.height == 960)
        let small = DesktopWindowMatcher.outputSize(width: 300, height: 200, scale: 1)
        #expect(small.width == 300 && small.height == 200)
        let portrait = DesktopWindowMatcher.outputSize(width: 500, height: 2000, scale: 2)
        #expect(portrait.width == 480 && portrait.height == 1920)
    }
}
