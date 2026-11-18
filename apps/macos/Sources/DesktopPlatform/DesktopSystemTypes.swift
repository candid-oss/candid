import AppKit
import CandidLocalization
import ApplicationServices
import CoreGraphics
import DesktopCore
import Foundation

public struct DesktopSystemSnapshot: Sendable, Equatable {
    public let rows: [DesktopInfoRow]
    public let screenPermission: Bool
    public let accessibilityPermission: Bool
    public let thermalBlocked: Bool
    public let lowPower: Bool
    public let onBattery: Bool?
    public init(rows: [DesktopInfoRow], screenPermission: Bool, accessibilityPermission: Bool, thermalBlocked: Bool, lowPower: Bool, onBattery: Bool? = nil) {
        self.rows = rows; self.screenPermission = screenPermission; self.accessibilityPermission = accessibilityPermission
        self.thermalBlocked = thermalBlocked; self.lowPower = lowPower; self.onBattery = onBattery
    }
}

public struct DesktopSystemEvent: Sendable {
    public enum Kind: Sendable, Equatable { case application, window, context, blocked, resumed, system }
    public let kind: Kind
    public let application: DesktopApplication?
    public let title: String
    public let detail: String
    public let trigger: String?
    public let context: DesktopContextSnapshot?
    public let windowIdentity: UInt?
    public init(kind: Kind, application: DesktopApplication? = nil, title: String = "", detail: String = "", trigger: String? = nil, context: DesktopContextSnapshot? = nil, windowIdentity: UInt? = nil) {
        self.kind = kind; self.application = application; self.title = title; self.detail = detail
        self.trigger = trigger; self.context = context; self.windowIdentity = windowIdentity
    }
}

public struct DesktopContextReading: Sendable {
    public let application: DesktopApplication
    public let title: String
    public let context: DesktopContextSnapshot
    public let windowIdentity: UInt?
    public init(application: DesktopApplication, title: String, context: DesktopContextSnapshot, windowIdentity: UInt? = nil) {
        self.application = application; self.title = title; self.context = context; self.windowIdentity = windowIdentity
    }
}

public struct CapturedWindow: Sendable {
    public let data: Data
    public let application: DesktopApplication
    public let title: String
    public let width: Int
    public let height: Int
    public let capturedAt: Date
    public let windowIdentity: UInt?
    public let axEvidence: DesktopAXEvidence?
    /// The uncompressed frame, only when raw-frame delivery is on (OCR before compression).
    public let frame: DesktopRawFrame?
    public init(data: Data, application: DesktopApplication, title: String, width: Int, height: Int, capturedAt: Date,
                windowIdentity: UInt? = nil, axEvidence: DesktopAXEvidence? = nil, frame: DesktopRawFrame? = nil) {
        self.data = data; self.application = application; self.title = title; self.width = width; self.height = height; self.capturedAt = capturedAt; self.windowIdentity = windowIdentity
        self.axEvidence = axEvidence; self.frame = frame
    }
}

/// CGImage is immutable; this only carries it across actors.
public struct DesktopRawFrame: @unchecked Sendable {
    public let image: CGImage
    public init(_ image: CGImage) { self.image = image }
}

public enum DesktopCaptureError: String, Error, Sendable, LocalizedError {
    case screenPermissionDenied, accessibilityPermissionDenied, inactiveSession, excludedApplication
    case noFocusedWindow, noVerifiedFocus, noMatchingWindow, ambiguousWindow, focusChanged, busy, timedOut, cancelled, imageTooLarge, unavailable, protectedContent, contentChanging
    public var shouldBackOffCapture: Bool { self == .timedOut || self == .unavailable }
    public var errorDescription: String? {
        switch self {
        case .screenPermissionDenied: L("屏幕录制权限尚未允许，请在系统设置中允许后重新检查。")
        case .accessibilityPermissionDenied: L("需要辅助功能权限来确认当前窗口；应用活动仍可记录。")
        case .inactiveSession: L("当前会话不活跃或状态无法确认，已跳过截图。")
        case .excludedApplication: L("当前应用未在截图允许范围内。")
        case .noFocusedWindow: L("无法读取当前窗口，已保留应用事件。")
        case .noVerifiedFocus: L("无法确认窗口内的控件焦点。可在「设置 → 应用」为此应用开启截图兼容。")
        case .noMatchingWindow: L("未找到与当前焦点对应的可截图窗口。")
        case .ambiguousWindow: L("存在多个同位置窗口，无法确定截图对象。")
        case .focusChanged: L("截图期间窗口发生变化，结果已丢弃。")
        case .busy: L("上一项系统截图请求尚未结束。")
        case .timedOut: L("系统截图请求超时；等待底层请求释放后才能继续。")
        case .cancelled: L("截图已取消。")
        case .imageTooLarge: L("截图超过单张大小上限，已跳过。")
        case .unavailable: L("系统暂时无法提供此窗口的截图。")
        case .protectedContent: L("当前焦点包含密码或受保护内容，已跳过采集。")
        case .contentChanging: L("文字仍在输入，等待内容稳定后再截图。")
        }
    }
}

/// Pure target selection, shared with the native adapter's synthetic tests.
public struct DesktopWindowCandidate: Sendable, Equatable {
    public let id: UInt32
    public let pid: Int32
    public let title: String
    public let frame: CGRect
    public let isOnScreen: Bool
    public init(id: UInt32, pid: Int32, title: String, frame: CGRect, isOnScreen: Bool = true) {
        self.id = id; self.pid = pid; self.title = title; self.frame = frame; self.isOnScreen = isOnScreen
    }
}

