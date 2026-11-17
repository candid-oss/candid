// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "Candid",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "Candid", targets: ["Candid"]),
        .executable(name: "DesktopDiagnostics", targets: ["DesktopDiagnostics"]),
        .executable(name: "CandidWidget", targets: ["CandidWidget"]),
        .library(name: "ConsoleCore", targets: ["ConsoleCore"]),
        .library(name: "DesktopCore", targets: ["DesktopCore"]),
        // The App's client of the Rust core (core): helper process, local channel, change feed.
        .library(name: "CandidClient", targets: ["CandidClient"]),
        .library(name: "AIProviders", targets: ["AIProviders"]),
        .library(name: "TimelineConnectors", targets: ["TimelineConnectors"]),
        .executable(name: "TimelineConnectorsDryRun", targets: ["TimelineConnectorsDryRun"])
    ],
    targets: [
        .target(name: "ConsoleCore", dependencies: ["CandidShared", "CandidLocalization"], resources: [.process("Resources")]),
        .target(name: "DesktopCore", dependencies: ["CandidClient", "CandidLocalization"]),
        .target(name: "DesktopPlatform", dependencies: ["DesktopCore", "CandidShared", "CandidLocalization"]),
        // Foundation only: the timeline library, query service, MCP and `candid` command live in the Rust core,
        // which the App runs as Contents/Helpers/candid (ADR-024).
        .target(name: "CandidClient", dependencies: ["CandidLocalization"]),
        .executableTarget(name: "DesktopDiagnostics", dependencies: ["DesktopCore", "DesktopPlatform", "CandidClient"]),
        .target(name: "CandidShared"),
        // UI text: Chinese keys in the source, English from Contents/Resources/Localizable.en.json.
        .target(name: "CandidLocalization"),
        .target(name: "AIProviders", dependencies: ["CandidShared", "CandidLocalization"]),
        // WidgetKit extension; packaged into Contents/PlugIns by script/build_and_run.sh.
        .executableTarget(name: "CandidWidget", dependencies: ["CandidShared", "CandidLocalization"],
                          linkerSettings: [.linkedFramework("WidgetKit")]),
        .target(name: "TimelineConnectors", dependencies: ["CandidClient"], linkerSettings: [.linkedFramework("EventKit")]),
        .executableTarget(name: "TimelineConnectorsDryRun", dependencies: ["TimelineConnectors"]),
        .executableTarget(name: "Candid", dependencies: ["ConsoleCore", "DesktopCore", "DesktopPlatform", "CandidShared", "AIProviders", "TimelineConnectors", "CandidClient", "CandidLocalization"]),
        .testTarget(name: "ConsoleCoreTests", dependencies: ["ConsoleCore"]),
        .testTarget(name: "CandidLocalizationTests", dependencies: ["CandidLocalization"]),
        .testTarget(name: "DesktopCoreTests", dependencies: ["DesktopCore", "CandidClient"]),
        .testTarget(name: "DesktopPlatformTests", dependencies: ["DesktopPlatform", "DesktopCore"]),
        .testTarget(name: "AIProvidersTests", dependencies: ["AIProviders", "CandidShared"]),
        .testTarget(name: "CandidClientTests", dependencies: ["CandidClient"]),
        .testTarget(name: "TimelineConnectorsTests", dependencies: ["TimelineConnectors"]),
        .testTarget(name: "DesktopSessionTests", dependencies: ["Candid", "DesktopPlatform", "DesktopCore", "CandidShared", "AIProviders", "CandidClient"])
    ]
)
