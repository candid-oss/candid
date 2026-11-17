import CoreGraphics
import DesktopCore
import CandidClient
import Foundation
import ImageIO
import UniformTypeIdentifiers
import Darwin

/// Offline diagnostics. Never requests permissions or reads screen pixels.
@main
struct DesktopDiagnostics {
    static func main() async {
        do { try await run() }
        catch {
            let data = Data(("DesktopDiagnostics: " + error.localizedDescription + "\n").utf8)
            try? FileHandle.standardError.write(contentsOf: data)
            exit(1)
        }
    }

    private static func run() async throws {
        let arguments = Array(CommandLine.arguments.dropFirst())
        if arguments.first == "judge" {
            guard arguments.count == 1 else { throw DiagnosticError.usage }
            let input = try JSONDecoder().decode(DesktopRecognitionRouting.Input.self,
                from: FileHandle.standardInput.readDataToEndOfFile())
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
            var output = try encoder.encode(DesktopRecognitionRouting.decide(input))
            output.append(0x0A)
            try FileHandle.standardOutput.write(contentsOf: output)
            return
        }
        guard let command = arguments.first, ["write", "verify", "stress", "recognition-energy"].contains(command),
              let directoryIndex = arguments.firstIndex(of: "--data-dir"), directoryIndex + 1 < arguments.count else {
            throw DiagnosticError.usage
        }
        let path = arguments[directoryIndex + 1]
        guard path.hasPrefix("/") else { throw DiagnosticError.usage }
        let root = URL(fileURLWithPath: path, isDirectory: true).standardizedFileURL
        let count: Int
        if let countIndex = arguments.firstIndex(of: "--count") {
            guard countIndex + 1 < arguments.count, let parsed = Int(arguments[countIndex + 1]), (1...100_000).contains(parsed) else { throw DiagnosticError.usage }
            count = parsed
        } else { count = command == "recognition-energy" ? 6 : 100 }
        if command == "recognition-energy", count > 100 { throw DiagnosticError.usage }
        try prepareSyntheticDirectory(root, allowCreate: command != "verify")
        if command == "recognition-energy" {
            try await RecognitionEnergyProbe.run(root: root, iterations: count)
            return
        }
        let outbox = try DesktopOutbox(directory: root.appendingPathComponent("outbox", isDirectory: true))
        switch command {
        case "write":
            // A receipt means the record is durable in the outbox, the capture's commit point.
            _ = try await write(outbox: outbox, count: count, receipts: true)
        case "verify":
            try await withCore(root) { channel in try await verify(outbox: outbox, channel: channel) }
        case "stress":
            let started = ProcessInfo.processInfo.systemUptime
            let committed = try await write(outbox: outbox, count: count, receipts: false)
            let elapsed = ProcessInfo.processInfo.systemUptime - started
            try await withCore(root) { channel in
                let deliveryStarted = ProcessInfo.processInfo.systemUptime
                try await drain(outbox: outbox, channel: channel)
                let delivery = ProcessInfo.processInfo.systemUptime - deliveryStarted
                let queryStarted = ProcessInfo.processInfo.systemUptime
                let page: AppActivities = try await channel.app("app_activities", ["start_ms": .int(0), "end_ms": .int(Int64(Date().timeIntervalSince1970 * 1000) + 86_400_000), "limit": .int(100)])
                let queryElapsed = ProcessInfo.processInfo.systemUptime - queryStarted
                try emit([
                    "mode": "stress", "fixture": "synthetic_offscreen_jpeg", "input_events": count,
                    "committed_records": committed, "duration_seconds": elapsed, "delivery_seconds": delivery,
                    "first_page_activities": page.activities.count, "first_page_seconds": queryElapsed,
                    "record_count": page.stats.record_count, "screenshot_count": page.stats.screenshot_count,
                    "storage_bytes": page.stats.storage_bytes, "data_dir": root.path,
                    "durability_scope": "process_exit_and_reopen_not_power_cut"
                ])
            }
        default: throw DiagnosticError.usage
        }
    }

