import AppKit
import CandidLocalization
import Carbon
import SwiftUI

/// Carbon hot keys do not need Accessibility or Input Monitoring permission.
@MainActor @Observable final class GlobalHotkeys {
    static let shared = GlobalHotkeys()
    struct Binding: Codable, Equatable {
        var key: UInt32
        var modifiers: UInt32
    }
    private(set) var errors: [String: String] = [:]
    private(set) var bindings: [String: Binding] = [:]
    private var refs: [EventHotKeyRef] = []
    private var handler: EventHandlerRef?
    private var active = false
    private let defaults: UserDefaults
    static let keys: [(String, UInt32)] = [("S", 1), ("M", 46), ("R", 15), ("N", 45), ("F", 3), ("1", 18), ("2", 19), ("3", 20), ("4", 21), ("5", 23)]
    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        if let data = defaults.data(forKey: "candid.hotkeys"), let values = try? JSONDecoder().decode([String: Binding].self, from: data) { bindings = values }
    }
    func start() {
        guard !active else { return }; active = true
        var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        InstallEventHandler(GetApplicationEventTarget(), { _, event, _ in
            var hot = EventHotKeyID()
            guard let event, GetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID), nil, MemoryLayout<EventHotKeyID>.size, nil, &hot) == noErr else { return OSStatus(eventNotHandledErr) }
            let index = Int(hot.id) - 1
            guard QuickAction.allCases.indices.contains(index) else { return OSStatus(eventNotHandledErr) }
            let action = QuickAction.allCases[index]
            Task { @MainActor in NotificationCenter.default.post(name: .candidQuickAction, object: action) }
            return noErr
        }, 1, &spec, nil, &handler)
        register()
    }
    func configure(_ action: QuickAction, key: UInt32?, modifiers: UInt32 = UInt32(cmdKey | shiftKey)) {
        bindings[action.rawValue] = key.map { Binding(key: $0, modifiers: modifiers) }
        defaults.set(try? JSONEncoder().encode(bindings), forKey: "candid.hotkeys")
        if active { register() } else { validate() }
    }
    func stop() { refs.forEach { UnregisterEventHotKey($0) }; refs = []; if let handler { RemoveEventHandler(handler) }; handler = nil; active = false }
    private func validate() {
        errors = [:]; var used: Set<String> = []
        for action in QuickAction.allCases {
            guard let value = bindings[action.rawValue] else { continue }
            if !used.insert("\(value.key):\(value.modifiers)").inserted { errors[action.rawValue] = L("与另一项快捷键重复") }
        }
    }
    private func register() {
        refs.forEach { UnregisterEventHotKey($0) }; refs = []; errors = [:]
        var used: Set<String> = []
        for (index, action) in QuickAction.allCases.enumerated() {
            guard let value = bindings[action.rawValue] else { continue }
            guard used.insert("\(value.key):\(value.modifiers)").inserted else { errors[action.rawValue] = L("与另一项快捷键重复"); continue }
            var ref: EventHotKeyRef?
            let result = RegisterEventHotKey(value.key, value.modifiers, EventHotKeyID(signature: 0x50564149, id: UInt32(index + 1)), GetApplicationEventTarget(), 0, &ref)
            if result == noErr, let ref { refs.append(ref) } else { errors[action.rawValue] = L("快捷键已被占用，请换一个") }
        }
    }
}

struct HotkeySettingsView: View {
    private let model = GlobalHotkeys.shared
    @AppStorage("candid.floating") private var floating = false
    var body: some View {
        VStack(alignment: .leading, spacing: Space.l) {
            Text(L("快捷操作")).typography(.title)
            ToggleChip(title: L("显示悬浮控件"), isOn: $floating)
            ForEach(QuickAction.allCases) { action in
                HStack {
                    Text(action.title).typography(.body)
                    Spacer()
                    MenuChoice(options: [(UInt32.max, L("关闭"))] + GlobalHotkeys.keys.map { ($0.1, "⇧⌘" + $0.0) },
                               selection: Binding(get: { model.bindings[action.rawValue]?.key ?? UInt32.max }, set: { model.configure(action, key: $0 == UInt32.max ? nil : $0) }))
                }
                if let error = model.errors[action.rawValue] { Text(error).typography(.secondary).foregroundStyle(.red) }
            }
        }.padding(Space.page)
    }
}
