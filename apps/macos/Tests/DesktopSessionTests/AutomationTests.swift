import AppKit
import Foundation
import SwiftUI
import CandidClient
import Testing
@testable import Candid

@MainActor final class FakeAutomationNotifier: AutomationNotifying {
    var sent: Set<String> = []
    func requestAccess() async -> Bool { true }
    func deliver(_ run: AutomationRun, title: String) async -> Bool {
        guard run.policy.notify, run.notify_ready == true, !run.notified, !run.references.isEmpty else { return false }
        sent.insert(run.id); return true
    }
}
@MainActor @Suite(.serialized) struct AutomationTests {
    private func setup() async throws -> (URL, LocalChannel) {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("automation-tests-" + UUID().uuidString)
        return (root, try #require(try await TestCore.ready(root, cutover: false).channel))
    }
    @Test func skillSettingsPersistAndRejectBadTimesWithoutChangingSavedState() async throws {
        let (root, channel) = try await setup(); defer { TestCore.finish(root) }
        let model = AutomationModel(channel: { channel }); await model.refresh()
        #expect(model.catalog.count == 5 && !model.passed)
        #expect(model.config.skills.values.allSatisfy { !$0.automatic && !$0.notify })
        await model.update("daily-review") { $0.time = "20:30"; $0.daily_runs = 2; $0.sources = [] }
        #expect(model.error == nil)
        let fresh = AutomationModel(channel: { channel }); await fresh.refresh()
        #expect(fresh.config.skills["daily-review"]?.time == "20:30" && fresh.config.skills["daily-review"]?.sources == [])
        await fresh.update("daily-review") { $0.time = "bad time" }
        #expect(fresh.error != nil)
        await fresh.refresh(); #expect(fresh.config.skills["daily-review"]?.time == "20:30")
    }
    @Test func backgroundRunFinishesAndCancelRetryUsePersistedCoreState() async throws {
        let (root, channel) = try await setup(); defer { TestCore.finish(root) }
        let queued: AutomationRun = try await automationPost(channel, ["action": "run", "skill": "daily-review", "key": "synthetic-unique-key"])
        let duplicate: AutomationRun = try await automationPost(channel, ["action": "run", "skill": "daily-review", "key": "synthetic-unique-key"])
        #expect(queued.id == duplicate.id)
        let agent = FakeChatAgent(); agent.script = [.assistantMessage("{\"has_updates\":false,\"summary\":\"没有新内容\"}"), .settled(aborted: false)]
        let skill = try #require(AISkill.parse(id: "daily-review", text: "---\ntitle: 日回顾\ntrigger: manual\n---\n# 日回顾\n只读合成来源。"))
        let notifier = FakeAutomationNotifier()
        let worker = SkillRunner(channel: { channel }, agent: agent, notifier: notifier, skills: { [skill] })
        await worker.tick()
        for _ in 0..<30 {
            let snapshot: AutomationSnapshot = try await automationPost(channel, ["action": "status"])
            if snapshot.runs.first?.state == "done" { break }
            try await Task.sleep(for: .milliseconds(20))
        }
        let snapshot: AutomationSnapshot = try await automationPost(channel, ["action": "status"])
        #expect(snapshot.runs.first?.state == "done" && snapshot.runs.first?.displayResult == "没有新内容")
        #expect(agent.started.first?.automationID == queued.id && agent.prompts.count == 1 && notifier.sent.isEmpty)
        let another: AutomationRun = try await automationPost(channel, ["action": "run", "skill": "daily-review"])
        let model = AutomationModel(channel: { channel }); await model.command("cancel", id: another.id)
        #expect(model.runs.first(where: { $0.id == another.id })?.state == "cancelled")
        await model.command("retry", id: another.id)
        #expect(model.runs.first(where: { $0.id == another.id })?.state == "queued")
    }
    @Test func skillPanelRendersAtSettingsWidthInLightAndDark() async throws {
        let (root, channel) = try await setup(); defer { TestCore.finish(root) }
        let model = AutomationModel(channel: { channel }); await model.refresh()
        _ = NSApplication.shared
        for dark in [false, true] {
            let content = ScrollView { SkillSettingsView(model: model, expanded: "daily-review", refreshOnAppear: false).padding(24) }
                .frame(width: 760, height: 1100).environment(\.colorScheme, dark ? .dark : .light)
                .background(dark ? Color(white: 0.12) : Color(white: 0.96))
            let view = NSHostingView(rootView: content); view.frame = NSRect(x: 0, y: 0, width: 760, height: 1100)
            let window = NSWindow(contentRect: view.frame, styleMask: [.borderless], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false; window.contentView = view; view.layoutSubtreeIfNeeded()
            try await Task.sleep(for: .milliseconds(150)); view.layoutSubtreeIfNeeded()
            let bitmap = try #require(view.bitmapImageRepForCachingDisplay(in: view.bounds)); view.cacheDisplay(in: view.bounds, to: bitmap)
            let png = try #require(bitmap.representation(using: .png, properties: [:]))
            if let dir = ProcessInfo.processInfo.environment["CANDID_RENDER_DIR"] {
                try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
                try png.write(to: URL(fileURLWithPath: dir).appendingPathComponent("skills-\(dark ? "dark" : "light").png"))
            }
            window.close()
        }
    }
    @Test func screenProjectionEditsAndEvidenceRenderFromRealCore() async throws {
        let (root, channel) = try await setup(); defer { TestCore.finish(root) }
        try await TestCore.ready(root, cutover: false).cutover()
        func post(_ path: String, _ body: [String: Any]) async throws -> [String: Any] {
            let data = try await channel.send("POST", path, body: JSONSerialization.data(withJSONObject: body)).checked()
            return try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
        }
        var features = (try await post("/local/workflows", ["action": "status"]))["features"] as! [String: Any]
        var items = features["items"] as! [String: Any]
        var screenPolicy = items["screen-episodes"] as! [String: Any]; screenPolicy["enabled"] = true; items["screen-episodes"] = screenPolicy
        features["items"] = items
        _ = try await post("/local/workflows", ["action": "features", "config": features])
        let source = try await post("/local/sources", ["kind": "mac.desktop", "display_name": "合成桌面"])["source_id"] as! String
        let time = Int64(Date().addingTimeInterval(-3600).timeIntervalSince1970 * 1000)
        _ = try await post("/local/ingest", ["source_id": source, "events": [
            ["source_key": "screen-synthetic-1", "kind": "screen.text", "start_ms": time, "app_bundle_id": "org.synthetic.notes", "app_name": "合成笔记", "title": "合成项目讨论", "text": [["field": "body", "content": "项目预算是 100 元\n等周五确认。", "visible": true]]],
            ["source_key": "screen-synthetic-2", "kind": "screen.text", "start_ms": time + 30000, "app_bundle_id": "org.synthetic.notes", "app_name": "合成笔记", "title": "合成项目讨论", "text": [["field": "body", "content": "项目预算改为 200 元\n等周五确认。", "visible": true]]]
        ]])
        _ = try await post("/local/screen", ["action": "rebuild"])
        let model = ScreenProcessingModel(channel: { channel }); await model.refresh()
        let episode = try #require(model.episodes.first)
        await model.select(episode)
        #expect(model.lines.contains { $0.text.contains("100") } && model.lines.contains { $0.text.contains("200") })
        model.title = "用户的标题"; model.summary = "预算改成 200 元，周五确认。"
        await model.command("edit", id: episode.id)
        #expect(model.episodes.first?.title == "用户的标题" && model.episodes.first?.status == "edited")
        _ = NSApplication.shared
        for dark in [false, true] {
            let content = ScrollView { ScreenProcessingView(model: model, refreshOnAppear: false).padding(24) }
                .frame(width: 760, height: 900).environment(\.colorScheme, dark ? .dark : .light)
                .background(dark ? Color(white: 0.12) : Color(white: 0.96))
            let view = NSHostingView(rootView: content); view.frame = NSRect(x: 0, y: 0, width: 760, height: 900)
            let window = NSWindow(contentRect: view.frame, styleMask: [.borderless], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false; window.contentView = view; view.layoutSubtreeIfNeeded()
            try await Task.sleep(for: .milliseconds(150)); view.layoutSubtreeIfNeeded()
            let bitmap = try #require(view.bitmapImageRepForCachingDisplay(in: view.bounds)); view.cacheDisplay(in: view.bounds, to: bitmap)
            if let dir = ProcessInfo.processInfo.environment["CANDID_RENDER_DIR"] {
                try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
                let png = try #require(bitmap.representation(using: .png, properties: [:]))
                try png.write(to: URL(fileURLWithPath: dir).appendingPathComponent("screen-\(dark ? "dark" : "light").png"))
            }
            window.close()
        }
    }

}
