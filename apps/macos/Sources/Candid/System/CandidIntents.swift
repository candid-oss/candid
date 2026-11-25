import AppIntents
import CandidLocalization
import Foundation

// Shortcuts actions and the Focus filter. Titles, descriptions and parameter names are read at build time from these
// literals (script/build_and_run.sh runs appintentsmetadataprocessor), so they stay literals; lines that carry them
// are marked `intent-metadata` for script/localize_sources.py, and their English text comes from en.lproj, which the
// build generates from Resources/Localizable.en.json. Results shown at run time use L() like the rest of the app.
// Each action checks SystemHooks.intentsEnabled, so 设置 → 快捷操作 → 系统 can turn them all off.

struct CandidIntentError: Error, CustomLocalizedStringResourceConvertible {
    let message: String
    var localizedStringResource: LocalizedStringResource { "\(message)" }

    static func requireEnabled() throws {
        guard SystemHooks.intentsEnabled else { throw CandidIntentError(message: L("快捷指令控制已在 Candid 设置中关闭")) }
    }
}

struct StartRecordingIntent: AppIntent {
    static let title: LocalizedStringResource = "开始记录" // intent-metadata
    static let description = IntentDescription("开始记录电脑上的应用活动；截图仍按 Candid 里的设置。") // intent-metadata
    static let openAppWhenRun = false

    @MainActor func perform() async throws -> some IntentResult & ProvidesDialog {
        try CandidIntentError.requireEnabled()
        let outcome = await RecordingControl.shared.start()
        if outcome == .notReady || outcome == .demo { throw CandidIntentError(message: outcome.message) }
        return .result(dialog: "\(outcome.message)")
    }
}

struct PauseRecordingIntent: AppIntent {
    static let title: LocalizedStringResource = "暂停记录" // intent-metadata
    static let description = IntentDescription("暂停电脑记录，直到你再次开始。") // intent-metadata
    static let openAppWhenRun = false

    @MainActor func perform() async throws -> some IntentResult & ProvidesDialog {
        try CandidIntentError.requireEnabled()
        let outcome = await RecordingControl.shared.pause()
        if outcome == .notReady || outcome == .demo { throw CandidIntentError(message: outcome.message) }
        return .result(dialog: "\(outcome.message)")
    }
}

struct PauseRecordingForIntent: AppIntent {
    static let title: LocalizedStringResource = "暂停记录一段时间" // intent-metadata
    static let description = IntentDescription("暂停电脑记录，到时自动继续；本来就暂停时不会自动开始。") // intent-metadata
    static let openAppWhenRun = false

    @Parameter(title: "分钟", default: 30, inclusiveRange: (1, 1440)) // intent-metadata
    var minutes: Int

    static var parameterSummary: some ParameterSummary {
        Summary("暂停记录 \(\.$minutes) 分钟") // intent-metadata
    }

    @MainActor func perform() async throws -> some IntentResult & ProvidesDialog {
        try CandidIntentError.requireEnabled()
        let outcome = await RecordingControl.shared.pause(minutes: minutes)
        if outcome == .notReady || outcome == .demo { throw CandidIntentError(message: outcome.message) }
        return .result(dialog: "\(outcome.message)")
    }
}

struct RecordingStatusIntent: AppIntent {
    static let title: LocalizedStringResource = "Candid 记录状态" // intent-metadata
    static let description = IntentDescription("返回电脑记录、截图和录音现在的状态。") // intent-metadata
    static let openAppWhenRun = false

    @MainActor func perform() async throws -> some IntentResult & ReturnsValue<String> & ProvidesDialog {
        try CandidIntentError.requireEnabled()
        let text = await RecordingControl.shared.statusText()
        return .result(value: text, dialog: "\(text)")
    }
}

struct TodaySummaryIntent: AppIntent {
    static let title: LocalizedStringResource = "今天做了什么" // intent-metadata
    static let description = IntentDescription("按时间列出今天整理好的活动，以文本返回。") // intent-metadata
    static let openAppWhenRun = false

    @MainActor func perform() async throws -> some IntentResult & ReturnsValue<String> & ProvidesDialog {
        try CandidIntentError.requireEnabled()
        do {
            let text = try await DaySummaryText.fetch()
            return .result(value: text, dialog: "\(text)")
        } catch let error as CandidIntentError { throw error }
        catch { throw CandidIntentError(message: L("没有读到今天的记录：%@", error.localizedDescription)) }
    }
}

struct CandidShortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(intent: StartRecordingIntent(), phrases: ["用 \(.applicationName) 开始记录", "Start recording with \(.applicationName)"], // intent-metadata
                    shortTitle: "开始记录", systemImageName: "record.circle") // intent-metadata
        AppShortcut(intent: PauseRecordingIntent(), phrases: ["用 \(.applicationName) 暂停记录", "Pause \(.applicationName)"], // intent-metadata
                    shortTitle: "暂停记录", systemImageName: "pause.circle") // intent-metadata
        AppShortcut(intent: RecordingStatusIntent(), phrases: ["\(.applicationName) 在记录吗", "\(.applicationName) status"], // intent-metadata
                    shortTitle: "记录状态", systemImageName: "info.circle") // intent-metadata
        AppShortcut(intent: TodaySummaryIntent(), phrases: ["\(.applicationName) 今天做了什么", "What did I do today in \(.applicationName)"], // intent-metadata
                    shortTitle: "今天做了什么", systemImageName: "calendar.day.timeline.left") // intent-metadata
    }
}

/// System Settings → Focus → (a Focus) → Focus Filters → Candid. Turning the switch on pauses computer capture while
/// that Focus is on and resumes it when the Focus ends, unless recording was changed in the meantime.
struct CandidFocusFilter: SetFocusFilterIntent {
    static let title: LocalizedStringResource = "Candid 记录" // intent-metadata
    static let description: IntentDescription? = IntentDescription("在这个专注模式下暂停电脑记录，结束后恢复。") // intent-metadata

    @Parameter(title: "暂停电脑记录", default: false) // intent-metadata
    var pauseRecording: Bool

    var displayRepresentation: DisplayRepresentation {
        DisplayRepresentation(title: pauseRecording ? "暂停电脑记录" : "不改变记录") // intent-metadata
    }

    @MainActor func perform() async throws -> some IntentResult {
        await RecordingControl.shared.applyFocus(pause: pauseRecording)
        return .result()
    }
}
