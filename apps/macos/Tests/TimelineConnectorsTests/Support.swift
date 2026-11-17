import Foundation
@testable import TimelineConnectors

/// Temp-directory helper. All transcript content in tests is synthetic and labelled `SYNTHETIC`.
final class TempDir {
    let url: URL
    init() {
        url = FileManager.default.temporaryDirectory.appendingPathComponent("tlc-\(UUID().uuidString)", isDirectory: true)
        try! FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    }
    deinit { try? FileManager.default.removeItem(at: url) }

    @discardableResult
    func write(_ rel: String, lines: [String], trailingNewline: Bool = true, age: TimeInterval = 3600) -> URL {
        let f = url.appendingPathComponent(rel)
        try! FileManager.default.createDirectory(at: f.deletingLastPathComponent(), withIntermediateDirectories: true)
        let text = lines.joined(separator: "\n") + (trailingNewline ? "\n" : "")
        try! text.write(to: f, atomically: true, encoding: .utf8)
        try! FileManager.default.setAttributes([.modificationDate: Date().addingTimeInterval(-age)], ofItemAtPath: f.path)
        return f
    }
    func append(_ rel: String, lines: [String], trailingNewline: Bool = true, age: TimeInterval = 3600) {
        let f = url.appendingPathComponent(rel)
        let h = try! FileHandle(forWritingTo: f)
        try! h.seekToEnd()
        try! h.write(contentsOf: Data((lines.joined(separator: "\n") + (trailingNewline ? "\n" : "")).utf8))
        try! h.close()
        try! FileManager.default.setAttributes([.modificationDate: Date().addingTimeInterval(-age)], ofItemAtPath: f.path)
    }
}

func j(_ o: [String: Any]) -> String {
    String(decoding: try! JSONSerialization.data(withJSONObject: o, options: [.sortedKeys]), as: UTF8.self)
}

extension SyncResult {
    var events: [ConnectorEvent] { batches.flatMap(\.events) }
    var threads: [ConnectorThread] { batches.flatMap(\.threads) }
}
