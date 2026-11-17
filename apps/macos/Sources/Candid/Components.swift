import CryptoKit
import Foundation

/// Runtimes Candid uses but does not ship inside the App: Node and Pi for the attached agent (and, separately, the
/// local voice service). They live under `Application Support/Candid/components`, are installed only when the user
/// asks, and are verified against pinned versions and hashes. A Node the user already has is reused when it is new
/// enough; a Pi the user already has is reported but only the pinned copy is launched (the extension and its
/// isolation flags are checked against that version).
enum Components {
    static var root: URL {
        if let dir = ProcessInfo.processInfo.environment["CANDID_COMPONENTS_DIR"], !dir.isEmpty { return URL(fileURLWithPath: dir, isDirectory: true) }
        return FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Candid/components", isDirectory: true)
    }

    /// Size of a folder on disk, in bytes (0 when missing).
    static func size(of url: URL) -> Int64 {
        guard let walker = FileManager.default.enumerator(at: url, includingPropertiesForKeys: [.totalFileAllocatedSizeKey, .isRegularFileKey]) else { return 0 }
        var total: Int64 = 0
        for case let file as URL in walker {
            let values = try? file.resourceValues(forKeys: [.totalFileAllocatedSizeKey, .isRegularFileKey])
            if values?.isRegularFile == true { total += Int64(values?.totalFileAllocatedSize ?? 0) }
        }
        return total
    }

    /// Runs a command to completion off the caller's thread; returns status and combined output (bounded).
    static func run(_ executable: URL, _ arguments: [String], directory: URL? = nil, environment: [String: String]? = nil,
                    timeout: TimeInterval = 600) async throws -> (status: Int32, output: String) {
        try await withCheckedThrowingContinuation { continuation in
            let process = Process()
            process.executableURL = executable
            process.arguments = arguments
            if let directory { process.currentDirectoryURL = directory }
            if let environment { process.environment = environment }
            let pipe = Pipe()
            process.standardOutput = pipe
            process.standardError = pipe
            process.standardInput = FileHandle.nullDevice
            let output = OutputBuffer()
            pipe.fileHandleForReading.readabilityHandler = { handle in
                let data = handle.availableData
                if data.isEmpty { handle.readabilityHandler = nil } else { output.append(data) }
            }
            process.terminationHandler = { ended in
                pipe.fileHandleForReading.readabilityHandler = nil
                output.append(pipe.fileHandleForReading.readDataToEndOfFile())
                continuation.resume(returning: (ended.terminationStatus, output.text))
            }
            do { try process.run() } catch { continuation.resume(throwing: error); return }
            DispatchQueue.global().asyncAfter(deadline: .now() + timeout) { if process.isRunning { process.terminate() } }
        }
    }

    private final class OutputBuffer: @unchecked Sendable {
        private let lock = NSLock()
        private var data = Data()
        func append(_ chunk: Data) { lock.withLock { if data.count < 64_000 { data.append(chunk.prefix(64_000 - data.count)) } } }
        var text: String { lock.withLock { String(decoding: data, as: UTF8.self) } }
    }
}

enum ComponentError: LocalizedError, Equatable {
    case noNode
    case checksum
    case failed(String)

    var errorDescription: String? {
        switch self {
        case .noNode: "需要 Node 22.19 或更新版本，请先安装 Node"
        case .checksum: "下载的文件校验不通过，已删除"
        case .failed(let detail): detail
        }
    }
}

/// A Node runtime: the one the user pointed at (`CANDID_PI_NODE`), Candid's managed copy, or one already on the system.
struct NodeRuntime: Sendable, Equatable {
    enum Origin: String, Sendable { case override, managed, system }
    var url: URL
    var version: String
    var origin: Origin

    static let pinned = "24.22.2"
    static let minimum = [22, 19, 0]
    private static let sha256 = [
        "arm64": "1f5ac075be50bed4780ea3be19a007fda95ea4d527d28d76d0b91de47d915115",
        "x64": "6965578fd823e72c1b5387f5ab84cb25368f886e02eb8779e6f0fce6b91c27b7",
    ]
    static var arch: String {
        #if arch(arm64)
        "arm64"
        #else
        "x64"
        #endif
    }
    static var managedDirectory: URL { Components.root.appendingPathComponent("node", isDirectory: true) }
    static var managedNode: URL { managedDirectory.appendingPathComponent("bin/node") }
    static let systemCandidates = ["/opt/homebrew/bin/node", "/usr/local/bin/node"]

