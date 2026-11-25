import AppKit
import AIProviders
import Foundation
import SwiftUI
import Testing
@testable import Candid

@MainActor @Suite(.serialized) struct ModelSettingsTests {
    @Test func chooserSearchesFiltersAndKeepsUnknownCapabilitiesSelectable() throws {
        var config = AIRuntime.syntheticSample
        let role = try #require(ModelRoleInfo.info("vision"))
        var noImages = AIModel(providerID: "sample-lm", modelID: "text-only", modalities: ["text"])
        noImages.modalitiesDeclared = true
        config.models.append(noImages)
        #expect(RoleModelMatch.reason(noImages, role: role, provider: config.providers[0]) != nil)
        #expect(RoleModelMatch.reason(config.models[1], role: role, provider: config.providers[0]) == nil)
        #expect(RoleModelMatch.candidates(config, role: role, query: "vision", filter: "images").count == 1)
        #expect(RoleModelMatch.candidates(config, role: role, query: "不存在", filter: "all").isEmpty)
        config.providers[0].enabled = false
        #expect(RoleModelMatch.reason(config.models[0], role: role, provider: config.providers[0]) == "来源已停用")
    }

    @Test func suggestedRolesNeverReplaceExistingAssignmentsOrLimits() throws {
        var config = AIRuntime.syntheticSample
        var assignment = RoleAssignment(role: "title", model: config.models[1].ref, limits: RoleLimits(maxOutputTokens: 80))
        assignment.prompt = "SYNTHETIC prompt"
        try config.setAssignment(assignment)
        let store = InMemoryProviderConfigStore(config)
        let runtime = AIRuntime(store: store, secrets: InMemorySecretStore(), sink: InMemoryUsageSink())
        let model = ModelSettingsModel(runtime: runtime)
        model.assignSuggestions(from: config.providers[0])
        #expect(model.config.assignment("title") == assignment)
        #expect(model.config.assignment("agent_chat")?.model != nil)
        #expect(model.config.assignment("ocr") == nil)
        let restored = ModelSettingsModel(runtime: runtime)
        #expect(restored.config == model.config)
    }

    @Test func recordingModesPreservePolicyAndPersistThroughReload() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("workflow-modes-" + UUID().uuidString)
        defer { TestCore.finish(root) }
        let core = try await TestCore.ready(root)
        let model = WorkflowControlModel(channel: { core.channel })
        await model.refresh(includeActivity: false)

        #expect(model.processingMode("transcribe") == .automatic)
        #expect(model.processingMode("title") == .automatic)
        #expect(model.processingMode("summary") == .manual)
        var policy = try #require(model.featureSettings.items["transcribe"])
        policy.daily_calls = 37
        policy.allow_remote = false
        policy.sources = ["mac_microphone"]
        await model.setPolicy("transcribe", policy)

        let modes: [WorkflowControlModel.ProcessingMode] = [.off, .manual, .off, .automatic]
        for mode in modes {
            await model.setProcessingMode("transcribe", mode)
            #expect(model.error == nil)
            let restored = WorkflowControlModel(channel: { core.channel })
            await restored.refresh(includeActivity: false)
            #expect(restored.processingMode("transcribe") == mode)
            #expect(restored.featureSettings.items["transcribe"]?.allow_remote == false)
            #expect(restored.featureSettings.items["transcribe"]?.daily_calls == 37)
            #expect(restored.featureSettings.items["transcribe"]?.sources == ["mac_microphone"])
            #expect(restored.processingMode("title") == .automatic)
            #expect(restored.processingMode("summary") == .manual)
        }
    }

    @Test func rendersSettingsAndChooserWithSyntheticData() async throws {
        _ = NSApplication.shared
        var config = AIRuntime.syntheticSample
        let voice = AIProvider(providerID: "synthetic-voice", name: "本机语音服务", api: .openaiChat, baseURL: "http://127.0.0.1:8767/v1", destination: .onDevice)
        let cloud = AIProvider(providerID: "synthetic-cloud", name: "合成云端来源", api: .openaiChat, baseURL: "https://example.invalid/v1", destination: .cloud, enabled: false)
        try config.upsert(voice); try config.upsert(cloud)
        let usage = InMemoryUsageSink()
        for day in 0..<7 {
            for index in 0...day {
                await usage.record(AICallRecord(callID: "synth-\(day)-\(index)", startedMs: nowMs() - Int64(day) * 86400000, feature: "agent_chat", providerID: config.providers[0].providerID,
                                               modelID: "synthetic", destination: .onDevice, inputTokens: 100, outputTokens: 20,
                                               cost: 0.01, currency: "USD", costSource: .estimated, status: .ok))
            }
        }
        let runtime = AIRuntime(store: InMemoryProviderConfigStore(config), secrets: InMemorySecretStore(), sink: usage)
        let model = ModelSettingsModel(runtime: runtime); await model.loadUsage(days: 7)
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("workflow-render-" + UUID().uuidString)
        defer { TestCore.finish(root) }
        let core = try await TestCore.ready(root)
        let controls = WorkflowControlModel(channel: { core.channel })
        await controls.refresh(includeActivity: false)
        #expect(controls.error == nil)
        let recognition = DesktopRecognitionSession()
        for dark in [false, true] {
            let contents: [(String, AnyView)] = [
                ("sources", AnyView(AIFunctionSettingsView(model: controls, models: model, initialTab: .sources, refreshOnAppear: false))),
                ("model-editor", AnyView(AIWorkspaceSheet(title: "图像理解") {
                    ModelRoleEditor(model: model, info: ModelRoleInfo.info("vision")!, promptOpen: true)
                })),
                ("chooser", AnyView(RoleModelChooser(model: model, info: ModelRoleInfo.info("vision")!, close: {}).padding(24))),
                ("ai-functions", AnyView(AIFunctionSettingsView(model: controls, models: model, recognition: recognition, refreshOnAppear: false)))
            ]
            for (name, content) in contents {
                let view = NSHostingView(rootView: content
                    .frame(width: 654, height: 700, alignment: .topLeading)
                    .environment(\.colorScheme, dark ? .dark : .light)
                    .environment(\.visualStyle, .flat)
                    .background(dark ? Color(white: 0.12) : Color(white: 0.96)))
                view.frame = NSRect(x: 0, y: 0, width: 654, height: 700)
                let window = NSWindow(contentRect: view.frame, styleMask: [.borderless], backing: .buffered, defer: false)
                window.isReleasedWhenClosed = false
                window.contentView = view
                view.layoutSubtreeIfNeeded()
                try await Task.sleep(for: .milliseconds(150))
                view.layoutSubtreeIfNeeded()
                let bitmap = try #require(view.bitmapImageRepForCachingDisplay(in: view.bounds))
                view.cacheDisplay(in: view.bounds, to: bitmap)
                if let dir = ProcessInfo.processInfo.environment["CANDID_RENDER_DIR"] {
                    try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
                    let png = try #require(bitmap.representation(using: .png, properties: [:]))
                    try png.write(to: URL(fileURLWithPath: dir).appendingPathComponent("settings-\(name)-\(dark ? "dark" : "light").png"))
                }
                window.close()
            }
        }
    }
}
