import Foundation
import Testing
@testable import DesktopPlatform

/// Synthetic bundle layouts; only directory names matter.
@Suite struct DesktopChromiumDetectionTests {
    private func bundle(_ paths: [String]) throws -> URL {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("chromium-detect-" + UUID().uuidString)
            .appendingPathComponent("Synthetic.app", isDirectory: true)
        for path in paths {
            try FileManager.default.createDirectory(at: root.appendingPathComponent(path, isDirectory: true), withIntermediateDirectories: true)
        }
        return root
    }

    @Test func recognisesElectronChromeAndCodexLayouts() throws {
        let electron = try bundle(["Contents/Frameworks/Electron Framework.framework", "Contents/Frameworks/Synthetic Helper (GPU).app"])
        let chrome = try bundle(["Contents/Frameworks/Synthetic Framework.framework/Versions/157.0.1/Helpers/Synthetic Helper (Renderer).app"])
        let codex = try bundle(["Contents/Frameworks/Synthetic Framework.framework/Helpers/Synthetic (Renderer).app"])
        for url in [electron, chrome, codex] {
            defer { try? FileManager.default.removeItem(at: url.deletingLastPathComponent()) }
            #expect(DesktopAXWorker.isChromiumBased(bundle: url))
        }
    }

    @Test func nativeAppsAreNotTouched() throws {
        let native = try bundle(["Contents/Frameworks/Sparkle.framework/Versions/B/Updater.app/Contents/Frameworks/Inner (Renderer).app",
                                 "Contents/Frameworks/Mantle.framework/Resources", "Contents/MacOS"])
        defer { try? FileManager.default.removeItem(at: native.deletingLastPathComponent()) }
        // A helper hidden inside another bundled app does not count.
        #expect(!DesktopAXWorker.isChromiumBased(bundle: native))
        #expect(!DesktopAXWorker.isChromiumBased(bundle: URL(fileURLWithPath: "/nonexistent/None.app")))
    }
}