    /// Runs a throwaway core on `<data-dir>/timeline` (`CANDID_CORE_BIN`, else `candid` next to this tool).
    @MainActor private static func withCore(_ root: URL, _ body: (LocalChannel) async throws -> Void) async throws {
        let environment = ProcessInfo.processInfo.environment
        let binary = environment["CANDID_CORE_BIN"].flatMap { $0.isEmpty ? nil : URL(fileURLWithPath: $0) }
            ?? URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent().appendingPathComponent("candid")
        let supervisor = CoreSupervisor(launch: CoreLaunch(executable: binary, root: root.appendingPathComponent("timeline", isDirectory: true),
                                                           runDirectory: root.appendingPathComponent("run", isDirectory: true)))
        supervisor.start()
        defer { supervisor.stop() }
        let deadline = Date().addingTimeInterval(30)
        while Date() < deadline {
            if case .running = supervisor.state { break }
            if case .failed(let message) = supervisor.state { throw DiagnosticError.core(message) }
            try await Task.sleep(for: .milliseconds(50))
        }
        guard case .running = supervisor.state else { throw DiagnosticError.core("核心没有在 30 秒内就绪") }
        _ = try await supervisor.channel.cutover()
        try await body(supervisor.channel)
    }

    private static func drain(outbox: DesktopOutbox, channel: LocalChannel) async throws {
        let source = try await DesktopDelivery.registerSource(channel)
        while true {
            let entries = await outbox.peek(64)
            if entries.isEmpty { return }
            let (done, note) = try await DesktopDelivery.deliver(entries, channel: channel, sourceID: source)
            if let note { throw DiagnosticError.core(note) }
            await outbox.remove(done)
        }
    }

