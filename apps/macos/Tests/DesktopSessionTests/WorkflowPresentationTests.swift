import AppKit
import Foundation
import SwiftUI
import Testing
@testable import Candid

@MainActor @Test func workflowControlsRenderRealStatesWithoutVisualChanges() async throws {
    let directory = URL(fileURLWithPath: "/tmp/pa-workflows-render")
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    let model = WorkflowControlModel()
    model.definitions = [.init(id: "recording", title: "录音整理", implemented: true), .init(id: "daily", title: "录音日回顾", implemented: true)]
    model.jobs = [.init(id: "SYNTHETIC-1", kind: "speech.recognize", input_ref: "SYNTHETIC", state: "running"), .init(id: "SYNTHETIC-2", kind: "workflow.title", input_ref: "SYNTHETIC", state: "queued", error_code: "waiting_model"), .init(id: "SYNTHETIC-3", kind: "workflow.summary", input_ref: "SYNTHETIC", state: "failed", error_code: "http_429")]
    _ = NSApplication.shared
    for dark in [false, true] {
        let content = AIControlPanels(workflows: model).workflowControls.padding(24)
            .frame(width: 940, height: 760, alignment: .topLeading)
            .environment(\.colorScheme, dark ? .dark : .light)
            .background(dark ? Color.black : Color.white)
        let view = NSHostingView(rootView: content)
        view.frame = NSRect(x: 0, y: 0, width: 940, height: 760)
        let window = NSWindow(contentRect: view.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = view; view.layoutSubtreeIfNeeded()
        let bitmap = try #require(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        let png = try #require(bitmap.representation(using: .png, properties: [:]))
        #expect(png.count > 10_000)
        try png.write(to: directory.appendingPathComponent(dark ? "dark.png" : "light.png"))
        window.close()
    }
}