    /// `v24.1.0` → [24, 1, 0]; nil when the binary does not answer like Node.
    static func version(of node: URL) -> String? {
        guard FileManager.default.isExecutableFile(atPath: node.path) else { return nil }
        let process = Process()
        process.executableURL = node
        process.arguments = ["--version"]
        process.environment = ["PATH": "/usr/bin:/bin"]
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        guard (try? process.run()) != nil else { return nil }
        let deadline = Date().addingTimeInterval(3)
        while process.isRunning && Date() < deadline { usleep(10_000) }
        if process.isRunning { process.terminate(); return nil }
        let text = String(decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        return text.hasPrefix("v") ? String(text.dropFirst()) : nil
    }

    static func isSupported(_ version: String) -> Bool {
        let parts = version.split(separator: ".").map { Int($0) ?? 0 }
        return parts.lexicographicallyPrecedes(minimum) == false
    }

    static func resolve(environment: [String: String] = ProcessInfo.processInfo.environment) -> NodeRuntime? {
        if let path = environment["CANDID_PI_NODE"], !path.isEmpty {
            let url = URL(fileURLWithPath: path)
            return NodeRuntime(url: url, version: version(of: url) ?? "?", origin: .override)
        }
        if let v = version(of: managedNode), isSupported(v) { return NodeRuntime(url: managedNode, version: v, origin: .managed) }
        for path in systemCandidates {
            let url = URL(fileURLWithPath: path)
            if let v = version(of: url), isSupported(v) { return NodeRuntime(url: url, version: v, origin: .system) }
        }
        return nil
    }

    /// npm that belongs to this Node (managed tarballs and Homebrew both keep it under `lib/node_modules/npm`).
    var npmCLI: URL? {
        let relative = "lib/node_modules/npm/bin/npm-cli.js"
        let roots = [url.resolvingSymlinksInPath().deletingLastPathComponent().deletingLastPathComponent(),
                     url.deletingLastPathComponent().deletingLastPathComponent()]
        return roots.map { $0.appendingPathComponent(relative) }.first { FileManager.default.fileExists(atPath: $0.path) }
    }

    /// Downloads the pinned Node from nodejs.org, checks its SHA-256 and installs it as the managed copy.
    static func installManaged(progress: @escaping @Sendable (String) -> Void) async throws -> NodeRuntime {
        let fm = FileManager.default
        let name = "node-v\(pinned)-darwin-\(arch)"
        let staging = Components.root.appendingPathComponent(".staging-node-\(UUID().uuidString)", isDirectory: true)
        try fm.createDirectory(at: staging, withIntermediateDirectories: true)
        defer { try? fm.removeItem(at: staging) }
        progress("正在下载 Node \(pinned)…")
        let (file, response) = try await URLSession.shared.download(from: URL(string: "https://nodejs.org/dist/v\(pinned)/\(name).tar.xz")!)
        guard (response as? HTTPURLResponse)?.statusCode == 200 else { throw ComponentError.failed("下载 Node 失败") }
        let tarball = staging.appendingPathComponent("node.tar.xz")
        try fm.moveItem(at: file, to: tarball)
        progress("正在校验…")
        let digest = SHA256.hash(data: try Data(contentsOf: tarball, options: .mappedIfSafe)).map { String(format: "%02x", $0) }.joined()
        guard digest == sha256[arch] else { throw ComponentError.checksum }
        progress("正在解压…")
        let result = try await Components.run(URL(fileURLWithPath: "/usr/bin/tar"), ["-xJf", tarball.path, "-C", staging.path])
        guard result.status == 0 else { throw ComponentError.failed("解压 Node 失败：\(result.output.prefix(300))") }
        let extracted = staging.appendingPathComponent(name, isDirectory: true)
        if fm.fileExists(atPath: managedDirectory.path) { try fm.removeItem(at: managedDirectory) }
        try fm.moveItem(at: extracted, to: managedDirectory)
        guard let v = version(of: managedNode) else { throw ComponentError.failed("Node 安装后无法运行") }
        return NodeRuntime(url: managedNode, version: v, origin: .managed)
    }
}

/// Pi with Candid's extension. The App carries only the small skeleton (package.json + lockfile, `index.ts`, system
/// prompt, skills); installing runs `npm ci` from that lockfile into the managed folder.
enum PiComponent {
    static let package = "@earendil-works/pi-coding-agent"

