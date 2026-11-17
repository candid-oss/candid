import DesktopCore
import Foundation
import Testing
@testable import DesktopPlatform

@Suite
struct DesktopApplicationCatalogTests {
    private func root() throws -> URL {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("synthetic-app-catalog-" + UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        return root
    }

    @discardableResult
    private func app(_ path: String, under root: URL, id: String, name: String = "Synthetic app", extra: [String: Any] = [:]) throws -> URL {
        let url = root.appendingPathComponent(path, isDirectory: true)
        let contents = url.appendingPathComponent("Contents", isDirectory: true)
        try FileManager.default.createDirectory(at: contents, withIntermediateDirectories: true)
        var info: [String: Any] = ["CFBundleIdentifier": id, "CFBundleName": name, "CFBundleDisplayName": name, "CFBundlePackageType": "APPL", "CFBundleExecutable": "SyntheticExecutable"]
        info.merge(extra) { _, new in new }
        let bytes = try PropertyListSerialization.data(fromPropertyList: info, format: .xml, options: 0)
        try bytes.write(to: contents.appendingPathComponent("Info.plist"))
        return url
    }

    @Test func installedAppsNeedNotBeRunningAndBundleIdentifiersDeduplicateCopies() throws {
        let root = try root(); defer { try? FileManager.default.removeItem(at: root) }
        try app("Synthetic.app", under: root, id: "test.synthetic.main", name: "Synthetic Main")
        try app("Utilities/AnotherCopy.app", under: root, id: "test.synthetic.main", name: "Synthetic Main")
        try app("NotRunning.app", under: root, id: "test.synthetic.not-running", name: "Synthetic Not Running")
        let result = DesktopApplicationCatalog.scan(roots: [root, root])
        #expect(Set(result.map(\.bundleID)) == ["test.synthetic.main", "test.synthetic.not-running"])
        #expect(result.count == 2 && result.allSatisfy { $0.pid == 0 && $0.bundlePath != nil })
    }

    @Test func helperWithSameDisplayNameDoesNotCreateSecondWechat() throws {
        let root = try root(); defer { try? FileManager.default.removeItem(at: root) }
        let main = try app("WeChat.app", under: root, id: "test.synthetic.wechat", name: "微信")
        let nested = try app("WeChat.app/Contents/MacOS/WeChatAppEx.app", under: root, id: "test.synthetic.wechat-helper", name: "微信", extra: ["LSUIElement": 1])
        let nestedWithoutFlag = try app("WeChat.app/Contents/PlainHelper.app", under: root, id: "test.synthetic.nested", name: "微信")
        let background = try app("Background.app", under: root, id: "test.synthetic.background", extra: ["LSBackgroundOnly": true])
        let extensionApp = try app("Extension.app", under: root, id: "test.synthetic.extension", extra: ["NSExtension": ["NSExtensionPointIdentifier": "test.synthetic"]])
        #expect(DesktopApplicationCatalog.isUserApplication(url: main))
        for helper in [nested, nestedWithoutFlag, background, extensionApp] {
            #expect(!DesktopApplicationCatalog.isUserApplication(url: helper))
        }
        #expect(DesktopApplicationCatalog.scan(roots: [root]).map(\.bundleID) == ["test.synthetic.wechat"])
    }

    @Test func standaloneMenuBarApplicationRemainsSelectable() throws {
        let root = try root(); defer { try? FileManager.default.removeItem(at: root) }
        let menuBar = try app("MenuBar.app", under: root, id: "test.synthetic.menu-bar", extra: ["LSUIElement": "1"])
        #expect(DesktopApplicationCatalog.isUserApplication(url: menuBar))
        let result = DesktopApplicationCatalog.scan(roots: [root])
        #expect(result.map(\.bundleID) == ["test.synthetic.menu-bar"])
        #expect(result.first?.pid == 0)
    }

    @Test func genuinelyDifferentApplicationsWithSameNameRemainSeparate() throws {
        let root = try root(); defer { try? FileManager.default.removeItem(at: root) }
        try app("First.app", under: root, id: "test.synthetic.first", name: "Same Name")
        try app("Second.app", under: root, id: "test.synthetic.second", name: "Same Name")
        let result = DesktopApplicationCatalog.scan(roots: [root])
        #expect(result.count == 2 && Set(result.map(\.bundleID)).count == 2)
        #expect(Set(result.compactMap(\.bundlePath)).count == 2)
    }

    @Test func depthAndCountLimitsDoNotTraverseAppPackagesOrUnrelatedTrees() throws {
        let root = try root(); defer { try? FileManager.default.removeItem(at: root) }
        try app("Top.app", under: root, id: "test.synthetic.top")
        try app("Tools/WithinDepth.app", under: root, id: "test.synthetic.child")
        try app("Tools/Deep/BeyondDepth.app", under: root, id: "test.synthetic.deep")
        #expect(DesktopApplicationCatalog.scan(roots: [root], maximumDepth: 0).map(\.bundleID) == ["test.synthetic.top"])
        #expect(DesktopApplicationCatalog.scan(roots: [root], maximumApplications: 1).count == 1)
        #expect(DesktopApplicationCatalog.scan(roots: [root], maximumApplications: 0).isEmpty)
        let shallow = DesktopApplicationCatalog.scan(roots: [root], maximumDepth: 1)
        #expect(Set(shallow.map(\.bundleID)) == ["test.synthetic.top", "test.synthetic.child"])
    }
}
