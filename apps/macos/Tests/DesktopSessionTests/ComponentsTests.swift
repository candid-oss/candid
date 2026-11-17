import Foundation
import Testing
@testable import Candid

/// Settings → 组件: versions, compatibility and where Node and Pi are taken from. The real install (network, npm) only
/// runs with `CANDID_COMPONENT_INSTALL_TEST=1`; it uses a temporary components folder, never the user's.
@Suite(.serialized) struct ComponentsTests {
    @Test func nodeVersionsBelowTheMinimumAreRejected() {
        #expect(NodeRuntime.isSupported("22.19.0"))
        #expect(NodeRuntime.isSupported("24.22.2"))
        #expect(NodeRuntime.isSupported("26.0.0"))
        #expect(!NodeRuntime.isSupported("22.18.9"))
        #expect(!NodeRuntime.isSupported("20.11.1"))
    }

    @Test func piCompatibilityIsTheSameMajorAndMinorAsThePinnedVersion() throws {
        let skeleton = try #require(Self.skeleton)
        try Self.withEnvironment(["CANDID_PI_SKELETON": skeleton.path]) {
            let pinned = try #require(PiComponent.pinned)
            let parts = pinned.split(separator: ".")
            #expect(PiComponent.isCompatible(pinned))
            #expect(PiComponent.isCompatible("\(parts[0]).\(parts[1]).99"))
            #expect(!PiComponent.isCompatible("\(parts[0]).\(Int(parts[1])! + 1).0"))
            #expect(!PiComponent.isCompatible("0.1.0"))
        }
    }

    @Test func overridesWinAndAMissingRuntimeIsReportedAsNotInstalled() {
        let install = PiInstall.resolved(environment: ["CANDID_PI_NODE": "/nonexistent/node", "CANDID_PI_HOME": "/nonexistent/pi"])
        #expect(install.node.path == "/nonexistent/node")
        #expect(install.home.path == "/nonexistent/pi")
        #expect(!install.isInstalled)
        let node = NodeRuntime.resolve(environment: ["CANDID_PI_NODE": "/nonexistent/node"])
        #expect(node?.origin == .override)
    }

    @Test func installsThePinnedPiIntoAnIsolatedFolder() async throws {
        guard ProcessInfo.processInfo.environment["CANDID_COMPONENT_INSTALL_TEST"] == "1" else { return }
        let skeleton = try #require(Self.skeleton)
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("components-" + UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        try await Self.withEnvironment(["CANDID_COMPONENTS_DIR": folder.path, "CANDID_PI_SKELETON": skeleton.path]) {
            let node = try #require(NodeRuntime.resolve(environment: [:]), "needs a system Node 22.19+")
            try await PiComponent.installManaged(node: node) { _ in }
            #expect(PiComponent.installedVersion(in: PiComponent.managedHome) == PiComponent.pinned)
            let install = PiInstall(node: node.url, home: PiComponent.managedHome)
            #expect(install.isInstalled)
            #expect(!FileManager.default.fileExists(atPath: PiComponent.managedHome.appendingPathComponent("node_modules/.bin").path))
            try PiComponent.removeManaged()
            #expect(!FileManager.default.fileExists(atPath: PiComponent.managedHome.path))
        }
    }

    /// A skeleton like the App's `Resources/Pi`, from the checkout's extension.
    static var skeleton: URL? {
        let dir = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("../../../../agents/pi-extension").standardizedFileURL
        return FileManager.default.fileExists(atPath: dir.appendingPathComponent("package.json").path) ? dir : nil
    }

    static func withEnvironment<T>(_ values: [String: String], _ body: () async throws -> T) async rethrows -> T {
        for (key, value) in values { setenv(key, value, 1) }
        defer { for key in values.keys { unsetenv(key) } }
        return try await body()
    }

    static func withEnvironment<T>(_ values: [String: String], _ body: () throws -> T) rethrows -> T {
        for (key, value) in values { setenv(key, value, 1) }
        defer { for key in values.keys { unsetenv(key) } }
        return try body()
    }
}
