import Foundation

/// The day at a glance, written by the app and read by the widget. Holds only
/// aggregates and the current window title: no screenshots, no window text.
public struct WidgetSnapshot: Codable, Sendable, Equatable {
    public struct App: Codable, Sendable, Equatable {
        public var name: String
        public var bundleID: String
        public var seconds: Double
        public init(name: String, bundleID: String, seconds: Double) { self.name = name; self.bundleID = bundleID; self.seconds = seconds }
    }

    public struct Current: Codable, Sendable, Equatable {
        public var title: String
        public var appName: String
        public var bundleID: String
        public var since: Date
        public init(title: String, appName: String, bundleID: String, since: Date) {
            self.title = title; self.appName = appName; self.bundleID = bundleID; self.since = since
        }
    }

    public static let version = 1
    public var version: Int
    public var generatedAt: Date
    public var day: Date
    public var isDemo: Bool
    public var isRecording: Bool
    public var isMicRecording: Bool
    public var recordedSeconds: Double
    public var appCount: Int
    public var recordingCount: Int
    public var topApps: [App]
    public var current: Current?
    /// Recorded minutes in each of the 24 hours of `day`.
    public var hours: [Double]

    public init(generatedAt: Date = Date(), day: Date, isDemo: Bool, isRecording: Bool, isMicRecording: Bool, recordedSeconds: Double,
                appCount: Int, recordingCount: Int, topApps: [App], current: Current?, hours: [Double]) {
        version = Self.version
        self.generatedAt = generatedAt; self.day = day; self.isDemo = isDemo; self.isRecording = isRecording
        self.isMicRecording = isMicRecording; self.recordedSeconds = recordedSeconds; self.appCount = appCount
        self.recordingCount = recordingCount; self.topApps = topApps; self.current = current
        self.hours = hours.count == 24 ? hours : Array(repeating: 0, count: 24)
    }

    /// Same content apart from when it was written.
    public func sameContent(as other: WidgetSnapshot) -> Bool {
        var copy = other; copy.generatedAt = generatedAt; return copy == self
    }

    /// `~/Library/Application Support/Candid/widget/status.json` of the real
    /// user home, also when read from inside the widget's sandbox container.
    public static var location: URL {
        let home = getpwuid(getuid()).flatMap { $0.pointee.pw_dir.map { String(cString: $0) } } ?? NSHomeDirectory()
        return URL(fileURLWithPath: home, isDirectory: true)
            .appendingPathComponent("Library/Application Support/Candid/widget/status.json")
    }

    public static func read() -> WidgetSnapshot? {
        guard let data = try? Data(contentsOf: location) else { return nil }
        let decoder = JSONDecoder(); decoder.dateDecodingStrategy = .iso8601
        guard let snapshot = try? decoder.decode(WidgetSnapshot.self, from: data), snapshot.version == version else { return nil }
        return snapshot
    }

    public func write() throws {
        let encoder = JSONEncoder(); encoder.dateEncodingStrategy = .iso8601
        let data = try encoder.encode(self)
        let directory = Self.location.deletingLastPathComponent()
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        try data.write(to: Self.location, options: [.atomic])
    }
}
