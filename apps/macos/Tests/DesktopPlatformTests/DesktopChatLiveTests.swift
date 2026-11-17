import AppKit
import ApplicationServices
import DesktopCore
import Testing
@testable import DesktopPlatform

/// Explicit, isolated native test. Launch only the synthetic bundle, never
/// enable the installed app or read another application's content.
@Suite(.serialized) struct DesktopChatLiveTests {
    @MainActor @Test(.enabled(if: ProcessInfo.processInfo.environment["CANDID_CHAT_LIVE"] == "1"))
    func nativeChat() async throws {
        #expect(AXIsProcessTrusted()); #expect(CGPreflightListenEventAccess())
        func resourceSample() throws -> [String: UInt64]? {
            guard let path = ProcessInfo.processInfo.environment["CANDID_RESOURCE_PROBE"] else { return nil }
            let process = Process(); process.executableURL = URL(fileURLWithPath: path)
            process.arguments = [String(ProcessInfo.processInfo.processIdentifier)]
            let pipe = Pipe(); process.standardOutput = pipe
            try process.run(); process.waitUntilExit()
            guard process.terminationStatus == 0 else { return nil }
            return try JSONDecoder().decode([String: UInt64].self, from: pipe.fileHandleForReading.readDataToEndOfFile())
        }
        let firstResource = try resourceSample()
        let observedStart = Date()
        let fixture = URL(fileURLWithPath: "/tmp/CandidCaptureChat.app")
        let config = NSWorkspace.OpenConfiguration(); config.activates = true
        let app = try await NSWorkspace.shared.openApplication(at: fixture, configuration: config)
        defer { app.terminate() }
        try await Task.sleep(for: .seconds(1))
        let descriptor = DesktopApplication(bundleID: DesktopSourceConfirmation.fixtureBundleID, name: "Synthetic Chat", pid: app.processIdentifier)
        let root = AXUIElementCreateApplication(app.processIdentifier)
        func attribute(_ node: AXUIElement, _ key: String) -> CFTypeRef? {
            var v: CFTypeRef?; guard AXUIElementCopyAttributeValue(node, key as CFString, &v) == .success else { return nil }; return v
        }
        func find(_ id: String) throws -> AXUIElement {
            var queue = (attribute(root, kAXWindowsAttribute) as? [AXUIElement]) ?? []
            while !queue.isEmpty {
                let n = queue.removeFirst()
                if attribute(n, kAXIdentifierAttribute) as? String == id { return n }
                queue += (attribute(n, kAXChildrenAttribute) as? [AXUIElement]) ?? []
            }
            throw DesktopCaptureError.unavailable
        }
        var options = DesktopContentOptions(); options.inputValueBundleIDs = [descriptor.bundleID]
        var proofState = DesktopSourceConfirmation()
        var records: [DesktopRecord] = [], proofs: [DesktopRecord] = []
        var rawTriggers: [String] = []
        let monitor = DesktopSystemMonitor()
        monitor.setContentOptions(options); monitor.setSamplingPolicy(balanced: true)
        monitor.setTextContentScope(allowedBundleIDs: [descriptor.bundleID]); monitor.setWindowContentScope(allowedBundleIDs: [descriptor.bundleID])
        monitor.setInteractionTriggerSelection(returnEnabled: true, scrollEnabled: true)
        _ = monitor.setInteractionTriggersEnabled(true)
        monitor.start { event in
            if event.application?.bundleID == descriptor.bundleID, let trigger = event.trigger { rawTriggers.append(trigger) }
            guard event.application?.bundleID == descriptor.bundleID, let context = event.context, context.recordEvidence != nil else { return }
            let r = DesktopRecord(kind: .context, bundleID: descriptor.bundleID, trigger: event.trigger, context: context)
            records.append(r); proofs += proofState.confirmations(for: r)
        }
        defer { monitor.stop() }
        try await Task.sleep(for: .seconds(1))
        let input = try find("candid.composer")
        func activateFixture() async throws {
            app.activate(options: [])
            try await Task.sleep(for: .milliseconds(250))
            try #require(NSWorkspace.shared.frontmostApplication?.processIdentifier == app.processIdentifier)
        }
        func draft(_ text: String) async throws {
            try await activateFixture()
            #expect(AXUIElementSetAttributeValue(input, kAXValueAttribute as CFString, text as CFString) == .success)
            try await Task.sleep(for: .seconds(1.8))
        }
        func key(_ flags: CGEventFlags = []) async throws {
            try await activateFixture()
            let down = CGEvent(keyboardEventSource: nil, virtualKey: 36, keyDown: true)!
            down.flags = flags; down.post(tap: .cghidEventTap)
            CGEvent(keyboardEventSource: nil, virtualKey: 36, keyDown: false)?.post(tap: .cghidEventTap)
        }
        func click(_ id: String) async throws {
            try await activateFixture()
            let node = try find(id)
            let pv = try #require(attribute(node, kAXPositionAttribute)), sv = try #require(attribute(node, kAXSizeAttribute))
            var p = CGPoint.zero, s = CGSize.zero
            #expect(AXValueGetValue(unsafeDowncast(pv, to: AXValue.self), .cgPoint, &p))
            #expect(AXValueGetValue(unsafeDowncast(sv, to: AXValue.self), .cgSize, &s))
            let pt = CGPoint(x: p.x+s.width/2, y: p.y+s.height/2)
            CGEvent(mouseEventSource: nil, mouseType: .leftMouseDown, mouseCursorPosition: pt, mouseButton: .left)?.post(tap: .cghidEventTap)
            CGEvent(mouseEventSource: nil, mouseType: .leftMouseUp, mouseCursorPosition: pt, mouseButton: .left)?.post(tap: .cghidEventTap)
        }
        try await draft("SYNTHETIC same")
        try await key(.maskShift); try await Task.sleep(for: .seconds(2)); #expect(proofs.isEmpty)
        try await click("clear"); try await Task.sleep(for: .seconds(2)); #expect(proofs.isEmpty)
        try await draft("SYNTHETIC same"); try await key()
        try await Task.sleep(for: .seconds(2)); #expect(proofs.count == 1)
        try await draft("SYNTHETIC same"); try await click("send")
        try await Task.sleep(for: .seconds(2)); #expect(proofs.count == 2)
        try await click("failure"); try await draft("SYNTHETIC failed"); try await key()
        try await Task.sleep(for: .seconds(2)); #expect(proofs.count == 2)
        #expect(proofs.allSatisfy { $0.context?.recordEvidence?.confirmation?.text == "SYNTHETIC same" })
        try await click("fast"); try await Task.sleep(for: .seconds(3))
        let fast = records.last?.context?.recordEvidence?.reading ?? []
        #expect(fast.filter { $0.occurrence?.contains("incoming.fast") == true }.count == 10)
        #expect(fast.contains { $0.text == "短" })
        let startCount = records.count
        try await click("stream"); try await Task.sleep(for: .seconds(27))
        #expect(records.count - startCount >= 3)
        #expect(records.dropFirst(startCount).contains { $0.context?.readableText?.contains(" 60") == true })
        let continuousCheckpoints = records.count - startCount
        try await activateFixture()
        let scroll = CGEvent(scrollWheelEvent2Source: nil, units: .line, wheelCount: 1, wheel1: -12, wheel2: 0, wheel3: 0)!
        scroll.location = CGPoint(x: 450, y: 450); scroll.post(tap: .cghidEventTap)
        try await Task.sleep(for: .seconds(2))
        #expect(rawTriggers.contains("scroll"))
        try await activateFixture()
        let frame = try await DesktopCaptureProvider().captureFrontmost(allowedBundleIDs: [descriptor.bundleID], windowCompatibilityBundleIDs: [], axTextBundleIDs: [descriptor.bundleID], includeAXFocus: true)
        #expect(frame.windowIdentity != nil && frame.width > 0 && frame.height > 0)
        let ocr = try await DesktopOCR().recognize(frame.data)
        #expect(ocr.text.contains("合成"))
        try frame.data.write(to: URL(fileURLWithPath: "/tmp/candid-chat-capture.jpg"))
        options.inputValueBundleIDs = []; monitor.setContentOptions(options)
        try await Task.sleep(for: .seconds(1))
        let countAtRevocation = records.count
        try await draft("SYNTHETIC disabled")
        #expect(records.dropFirst(countAtRevocation).allSatisfy { $0.context?.recordEvidence?.input_value == nil })
        let end = Date()
        let lastResource = try resourceSample()
        func ms(_ date: Date) -> Int64 { Int64(date.timeIntervalSince1970 * 1000) }
        let span: [String: Any] = ["start_ms": ms(observedStart), "end_ms": ms(end)]
        let measuredInputs: [[String: Any]] = proofs.enumerated().map { i, r in
            ["id": "send-\(i)", "at_ms": ms(r.timestamp), "sent": true, "expected_text": "SYNTHETIC same"]
        }
        let inputRecords: [[String: Any]] = proofs.enumerated().map { i, r in
            ["session_id": "send-\(i)", "visit_id": "synthetic-chat", "at_ms": ms(r.timestamp), "text": r.context?.recordEvidence?.confirmation?.text ?? "", "truncated": false]
        }
        // Independent native assertions above establish the two sends. Resource
        // and search measurements remain unknown, rather than reporting zero.
        var resources: Any = NSNull()
        if let before = firstResource, let after = lastResource {
            resources = [["span": span, "cpu_ms": ((after["cpu_ns"] ?? 0) - (before["cpu_ns"] ?? 0)) / 1_000_000,
                "wakeups": (after["wakeups"] ?? 0) - (before["wakeups"] ?? 0)]] as [[String: Any]]
        }
        let dataset: [String: Any] = ["format": "candid-measurement/1", "synthetic": true,
            "observed": [span], "context_items": [], "visits": [["id": "synthetic-chat", "object_id": "synthetic", "active": [span]]],
            "effective": NSNull(), "covered": NSNull(), "inputs": measuredInputs, "input_records": inputRecords,
            "text_records": records.compactMap { r -> [String: Any]? in
                guard let text = r.context?.readableText, !text.isEmpty else { return nil }
                return ["at_ms": ms(r.timestamp), "visit_id": "synthetic-chat", "text": text]
            }, "shots": [["at_ms": ms(frame.capturedAt), "bytes": frame.data.count, "blob_id": DesktopEventMapping.sha256(frame.data)]], "resources": resources, "searchable": NSNull(), "counts": ["desktop_events": records.count]]
        let scenarioReport: [String: Any] = ["synthetic": true, "scope": "native fixture AX and CGEvent; one window; two sends, ten rapid incoming messages, 24-second generation",
            "expected_sends": 2, "confirmed_sends": proofs.count, "missed_sends": max(0, 2-proofs.count),
            "false_sends": max(0, proofs.count-2), "distinct_send_identities": Set(proofs.compactMap { $0.context?.recordEvidence?.confirmation?.message_id }).count,
            "expected_fast_messages": 10, "observed_fast_messages": fast.filter { $0.occurrence?.contains("incoming.fast") == true }.count,
            "continuous_checkpoints": continuousCheckpoints, "native_scroll_signal": rawTriggers.contains("scroll"),
            "screenshot_bytes": frame.data.count, "local_ocr_has_synthetic_text": ocr.text.contains("合成"),
            "resources_scope": "test helper process including test overhead; excludes fixture and WindowServer", "search_latency": NSNull()]
        try JSONSerialization.data(withJSONObject: scenarioReport, options: [.prettyPrinted, .sortedKeys]).write(to: URL(fileURLWithPath: "/tmp/candid-chat-scenarios.json"))
        try JSONSerialization.data(withJSONObject: dataset, options: [.prettyPrinted, .sortedKeys]).write(to: URL(fileURLWithPath: "/tmp/candid-chat-measurement.json"))
        let bytes = try records.reduce(0) { try $0 + JSONEncoder().encode($1).count }
        let report = "synthetic native AX/CGEvent: records=\(records.count), confirmed=\(proofs.count), encodedBytes=\(bytes), continuousCheckpoints=\(continuousCheckpoints); CPU/wakeups scope: native test helper only, fixture/WindowServer excluded; search latency unmeasured\n"
        try report.write(toFile: "/tmp/candid-chat-live-result.txt", atomically: true, encoding: .utf8)
        print(report)
    }
}
