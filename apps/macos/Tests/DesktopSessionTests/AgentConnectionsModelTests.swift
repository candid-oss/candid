import Foundation
import AppKit
import SwiftUI
import Testing
import TimelineConnectors
import CandidClient
@testable import Candid

private let unavailableAgents = AgentCoreClient(transport: { _, _ in throw CoreError.unreachable("Synthetic unavailable core") })

@MainActor
@Test func connectionsModelRequiresReadAndKeepsActivityIndependent() async throws {
    let store = InMemoryAgentConsentStore()
    let model = AgentConnectionsModel(store: store, connectors: AgentConnectors.all(client: unavailableAgents))
    #expect(model.rows.map(\.id) == ["codex", "claude_code", "pi"])
    #expect(model.rows.allSatisfy { !$0.consent.read && !$0.consent.search && !$0.consent.activity })

    model.update("codex") { $0.search = true }                               // search or activity turns reading on, in any order
    #expect(store.consent(for: "codex").read && store.consent(for: "codex").search && !store.consent(for: "codex").activity)
    model.update("codex") { $0.activity = true }
    #expect(store.consent(for: "codex").activity)
    #expect(store.consent(for: "codex").read && store.consent(for: "codex").search)
    model.update("codex") { $0.read = false }                                // turning read off turns search off too
    #expect(!store.consent(for: "codex").search && !store.consent(for: "codex").activity)
    #expect(model.pendingRemoval == "codex")
    model.keepStored()
    #expect(model.pendingRemoval == nil)

    model.setPaths("pi", "/a/b, /c\n/d")
    model.setExcludedProjects("pi", "/x， /y")
    #expect(store.consent(for: "pi").paths == ["/a/b", "/c", "/d"] && store.consent(for: "pi").exclude.projects == ["/x", "/y"])
}

@MainActor
@Test func connectionsStatusTextFollowsDetectionAndConsent() {
    let d = AgentDescription(agent: "codex", displayName: "Codex", defaultPaths: [], formatVersions: [])
    typealias R = AgentConnectionsModel.Row
    var row = R(description: d, consent: AgentConsent(agent: "codex"), detection: nil)
    #expect(AgentConnectionsModel.statusText(row).text == "检测中…")
    row.detection = AgentDetection(agent: "codex", state: .notFound, formatVersion: nil, headInspected: false)
    #expect(AgentConnectionsModel.statusText(row).text == "未发现")
    row.detection = AgentDetection(agent: "codex", state: .found, formatVersion: nil, headInspected: false)
    #expect(AgentConnectionsModel.statusText(row).text == "已发现，未授权")
    row.consent.read = true
    #expect(AgentConnectionsModel.statusText(row).on)
}

/// Opt-in render of the production settings view, using only synthetic authorisation and transport.
@MainActor @Test(.enabled(if: ProcessInfo.processInfo.environment["CANDID_AGENT_SNAPSHOT_DIR"] != nil))
func renderAgentConnectionsWithSyntheticConsent() async throws {
    let directory = URL(fileURLWithPath: try #require(ProcessInfo.processInfo.environment["CANDID_AGENT_SNAPSHOT_DIR"]))
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    let transport = AgentCoreClient(transport: { _, body in
        let request = try JSONSerialization.jsonObject(with: body ?? Data("{}".utf8)) as? [String: String]
        return try JSONSerialization.data(withJSONObject: ["agent": request?["agent"] ?? "codex", "state": "found", "headInspected": false])
    })
    let store = InMemoryAgentConsentStore()
    var consent = AgentConsent(agent: "codex"); consent.read = true; consent.search = true; consent.activity = true
    try store.save(consent)
    let model = AgentConnectionsModel(store: store, connectors: AgentConnectors.all(client: transport))
    await model.refresh()
    model.update("codex") { $0.activity = false }
    for dark in [false, true] {
        for style in [VisualStyle.ambient, .flat] {
            let content = AgentConnectionsGroup(model: model).padding(24)
                .frame(width: 940, height: 660, alignment: .topLeading)
                .environment(\.visualStyle, style).environment(\.colorScheme, dark ? .dark : .light)
                .background(dark ? Color.black : Color.white)
            let view = NSHostingView(rootView: content)
            view.frame = NSRect(x: 0, y: 0, width: 940, height: 660)
            let window = NSWindow(contentRect: view.frame, styleMask: [.borderless], backing: .buffered, defer: false)
            window.contentView = view
            view.layoutSubtreeIfNeeded()
            let rep = try #require(view.bitmapImageRepForCachingDisplay(in: view.bounds))
            view.cacheDisplay(in: view.bounds, to: rep)
            let png = try #require(rep.representation(using: .png, properties: [:]))
            try png.write(to: directory.appendingPathComponent("agents-\(style.rawValue)-\(dark ? "dark" : "light").png"))
            window.orderOut(nil)
        }
    }
}