    private static func prepareSyntheticDirectory(_ root: URL, allowCreate: Bool) throws {
        let manager = FileManager.default
        let marker = root.appendingPathComponent(".desktop-diagnostics-synthetic")
        if manager.fileExists(atPath: root.path) {
            let files = try manager.contentsOfDirectory(at: root, includingPropertiesForKeys: nil)
            if manager.fileExists(atPath: marker.path) {
                guard try String(contentsOf: marker, encoding: .utf8) == "synthetic-desktop-diagnostics/v1\n" else { throw DiagnosticError.realDataForbidden }
            } else {
                guard allowCreate, files.isEmpty else { throw DiagnosticError.realDataForbidden }
                try Data("synthetic-desktop-diagnostics/v1\n".utf8).write(to: marker, options: .withoutOverwriting)
            }
        } else {
            guard allowCreate else { throw DiagnosticError.realDataForbidden }
            try manager.createDirectory(at: root, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            try Data("synthetic-desktop-diagnostics/v1\n".utf8).write(to: marker, options: .withoutOverwriting)
        }
    }

    private static func write(outbox: DesktopOutbox, count: Int, receipts: Bool) async throws -> Int {
        let run = "synthetic-" + UUID().uuidString
        var committed = 0
        for index in 0..<count {
            let event = DesktopRecord(kind: .application, appName: "Synthetic Editor", bundleID: "test.synthetic.diagnostics", windowTitle: "Synthetic document \(index)", detail: "Offline fixture; no screen, microphone, network or personal content was accessed.", runID: run)
            try await outbox.append(.ingest([try DesktopEventMapping.event(for: event).event]))
            committed += 1
            if receipts { try emit(["id": event.id, "kind": event.kind.rawValue]) }
            if index % 4 == 0 {
                let image = try syntheticJPEG(index: index)
                var record = DesktopRecord(kind: .screenshot, appName: "Synthetic Editor", bundleID: "test.synthetic.diagnostics", windowTitle: "Synthetic offscreen frame \(index)", detail: "320 × 180 JPEG generated offscreen from colored rectangles; no display pixels.", imageWidth: 320, imageHeight: 180, parentID: event.id, runID: run)
                record.imageHash = DesktopEventMapping.sha256(image)
                try await outbox.append(.ingest([try DesktopEventMapping.event(for: record).event]), files: [record.imageHash!: (image, "jpg")])
                committed += 1
                if receipts { try emit(["id": record.id, "kind": record.kind.rawValue]) }
            }
        }
        return committed
    }

    private static func syntheticJPEG(index: Int) throws -> Data {
        try autoreleasepool {
            let colorSpace = CGColorSpaceCreateDeviceRGB()
            guard let context = CGContext(data: nil, width: 320, height: 180, bitsPerComponent: 8, bytesPerRow: 320 * 4, space: colorSpace, bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else { throw DiagnosticError.imageFailed }
            context.setFillColor(CGColor(red: 0.96, green: 0.97, blue: 0.99, alpha: 1)); context.fill(CGRect(x: 0, y: 0, width: 320, height: 180))
            context.setFillColor(CGColor(red: CGFloat(index % 17) / 32 + 0.1, green: 0.35, blue: 0.75, alpha: 1)); context.fill(CGRect(x: 0, y: 148, width: 320, height: 32))
            for row in 0..<6 {
                context.setFillColor(CGColor(gray: 0.75 + CGFloat(row) / 40, alpha: 1))
                context.fill(CGRect(x: 20, y: 20 + row * 19, width: 160 + ((index + row * 13) % 120), height: 7))
            }
            guard let image = context.makeImage() else { throw DiagnosticError.imageFailed }
            let data = NSMutableData()
            guard let destination = CGImageDestinationCreateWithData(data, UTType.jpeg.identifier as CFString, 1, nil) else { throw DiagnosticError.imageFailed }
            CGImageDestinationAddImage(destination, image, [kCGImageDestinationLossyCompressionQuality: 0.8] as CFDictionary)
            guard CGImageDestinationFinalize(destination) else { throw DiagnosticError.imageFailed }
            return data as Data
        }
    }

    /// Delivers whatever the outbox still holds, then reads every activity back and checks each screenshot's file.
    private static func verify(outbox: DesktopOutbox, channel: LocalChannel) async throws {
        try await drain(outbox: outbox, channel: channel)
        let day: AppActivities = try await channel.app("app_activities", ["start_ms": .int(0), "end_ms": .int(Int64(Date().timeIntervalSince1970 * 1000) + 86_400_000), "limit": .int(1000)])
        var ids: [String] = []; var hashes = Set<String>()
        for summary in day.activities {
            struct Answer: Decodable { var activity: AppActivity? }
            let detail: Answer = try await channel.app("app_activity", ["id": .string(summary.anchor.id), "limit": .int(10_000)])
            guard let activity = detail.activity else { throw DiagnosticError.inconsistentRecords }
            for record in activity.observations + activity.screenshots + activity.issues {
                ids.append(record.id)
                if let hash = record.image_hash {
                    guard let path = record.image_path, DesktopEventMapping.sha256(try Data(contentsOf: URL(fileURLWithPath: path))) == hash else { throw DiagnosticError.inconsistentRecords }
                    hashes.insert(hash)
                }
            }
        }
        guard ids.count == Set(ids).count, ids.count == day.stats.record_count else { throw DiagnosticError.inconsistentRecords }
        try emit([
            "mode": "verify", "fixture": "synthetic_offscreen_jpeg", "ids": ids,
            "record_count": day.stats.record_count, "screenshot_count": day.stats.screenshot_count,
            "verified_unique_images": hashes.count, "storage_bytes": day.stats.storage_bytes,
            "durability_scope": "process_exit_and_reopen_not_power_cut"
        ])
    }

    /// One unbuffered pipe write includes the full receipt and newline, after commit.
    private static func emit(_ object: [String: Any]) throws {
        var data = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
        data.append(0x0A)
        try FileHandle.standardOutput.write(contentsOf: data)
    }
}

private enum DiagnosticError: Error, LocalizedError {
    case usage, realDataForbidden, imageFailed, inconsistentRecords, core(String)
    var errorDescription: String? {
        switch self {
        case .usage: "Usage: DesktopDiagnostics judge < frame.json | DesktopDiagnostics write|verify|stress|recognition-energy --data-dir /absolute/synthetic/path [--count N]; recognition-energy accepts at most 100 iterations"
        case .realDataForbidden: "Diagnostics accepts only a new empty directory or a marked synthetic fixture directory; existing user data is forbidden."
        case .imageFailed: "Synthetic offscreen JPEG encoding failed."
        case .inconsistentRecords: "Stored records or pagination verification is inconsistent."
        case .core(let detail): "Timeline core: \(detail)"
        }
    }
}