public enum DesktopWindowMatcher {
    /// CGWindowList and AX use global top-left logical coordinates. Titles are
    /// different providers' labels, not identity. The WindowServer ID is the
    /// documented bridge to SCK, after unique PID/bounds correspondence to AX.
    public static func resolveWindowServer(pid: Int32, frame: CGRect, candidates: [DesktopWindowCandidate], tolerance: CGFloat = 3) throws -> UInt32 {
        guard valid(frame) else { throw DesktopCaptureError.noFocusedWindow }
        let matches = candidates.filter { $0.pid == pid && $0.isOnScreen && close($0.frame, frame, tolerance: tolerance) }
        guard !matches.isEmpty else { throw DesktopCaptureError.noMatchingWindow }
        guard matches.count == 1 else { throw DesktopCaptureError.ambiguousWindow }
        return matches[0].id
    }
    public static func match(pid: Int32, title: String, frame: CGRect, candidates: [DesktopWindowCandidate], tolerance: CGFloat = 3, windowServerCandidates: [DesktopWindowCandidate]? = nil) throws -> UInt32 {
        guard valid(frame) else { throw DesktopCaptureError.noFocusedWindow }
        if let server = windowServerCandidates {
            let id = try resolveWindowServer(pid: pid, frame: frame, candidates: server, tolerance: tolerance)
            let exact = candidates.filter { $0.id == id && $0.pid == pid && $0.isOnScreen }
            guard !exact.isEmpty else { throw DesktopCaptureError.noMatchingWindow }
            guard exact.count == 1 else { throw DesktopCaptureError.ambiguousWindow }
            return id
        }
        let matches = candidates.filter { $0.pid == pid && $0.isOnScreen && close($0.frame, frame, tolerance: tolerance) && (title.isEmpty || $0.title.isEmpty || $0.title == title) }
        guard !matches.isEmpty else { throw DesktopCaptureError.noMatchingWindow }
        guard matches.count == 1 else { throw DesktopCaptureError.ambiguousWindow }
        return matches[0].id
    }
    private static func valid(_ frame: CGRect) -> Bool {
        !frame.isEmpty && !frame.isNull && [frame.minX, frame.minY, frame.width, frame.height].allSatisfy(\.isFinite)
    }
    private static func close(_ a: CGRect, _ b: CGRect, tolerance: CGFloat) -> Bool {
        valid(a) && abs(a.minX - b.minX) <= tolerance && abs(a.minY - b.minY) <= tolerance && abs(a.width - b.width) <= tolerance && abs(a.height - b.height) <= tolerance
    }
    public static func outputSize(width: CGFloat, height: CGFloat, scale: CGFloat = 1, maximumEdge: CGFloat = 1920) -> (width: Int, height: Int) {
        guard width.isFinite, height.isFinite, scale.isFinite, width > 0, height > 0, scale > 0, maximumEdge.isFinite, maximumEdge > 0 else { return (1, 1) }
        let w = width * min(scale, 4), h = height * min(scale, 4)
        let factor = min(1, maximumEdge / max(w, h))
        return (max(1, Int((w * factor).rounded())), max(1, Int((h * factor).rounded())))
    }
}

@MainActor
enum DesktopSessionSafety {
    static var enabled = false
    static var blocked = true
    static var epoch: UInt64 = 0

    static func currentConsoleIsUsable() -> Bool {
        guard let dictionary = CGSessionCopyCurrentDictionary() as? [String: Any],
              dictionary[kCGSessionOnConsoleKey as String] as? Bool == true,
              dictionary[kCGSessionLoginDoneKey as String] as? Bool == true else { return false }
        // Supplemental WindowServer key: not a stable public lock-state contract.
        // A true value closes capture; notification-based lock protection is also
        // mandatory and the full lock/sleep matrix remains a release test.
        if dictionary["CGSSessionScreenIsLocked"] as? Bool == true { return false }
        guard let app = NSWorkspace.shared.frontmostApplication, app.activationPolicy == .regular,
              let identifier = app.bundleIdentifier, !identifier.isEmpty,
              identifier != "com.apple.loginwindow", identifier != "com.apple.ScreenSaver.Engine" else { return false }
        return true
    }

    static var canCapture: Bool { enabled && !blocked && currentConsoleIsUsable() }
    static func update(blocked: Bool) { self.blocked = blocked; epoch &+= 1 }
}

@MainActor
enum DesktopFocusReader {
    struct Focus: Sendable {
        let application: DesktopApplication
        let title: String
        let frame: CGRect
        let context: DesktopContextSnapshot
        let focusIdentity: UInt
        let windowIdentity: UInt
        let controlFocusAbsent: Bool
    }
    static func application(_ app: NSRunningApplication?) -> DesktopApplication? {
        guard let app, app.activationPolicy == .regular, let bundleID = app.bundleIdentifier, !bundleID.isEmpty else { return nil }
        return DesktopApplication(bundleID: bundleID, name: app.localizedName ?? bundleID, pid: app.processIdentifier, bundlePath: app.bundleURL?.path)
    }
    static func focusedWindow() async throws -> Focus {
        guard AXIsProcessTrusted() else { throw DesktopCaptureError.accessibilityPermissionDenied }
        guard let application = application(NSWorkspace.shared.frontmostApplication) else { throw DesktopCaptureError.noFocusedWindow }
        let result = try await DesktopAXWorker.shared.read(application: application, minimalMetadata: true)
        guard NSWorkspace.shared.frontmostApplication?.processIdentifier == application.pid else { throw DesktopCaptureError.focusChanged }
        return result
    }
}
