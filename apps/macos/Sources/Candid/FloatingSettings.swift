import AIProviders
import CandidLocalization
import SwiftUI

/// The floating ball's own switches, kept in user defaults next to the existing `candid.floating`.
enum FloatingPreferences {
    /// The ball as a whole (the key the old floating toolbar used).
    static let ballKey = "candid.floating"
    /// Voice input: holding the ball or the voice shortcut.
    static let voiceKey = "candid.floating.voice"
    static let engineKey = "candid.voice.engine"
    static let cloudProviderKey = "candid.voice.cloud.provider"
    static let cloudModelKey = "candid.voice.cloud.model"
    static let positionKey = "candid.floating.position"
    static let defaultCloudModel = "whisper-1"

    static func voiceEnabled(_ defaults: UserDefaults = .standard) -> Bool { defaults.object(forKey: voiceKey) as? Bool ?? true }
    static func engine(_ defaults: UserDefaults = .standard) -> VoiceEngineChoice {
        VoiceEngineChoice(rawValue: defaults.string(forKey: engineKey) ?? "") ?? .automatic
    }
    static func cloudProvider(_ defaults: UserDefaults = .standard) -> String { defaults.string(forKey: cloudProviderKey) ?? "" }
    static func cloudModel(_ defaults: UserDefaults = .standard) -> String {
        let model = defaults.string(forKey: cloudModelKey)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return model.isEmpty ? defaultCloudModel : model
    }
}

/// The small settings inside the ball's panel: voice on or off, which recognizer, the cloud source, the shortcut.
struct FloatingSettingsView: View {
    let voice: VoiceInputController
    let close: () -> Void
    @AppStorage(FloatingPreferences.voiceKey) private var voiceEnabled = true
    @AppStorage(FloatingPreferences.engineKey) private var engine = VoiceEngineChoice.automatic.rawValue
    @AppStorage(FloatingPreferences.cloudProviderKey) private var cloudProvider = ""
    @AppStorage(FloatingPreferences.cloudModelKey) private var cloudModel = FloatingPreferences.defaultCloudModel
    private let hotkeys = GlobalHotkeys.shared

    var body: some View {
        VStack(alignment: .leading, spacing: Space.s) {
            HStack {
                Text(L("悬浮球设置")).typography(.headline)
                Spacer()
                QuietButton(title: L("完成"), action: close)
            }
            ToggleChip(title: L("语音输入"), isOn: $voiceEnabled)
            if voiceEnabled {
                row(L("识别引擎")) {
                    MenuChoice(options: VoiceEngineChoice.allCases.map { ($0.rawValue, $0.title) }, selection: $engine)
                }
                row(L("云端来源")) {
                    MenuChoice(options: [("", L("不用云端"))] + CloudVoiceTarget.candidates(AIRuntime.shared.config()).map { ($0.providerID, $0.name) },
                               selection: $cloudProvider)
                }
                if !cloudProvider.isEmpty {
                    row(L("转写模型")) {
                        TextField(FloatingPreferences.defaultCloudModel, text: $cloudModel).textFieldStyle(.plain).typography(.body)
                            .multilineTextAlignment(.trailing).frame(width: 150)
                    }
                    Text(L("语音会发到这个来源的 /audio/transcriptions，用它已有的地址和密钥。")).typography(.caption).foregroundStyle(.secondary)
                }
                row(L("按住说话的快捷键")) {
                    MenuChoice(options: [(-1, L("关闭"))] + GlobalHotkeys.voiceKeys.enumerated().map { ($0.offset, $0.element.title) },
                               selection: Binding(get: { hotkeys.voiceKeyIndex },
                                                  set: { hotkeys.configureVoice($0 < 0 ? nil : GlobalHotkeys.voiceKeys[$0].binding) }))
                }
                if let error = hotkeys.errors[GlobalHotkeys.voiceID] { Text(error).typography(.secondary).foregroundStyle(Palette.danger) }
                HStack(spacing: Space.xxs) {
                    Text(L("现在会用：")).typography(.secondary).foregroundStyle(.secondary)
                    Text(voice.resolved?.title ?? L("检查中…")).typography(.secondary, .medium)
                }
                Text(L("只在按住时打开麦克风，录音片段不保存；识别出的文字先放进输入框，回车才发送。")).typography(.caption).foregroundStyle(.secondary)
            }
        }
        .task(id: "\(engine)|\(cloudProvider)|\(cloudModel)|\(voiceEnabled)") { await voice.refreshResolved() }
    }

    private func row<Control: View>(_ title: String, @ViewBuilder control: () -> Control) -> some View {
        HStack { Text(title).typography(.body); Spacer(); control() }
    }
}
