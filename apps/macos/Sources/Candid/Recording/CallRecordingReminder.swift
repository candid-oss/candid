import AppKit
import CandidLocalization
import CoreAudio
import CoreGraphics
import SwiftUI

struct CallAudioProcess: Sendable, Equatable {
    let bundleID: String
    let pid: Int32
    var callWindowID: CGWindowID? = nil
    var id: String { "\(bundleID):\(pid)" }
    private static let prefixes = ["com.tencent.xinWeChat", "com.tencent.WeChat", "com.tencent.meeting", "com.tencent.WeWork", "com.tencent.qq",
                        "us.zoom.xos", "com.microsoft.teams", "com.apple.FaceTime", "com.alibaba.DingTalkMac",
                        "com.electron.lark", "com.larksuite", "com.tinyspeck.slackmacgap", "com.skype.skype",
                        "com.google.Chrome", "com.microsoft.edgemac", "org.mozilla.firefox", "com.apple.Safari",
                        "com.apple.WebKit", "company.thebrowser.Browser", "com.brave.Browser"]
    var family: String { Self.prefixes.first { bundleID == $0 || bundleID.hasPrefix($0 + ".") } ?? bundleID }
    static func supported(_ bundle: String) -> Bool {
        return prefixes.contains { bundle == $0 || bundle.hasPrefix($0 + ".") }
    }
}

/// One prompt per microphone-use session. Short interruptions do not re-prompt.
struct CallReminderGate {
    private struct Seen { var first: TimeInterval; var last: TimeInterval; var offered = false }
    private var seen: [String: Seen] = [:]
    mutating func update(_ processes: [CallAudioProcess], now: TimeInterval, canOffer: Bool) -> CallAudioProcess? {
        seen = seen.filter { now - $0.value.last < 20 }
        for process in processes {
            if var value = seen[process.id] {
                if !value.offered && now - value.last > 3 { value.first = now }
                value.last = now; seen[process.id] = value
            }
            else { seen[process.id] = Seen(first: now, last: now) }
        }
        guard canOffer else { return nil }
        for process in processes.sorted(by: { $0.id < $1.id }) {
            if let value = seen[process.id], !value.offered, now - value.first >= 2 {
                seen[process.id]?.offered = true
                return process
            }
        }
        return nil
    }
}

