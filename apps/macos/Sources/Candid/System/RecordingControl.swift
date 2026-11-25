import AppKit
import CandidClient
import CandidLocalization
import Foundation
import Observation

/// Switches for the macOS hooks: Shortcuts actions, `candid://` links and the Focus filter. Each can be turned off on
/// its own. Defaults are conservative: nothing here runs unless the user builds a shortcut, opens a link or adds the
/// filter to a Focus, and a link may only pause (starting to record from a link is a separate opt-in).
enum SystemHooks {
    static let intentsKey = "system.intents.enabled"
    static let urlKey = "system.url.enabled"
    static let urlResumeKey = "system.url.resume"
    static let focusKey = "system.focus.enabled"

    static func value(_ key: String, default fallback: Bool, defaults: UserDefaults = .standard) -> Bool {
        defaults.object(forKey: key) as? Bool ?? fallback
    }
    static var intentsEnabled: Bool { value(intentsKey, default: true) }
    static var urlEnabled: Bool { value(urlKey, default: true) }
    static var urlResumeEnabled: Bool { value(urlResumeKey, default: false) }
    static var focusEnabled: Bool { value(focusKey, default: true) }
}

/// Start, pause and timed pause of computer capture for callers outside the window: Shortcuts, `candid://` links and
/// the Focus filter. A timed pause resumes only a session that was recording, and is dropped as soon as recording
/// starts some other way; a Focus that paused recording resumes it when the Focus ends, unless the user changed it.
@MainActor @Observable final class RecordingControl {
    static let shared = RecordingControl()

    enum Outcome: Equatable {
        case started, paused, pausedUntil(Date), alreadyRecording, alreadyPaused, notReady, demo

        var message: String {
            switch self {
            case .started: L("已开始记录")
            case .paused: L("已暂停记录")
            case .pausedUntil(let date): L("已暂停记录，%@ 自动继续", date.formatted(date: .omitted, time: .shortened))
            case .alreadyRecording: L("已经在记录")
            case .alreadyPaused: L("记录本来就是暂停的")
            case .notReady: L("Candid 尚未就绪，请稍后再试")
            case .demo: L("合成样例中不能控制记录")
            }
        }
    }

    /// When a timed pause will resume recording.
    private(set) var resumeAt: Date?
    @ObservationIgnored private(set) weak var session: DesktopSession?
    @ObservationIgnored private(set) weak var media: MediaWorkspaceStore?
    @ObservationIgnored private var resumeTask: Task<Void, Never>?
    @ObservationIgnored private var focusHold = false
    @ObservationIgnored private var pausedByFocus = false

    func attach(session: DesktopSession, media: MediaWorkspaceStore) {
        guard self.session !== session else { return }
        self.session = session; self.media = media
        watch()
    }

    /// The session once launch has finished; an intent or a link can be what launched the app.
    func readySession(timeout: Duration = .seconds(10)) async -> DesktopSession? {
        let clock = ContinuousClock()
        let deadline = clock.now + timeout
        while clock.now < deadline {
            if let session, session.isReady { return session }
            try? await Task.sleep(for: .milliseconds(200))
        }
        return session?.isReady == true ? session : nil
    }

    func start() async -> Outcome {
        guard let session = await readySession() else { return .notReady }
        if session.isDemo { return .demo }
        cancelResume()
        if session.isRecording { return .alreadyRecording }
        session.startRecording()
        return session.isRecording ? .started : .notReady
    }

    /// Pauses now; with `minutes`, recording resumes then, but only if it was on (or already in a timed pause).
    func pause(minutes: Int? = nil) async -> Outcome {
        guard let session = await readySession() else { return .notReady }
        if session.isDemo { return .demo }
        let resumable = session.isRecording || resumeAt != nil
        if session.isRecording { session.pauseRecording() }
        cancelResume()
        guard resumable else { return .alreadyPaused }
        guard let minutes, minutes > 0 else { return .paused }
        let target = Date().addingTimeInterval(TimeInterval(min(minutes, 24 * 60) * 60))
        scheduleResume(at: target)
        return .pausedUntil(target)
    }

    /// One line for Shortcuts and the menu bar.
    func statusText() async -> String {
        guard let session = await readySession(timeout: .seconds(5)) else { return Outcome.notReady.message }
        var parts = [session.isRecording ? L("正在记录") : L("已暂停")]
        if !session.isRecording, let resumeAt { parts.append(L("%@ 自动继续", resumeAt.formatted(date: .omitted, time: .shortened))) }
        if session.isRecording { parts.append(session.screenshotsEnabled ? L("截图已开启") : L("截图未开启")) }
        if media?.isRecording == true { parts.append(L("正在录音")) }
        return parts.joined(separator: L("，"))
    }

