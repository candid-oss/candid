import AppKit
import CandidLocalization
import Foundation
import ServiceManagement

/// `candid://` links for Raycast, Alfred and scripts:
/// `candid://pause`, `candid://pause?minutes=30`, `candid://resume`, `candid://open?date=2026-11-25`,
/// `candid://search?q=…`. Registered in Info.plist by `script/build_and_run.sh`.
enum CandidURL: Equatable {
    case pause(minutes: Int?)
    case resume
    case open(date: Date?)
    case search(String)

    static func parse(_ url: URL, calendar: Calendar = .current) -> CandidURL? {
        guard url.scheme?.lowercased() == "candid", let components = URLComponents(url: url, resolvingAgainstBaseURL: false) else { return nil }
        // `candid://pause` puts the command in the host; `candid:pause` and `candid:///pause` in the path.
        let command = (components.host?.nonEmpty ?? components.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))).lowercased()
        let query = Dictionary((components.queryItems ?? []).compactMap { item in item.value.map { (item.name.lowercased(), $0) } },
                               uniquingKeysWith: { first, _ in first })
        switch command {
        case "pause":
            guard let raw = query["minutes"] else { return .pause(minutes: nil) }
            guard let minutes = Int(raw), (1...24 * 60).contains(minutes) else { return nil }
            return .pause(minutes: minutes)
        case "resume", "start": return .resume
        case "open":
            guard let raw = query["date"] else { return .open(date: nil) }
            guard let date = day(raw, calendar: calendar) else { return nil }
            return .open(date: date)
        case "search":
            guard let text = query["q"]?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty else { return nil }
            return .search(String(text.prefix(200)))
        default: return nil
        }
    }

    /// `today`, `yesterday` or `yyyy-MM-dd`, as the start of that local day.
    static func day(_ text: String, calendar: Calendar = .current, now: Date = Date()) -> Date? {
        switch text.lowercased() {
        case "today": return calendar.startOfDay(for: now)
        case "yesterday": return calendar.date(byAdding: .day, value: -1, to: calendar.startOfDay(for: now))
        default:
            let parts = text.split(separator: "-").compactMap { Int($0) }
            guard parts.count == 3, (1...12).contains(parts[1]), (1...31).contains(parts[2]) else { return nil }
            let components = DateComponents(year: parts[0], month: parts[1], day: parts[2])
            guard let date = calendar.date(from: components), calendar.component(.day, from: date) == parts[2] else { return nil }
            return date
        }
    }
}

extension Notification.Name {
    /// Object: the `Date` of the day to show in the timeline.
    static let candidOpenDay = Notification.Name("Candid.openDay")
    /// Object: the search text.
    static let candidOpenSearch = Notification.Name("Candid.openSearch")
}

@MainActor enum CandidURLHandler {
    static func handle(_ url: URL, showMainWindow: @escaping () -> Void) {
        guard SystemHooks.urlEnabled, let command = CandidURL.parse(url) else { return }
        Task { @MainActor in
            switch command {
            case .pause(let minutes):
                _ = await RecordingControl.shared.pause(minutes: minutes)
            case .resume:
                guard SystemHooks.urlResumeEnabled else {
                    NotificationCenter.default.post(name: .candidActionError, object: L("链接开始记录未开启：设置 → 快捷操作 → 系统"))
                    return
                }
                _ = await RecordingControl.shared.start()
            case .open(let date):
                // Wait for launch, so the timeline is there to receive the jump.
                _ = await RecordingControl.shared.readySession()
                showMainWindow()
                if let date { NotificationCenter.default.post(name: .candidOpenDay, object: date) }
            case .search(let text):
                _ = await RecordingControl.shared.readySession()
                showMainWindow()
                NotificationCenter.default.post(name: .candidOpenSearch, object: text)
            }
        }
    }
}

/// 「登录时打开 Candid」: the system's login item for this app (System Settings → General → Login Items shows and
/// controls the same entry). Off unless the user turns it on; a login launch still starts paused.
@MainActor @Observable final class LoginItem {
    static let shared = LoginItem()
    private(set) var status: SMAppService.Status = SMAppService.mainApp.status
    private(set) var error: String?
    var enabled: Bool { status == .enabled || status == .requiresApproval }
    var needsApproval: Bool { status == .requiresApproval }

    func refresh() { status = SMAppService.mainApp.status }

    func set(_ on: Bool) {
        do {
            if on { try SMAppService.mainApp.register() } else { try SMAppService.mainApp.unregister() }
            error = nil
        } catch {
            self.error = on ? L("没有加入登录项：%@", error.localizedDescription) : L("没有移出登录项：%@", error.localizedDescription)
        }
        refresh()
    }

    func openSystemSettings() { SMAppService.openSystemSettingsLoginItems() }

    /// Whether this launch came from the login item, so the window can stay closed.
    static var launchedAtLogin: Bool {
        guard let event = NSAppleEventManager.shared().currentAppleEvent else { return false }
        return event.eventID == kAEOpenApplication
            && event.paramDescriptor(forKeyword: keyAEPropData)?.enumCodeValue == keyAELaunchedAsLogInItem
    }
}

private extension String {
    var nonEmpty: String? { isEmpty ? nil : self }
}