@available(macOS 14.2, *)
enum CallAudioActivity {
    static func read() -> [CallAudioProcess]? {
        var address = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyProcessObjectList,
                                                mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var size: UInt32 = 0
        guard AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size) == noErr else { return nil }
        guard size > 0 else { return [] }
        var objects = [AudioObjectID](repeating: 0, count: Int(size) / MemoryLayout<AudioObjectID>.size)
        let result = objects.withUnsafeMutableBytes { AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, $0.baseAddress!) }
        guard result == noErr else { return nil }
        return objects.compactMap { object in
            guard scalar(object, kAudioProcessPropertyIsRunningInput) == 1,
                  let pid = scalar(object, kAudioProcessPropertyPID), pid != UInt32(ProcessInfo.processInfo.processIdentifier) else { return nil }
            var address = AudioObjectPropertyAddress(mSelector: kAudioProcessPropertyBundleID,
                                                    mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
            var value: Unmanaged<CFString>?
            var size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
            guard AudioObjectGetPropertyData(object, &address, 0, nil, &size, &value) == noErr, let value else { return nil }
            let bundle = value.takeRetainedValue() as String
            guard CallAudioProcess.supported(bundle) else { return nil }
            return CallAudioProcess(bundleID: bundle, pid: Int32(bitPattern: pid))
        }
    }
    @MainActor static func callWindow(for process: CallAudioProcess) -> CGWindowID? {
        guard CGPreflightScreenCaptureAccess(), let windows=CGWindowListCopyWindowInfo([.optionAll,.excludeDesktopElements],kCGNullWindowID) as? [[String:Any]] else {return nil}
        let pids=Set(NSWorkspace.shared.runningApplications.filter {
            guard let bundle=$0.bundleIdentifier else{return false}
            return CallAudioProcess(bundleID:bundle,pid:$0.processIdentifier).family==process.family
        }.map(\.processIdentifier)+[process.pid])
        let phrases=[L("视频聊天"),L("语音通话"),L("视频通话"),L("语音聊天"),L("视频会议"),L("的会议"),L("快速会议"),"Zoom Meeting",L("腾讯会议 - ")]
        return windows.first { window in
            guard (window[kCGWindowIsOnscreen as String] as? Bool) != false,
                  let pid=(window[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value,pids.contains(pid),
                  let title=window[kCGWindowName as String] as? String else{return false}
            return phrases.contains {title.localizedCaseInsensitiveContains($0)}
        }.flatMap {($0[kCGWindowNumber as String] as? NSNumber)?.uint32Value}
    }
    @MainActor static func callWindowPresent(for process: CallAudioProcess?) -> Bool? {
        guard let process,let windowID=process.callWindowID,CGPreflightScreenCaptureAccess(),
              let windows=CGWindowListCopyWindowInfo([.optionAll,.excludeDesktopElements],kCGNullWindowID) as? [[String:Any]] else{return nil}
        if let window=windows.first(where:{($0[kCGWindowNumber as String] as? NSNumber)?.uint32Value==windowID}) {return (window[kCGWindowIsOnscreen as String] as? Bool) == false ? nil : true}
        return callWindow(for:process) != nil
    }
    private static func scalar(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector) -> UInt32? {
        var address = AudioObjectPropertyAddress(mSelector: selector, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var value: UInt32 = 0, size = UInt32(MemoryLayout<UInt32>.size)
        return AudioObjectGetPropertyData(object, &address, 0, nil, &size, &value) == noErr ? value : nil
    }
}

/// Wakes when any process starts or stops using audio input: the audio process list, each process's input state,
/// and whether the default input device runs for anyone. Lets the reminder sleep while no call is going on.
@available(macOS 14.2, *)
@MainActor final class AudioInputWatch {
    let changes: AsyncStream<Void>
    private let continuation: AsyncStream<Void>.Continuation
    private let queue = DispatchQueue(label: "io.github.candid-oss.candid.audio-input-watch", qos: .utility)
    private let listener: AudioObjectPropertyListenerBlock
    private var registered: [(AudioObjectID, AudioObjectPropertyAddress)] = []

    init() {
        (changes, continuation) = AsyncStream.makeStream(bufferingPolicy: .bufferingNewest(1))
        listener = Self.listener(continuation)
        refresh()
    }

    isolated deinit {
        unregister()
        continuation.finish()
    }

    /// Listens again on the current processes and default input device (call after a change: processes come and go).
    func refresh() {
        unregister()
        let system = AudioObjectID(kAudioObjectSystemObject)
        add(system, kAudioHardwarePropertyProcessObjectList)
        add(system, kAudioHardwarePropertyDefaultInputDevice)
        if let device: AudioObjectID = Self.value(system, kAudioHardwarePropertyDefaultInputDevice) {
            add(device, kAudioDevicePropertyDeviceIsRunningSomewhere)
        }
        for process in Self.processes() { add(process, kAudioProcessPropertyIsRunningInput) }
    }

    /// Built outside the main actor: Core Audio calls it on its own queue.
    private nonisolated static func listener(_ continuation: AsyncStream<Void>.Continuation) -> AudioObjectPropertyListenerBlock {
        { _, _ in continuation.yield() }
    }

    private func add(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector) {
        var address = AudioObjectPropertyAddress(mSelector: selector, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        if AudioObjectAddPropertyListenerBlock(object, &address, queue, listener) == noErr { registered.append((object, address)) }
    }

    private func unregister() {
        for (object, address) in registered {
            var address = address
            AudioObjectRemovePropertyListenerBlock(object, &address, queue, listener)
        }
        registered = []
    }

    private static func value<T>(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector) -> T? {
        var address = AudioObjectPropertyAddress(mSelector: selector, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var size = UInt32(MemoryLayout<T>.size)
        let pointer = UnsafeMutablePointer<T>.allocate(capacity: 1)
        defer { pointer.deallocate() }
        return AudioObjectGetPropertyData(object, &address, 0, nil, &size, pointer) == noErr ? pointer.pointee : nil
    }

    private static func processes() -> [AudioObjectID] {
        var address = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyProcessObjectList,
                                                mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var size: UInt32 = 0
        guard AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size) == noErr, size > 0 else { return [] }
        var objects = [AudioObjectID](repeating: 0, count: Int(size) / MemoryLayout<AudioObjectID>.size)
        let result = objects.withUnsafeMutableBytes { AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, $0.baseAddress!) }
        return result == noErr ? objects : []
    }
}

/// Lives with the application delegate, including when the main window is closed.
@MainActor final class CallRecordingReminder {
    private var task: Task<Void, Never>?
    private var panel: NSPanel?
    private var offered: CallAudioProcess?
    private var gate = CallReminderGate()
    private var stopGate = CallRecordingStopGate()
    private let media: MediaWorkspaceStore
    init(media: MediaWorkspaceStore) { self.media = media }
    func start() {
        guard #available(macOS 15, *), task == nil, !media.isDemo else { return }
        task = Task { [weak self] in
            let watch = AudioInputWatch()
            var changes = watch.changes.makeAsyncIterator()
            while !Task.isCancelled {
                guard let self else { return }
                var callAudio = false
                if self.media.callRecordingReminders || self.media.callRecordingTarget != nil {
                    let processes = await Task.detached(priority: .utility) { CallAudioActivity.read() }.value
                    if self.stopGate.shouldStop(target: self.media.autoStopCallRecording ? self.media.callRecordingTarget : nil,
                                                active: processes, now: ProcessInfo.processInfo.systemUptime, callWindowPresent: CallAudioActivity.callWindowPresent(for: self.media.callRecordingTarget)) {
                        await self.media.stopRecording(automatic: true)
                    }
                    if let processes { self.update(processes); callAudio = !processes.isEmpty }
                } else { self.close() }
                // While a call app holds the microphone, a call is recorded or the prompt is up, the timing rules above
                // (how long the microphone has been in use, when a call ended) need a look every 2 s. Otherwise
                // nothing runs until some process starts or stops using audio input.
                if callAudio || self.media.callRecordingTarget != nil || self.panel != nil {
                    try? await Task.sleep(for: .seconds(2))
                } else {
                    watch.refresh()
                    guard await changes.next() != nil else { return }
                }
            }
        }
    }
    func stop() { task?.cancel(); task = nil; close() }
    private func update(_ processes: [CallAudioProcess]) {
        if media.isRecording || !media.callRecordingReminders || (offered.map { !processes.contains($0) } ?? false) { close() }
        let candidate = gate.update(processes, now: ProcessInfo.processInfo.systemUptime,
                                    canOffer: panel == nil && media.canCapture && media.callRecordingReminders)
        if let candidate { show(candidate) }
    }
    private func close() { panel?.orderOut(nil); panel = nil; offered = nil }
    private func show(_ process: CallAudioProcess) {
        offered = process
        let name = NSRunningApplication(processIdentifier: process.pid)?.localizedName ?? process.bundleID
        let panel = NSPanel(contentRect: .zero, styleMask: [.titled, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.title = L("Candid · 通话录音")
        panel.level = .floating; panel.hidesOnDeactivate = false; panel.isReleasedWhenClosed = false
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        let view = CallRecordingPrompt(appName: name, media: media, start: { [weak self] in
            guard let self else { return }
            Task { await self.media.startRecording(input: .systemAndMicrophone, call: process); if self.media.isRecording { self.close() } }
        }, dismiss: { [weak self] in self?.close() })
        let host = NSHostingController(rootView: view)
        panel.contentViewController = host
        let size = host.view.fittingSize
        panel.setContentSize(size)
        let screen = NSScreen.main?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1280, height: 800)
        panel.setFrameTopLeftPoint(NSPoint(x: screen.maxX - size.width - 24, y: screen.maxY - 24))
        self.panel = panel
        panel.orderFrontRegardless()
    }
}

struct CallRecordingPrompt: View {
    let appName: String
    let media: MediaWorkspaceStore
    let start: () -> Void
    let dismiss: () -> Void
    var body: some View {
        VStack(alignment: .leading, spacing: Space.m) {
            Label(L("%@ 正在使用麦克风", appName), systemImage: "phone.badge.waveform").typography(.body, .semibold)
            Text(L("开始后录制所有系统声音和麦克风。请先告知通话参与者。"))
                .typography(.secondary).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            if media.autoStopCallRecording {
                Text(L("通话结束后自动停止录音")).typography(.caption).foregroundStyle(.secondary)
            }
            if let error = media.error {
                Text(error).typography(.secondary).foregroundStyle(Palette.warning).fixedSize(horizontal: false, vertical: true)
                if error.contains("麦克风权限") || error.contains("屏幕录制权限") {
                    QuietButton(title: L("打开权限设置"), emphasized: true) {
                        let pane = error.contains("麦克风权限") ? "Privacy_Microphone" : "Privacy_ScreenCapture"
                        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?" + pane) { NSWorkspace.shared.open(url) }
                    }
                }
            }
            HStack {
                QuietButton(title: L("本次忽略"), action: dismiss).disabled(media.busy).keyboardShortcut(.cancelAction)
                Spacer()
                PrimaryButton(title: media.busy ? L("正在开启…") : L("开始内录"), action: start).disabled(!media.canCapture)
            }
        }
        .padding(Space.m).frame(width: 360).background(.regularMaterial)
    }
}