    static var skeleton: URL {
        if let dir = ProcessInfo.processInfo.environment["CANDID_PI_SKELETON"], !dir.isEmpty { return URL(fileURLWithPath: dir, isDirectory: true) }
        return Bundle.main.bundleURL.appendingPathComponent("Contents/Resources/Pi", isDirectory: true)
    }
    static var managedHome: URL { Components.root.appendingPathComponent("pi", isDirectory: true) }

    /// The version the extension was built and tested against.
    static var pinned: String? {
        guard let data = try? Data(contentsOf: skeleton.appendingPathComponent("package.json")),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return nil }
        return (json["dependencies"] as? [String: String])?[package]
    }

    static func installedVersion(in home: URL) -> String? {
        let file = home.appendingPathComponent("node_modules/\(package)/package.json")
        guard let data = try? Data(contentsOf: file), let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return nil }
        return json["version"] as? String
    }

    /// Same major and minor as the pinned version.
    static func isCompatible(_ version: String) -> Bool {
        guard let pinned else { return false }
        return version.split(separator: ".").prefix(2) == pinned.split(separator: ".").prefix(2)
    }

    /// A Pi the user installed themselves (Homebrew or a global npm install), if any.
    static func userInstall() -> (version: String, location: String)? {
        let fm = FileManager.default
        var candidates = ["/opt/homebrew/lib/node_modules/\(package)", "/usr/local/lib/node_modules/\(package)",
                          NSHomeDirectory() + "/.npm-global/lib/node_modules/\(package)"]
        for brew in ["/opt/homebrew/bin/pi", "/usr/local/bin/pi"] {
            let resolved = URL(fileURLWithPath: brew).resolvingSymlinksInPath()
            if resolved.path != brew { candidates.insert(resolved.deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("libexec/lib/node_modules/\(package)").path, at: 0) }
        }
        for path in candidates {
            guard let data = fm.contents(atPath: path + "/package.json"),
                  let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any], let v = json["version"] as? String else { continue }
            return (v, path.contains("/Cellar/") ? "Homebrew" : "npm")
        }
        return nil
    }

    /// Installs the pinned Pi and the extension into the managed folder with `npm ci`, without install scripts.
    static func installManaged(node: NodeRuntime, progress: @escaping @Sendable (String) -> Void) async throws {
        let fm = FileManager.default
        guard let npm = node.npmCLI else { throw ComponentError.failed("找不到与 Node \(node.version) 配套的 npm") }
        guard let pinned else { throw ComponentError.failed("App 里缺少 Pi 的版本清单") }
        let staging = Components.root.appendingPathComponent(".staging-pi-\(UUID().uuidString)", isDirectory: true)
        try fm.createDirectory(at: Components.root, withIntermediateDirectories: true)
        try fm.copyItem(at: skeleton, to: staging)
        defer { try? fm.removeItem(at: staging) }
        progress("正在安装 Pi \(pinned)…")
        let home = Components.root.appendingPathComponent(".npm-home", isDirectory: true)
        try fm.createDirectory(at: home, withIntermediateDirectories: true)
        let result = try await Components.run(node.url, [npm.path, "ci", "--omit=dev", "--ignore-scripts", "--no-audit", "--no-fund",
                                                         "--no-update-notifier", "--loglevel=error", "--cache", Components.root.appendingPathComponent(".npm-cache").path],
                                              directory: staging,
                                              environment: ["PATH": node.url.deletingLastPathComponent().path + ":/usr/bin:/bin", "HOME": home.path])
        guard result.status == 0 else { throw ComponentError.failed("npm 安装失败：\(result.output.suffix(400))") }
        // Only what runs: no shims, no optional native helpers, no SDKs for providers Candid never uses (the extension
        // is Pi's only provider, through the core).
        let modules = staging.appendingPathComponent("node_modules")
        for item in [".bin", "@earendil-works/pi-tui/native", "@anthropic-ai", "@aws", "@aws-sdk", "@smithy", "@google",
                     "google-auth-library", "gaxios", "gcp-metadata", "google-logging-utils", "protobufjs", "@protobufjs"] {
            try? fm.removeItem(at: modules.appendingPathComponent(item))
        }
        guard installedVersion(in: staging) == pinned else { throw ComponentError.failed("安装的 Pi 不是固定版本 \(pinned)") }
        if fm.fileExists(atPath: managedHome.path) { try fm.removeItem(at: managedHome) }
        try fm.moveItem(at: staging, to: managedHome)
    }

    static func removeManaged() throws {
        if FileManager.default.fileExists(atPath: managedHome.path) { try FileManager.default.removeItem(at: managedHome) }
    }
}