    /// The Focus filter: `pause` is the filter's setting for the Focus that just turned on, or the default (off)
    /// when that Focus ended.
    func applyFocus(pause: Bool) async {
        if pause {
            guard SystemHooks.focusEnabled, let session = await readySession(timeout: .seconds(5)), !session.isDemo else { return }
            focusHold = true
            if session.isRecording { session.pauseRecording(); pausedByFocus = true }
        } else {
            focusHold = false
            guard pausedByFocus else { return }
            pausedByFocus = false
            if let session, session.isReady, !session.isRecording { session.startRecording() }
        }
    }

    private func scheduleResume(at target: Date) {
        resumeAt = target
        let seconds = max(0, target.timeIntervalSinceNow)
        resumeTask = Task { [weak self] in
            // The continuous clock keeps counting while the Mac sleeps, so a resume due during sleep runs on wake.
            try? await ContinuousClock().sleep(for: .seconds(seconds))
            guard !Task.isCancelled else { return }
            self?.resumeIfDue(target)
        }
    }

    private func resumeIfDue(_ target: Date) {
        guard resumeAt == target else { return }
        resumeAt = nil; resumeTask = nil
        guard let session, session.isReady, !session.isRecording else { return }
        // A Focus that asks for a pause wins; recording resumes when that Focus ends.
        if focusHold { pausedByFocus = true; return }
        session.startRecording()
    }

    private func cancelResume() { resumeTask?.cancel(); resumeTask = nil; resumeAt = nil }

    /// Recording started some other way: a pending timed resume or Focus resume no longer applies.
    private func watch() {
        withObservationTracking { _ = session?.isRecording } onChange: { [weak self] in
            Task { @MainActor in
                guard let self else { return }
                if self.session?.isRecording == true { self.cancelResume(); self.pausedByFocus = false }
                self.watch()
            }
        }
    }
}

/// 「今天做了什么」 as plain text, from the core's `day` tool (the same tool MCP clients use).
enum DaySummaryText {
    @MainActor static func fetch(date: String = "today", channel: LocalChannel? = nil) async throws -> String {
        let clock = ContinuousClock()
        let deadline = clock.now + .seconds(10)
        var resolved = channel ?? TimelineService.shared.channel
        while resolved == nil, clock.now < deadline {
            try? await Task.sleep(for: .milliseconds(250))
            resolved = TimelineService.shared.channel
        }
        guard let resolved else { throw CoreError.unreachable(L("时间线库尚未打开")) }
        let arguments = try JSONSerialization.data(withJSONObject: ["date": date, "limit": 60])
        return format(try await resolved.tool("day", arguments: arguments))
    }

    static func format(_ data: Data, timeZone: TimeZone = .current) -> String {
        guard let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return L("没有读到当天的记录") }
        let blocks = root["blocks"] as? [[String: Any]] ?? []
        let daily = (root["daily"] as? [[String: Any]] ?? []).compactMap { ($0["text"] as? String)?.trimmed }.filter { !$0.isEmpty }
        let clock = DateFormatter()
        clock.dateFormat = "HH:mm"; clock.timeZone = timeZone; clock.locale = Locale(identifier: "en_US_POSIX")
        let lines: [String] = blocks.compactMap { block in
            let start = (block["start"] as? String).flatMap(parse).map(clock.string(from:))
            let end = (block["end"] as? String).flatMap(parse).map(clock.string(from:))
            let span = [start, end].compactMap { $0 }.joined(separator: "–")
            let title = (block["title"] as? String)?.trimmed.nonEmpty
                ?? [block["app"] as? String, block["window"] as? String].compactMap { $0?.trimmed.nonEmpty }.joined(separator: " · ")
            guard !title.isEmpty || !span.isEmpty else { return nil }
            var line = span.isEmpty ? title : span + "  " + title
            if let summary = (block["summary"] as? String)?.trimmed.nonEmpty { line += L("：") + summary }
            return line
        }
        if lines.isEmpty && daily.isEmpty { return L("这段时间没有整理好的活动") }
        return (daily + lines).joined(separator: "\n")
    }

    private static func parse(_ text: String) -> Date? {
        let full = ISO8601DateFormatter()
        full.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = full.date(from: text) { return date }
        full.formatOptions = [.withInternetDateTime]
        return full.date(from: text)
    }
}

private extension String {
    var trimmed: String { trimmingCharacters(in: .whitespacesAndNewlines) }
    var nonEmpty: String? { isEmpty ? nil : self }
}
