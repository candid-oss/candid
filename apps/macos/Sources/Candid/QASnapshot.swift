import AppKit
import Foundation

/// QA only, enabled by `--qa-snapshot-dir DIR`. Polls DIR/request for one line:
/// `shot NAME` renders this app's own window into DIR/NAME.png (no screen
/// capture permission involved); other lines are posted as `qaCommand` for the
/// views to apply (`page timeline`, `mode rhythm`, `open`, `close`, `step 1`, `search 预算`, `settings data`).
@MainActor enum QASnapshot {
    static let command = Notification.Name("Candid.qaCommand")
    /// A QA instance renders while hidden; it must not pause timeline loading.
    static var isActive: Bool { timer != nil }
    private static var timer: Timer?

    static func startIfRequested(arguments: [String] = ProcessInfo.processInfo.arguments) {
        guard let index = arguments.firstIndex(of: "--qa-snapshot-dir"), index + 1 < arguments.count else { return }
        let directory = URL(fileURLWithPath: arguments[index + 1], isDirectory: true)
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        timer = Timer.scheduledTimer(withTimeInterval: 0.4, repeats: true) { _ in
            MainActor.assumeIsolated { poll(directory) }
        }
    }

    private static func poll(_ directory: URL) {
        let request = directory.appendingPathComponent("request")
        guard let text = try? String(contentsOf: request, encoding: .utf8) else { return }
        try? FileManager.default.removeItem(at: request)
        for line in text.split(separator: "\n").map({ $0.trimmingCharacters(in: .whitespaces) }) where !line.isEmpty {
            if line.hasPrefix("shot ") {
                let name = String(line.dropFirst(5)).replacingOccurrences(of: "/", with: "_")
                write(directory.appendingPathComponent(name + ".png"))
            } else {
                NotificationCenter.default.post(name: command, object: line)
            }
        }
    }

    private static func write(_ url: URL) {
        guard let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil && $0.frame.width > 600 }),
              let view = window.contentView?.superview ?? window.contentView,
              let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
        view.cacheDisplay(in: view.bounds, to: rep)
        try? rep.representation(using: .png, properties: [:])?.write(to: url)
    }
}
