import SwiftUI
import CandidLocalization
import AppKit
import Observation

/// 设置 → 数据 → 备份与迁移: `candid backup`, `candid restore` and `candid move-root` (docs/data-migration.md),
/// run as the bundled helper with `--json`. Capture pauses while one runs. Restore and moving the library need the
/// core stopped, so the App stops it first and relaunches afterwards.
struct DataMigrationGroup: View {
    @Bindable var session: DesktopSession
    @ViewState private var model = DataMigrationModel()

    var body: some View {
        SettingsGroup(title: L("备份与迁移")) {
            SettingRow(title: L("备份…"), detail: L("把时间线库、截图、录音和设置复制到一个文件夹，Candid 运行中也能做。不含 Keychain 里的密钥。")) {
                QuietButton(title: L("备份…")) { model.chooseBackup() }
            }
            HairlineDivider()
            SettingRow(title: L("恢复…"), detail: L("从备份文件夹恢复。现有数据先移到「before-restore-时间」文件夹，不会被覆盖；完成后 Candid 会重新启动。")) {
                QuietButton(title: L("恢复…")) { model.chooseRestore() }
            }
            HairlineDivider()
            SettingRow(title: L("移动数据位置…"), detail: model.location) {
                QuietButton(title: L("移动数据位置…")) { model.chooseMove() }
            }
            if let pending = model.pending {
                InlinePanel {
                    VStack(alignment: .leading, spacing: Space.s) {
                        InlineMessage(kind: pending.destructive ? .warning : .note, text: pending.message)
                        HStack(spacing: Space.xxs) {
                            Spacer()
                            QuietButton(title: L("取消")) { withAnimation(Motion.spring) { model.pending = nil } }
                            ForEach(pending.choices, id: \.title) { choice in
                                QuietButton(title: choice.title, role: pending.destructive ? .destructive : nil, emphasized: !pending.destructive) {
                                    model.run(choice.arguments, operation: pending.operation, session: session)
                                }
                            }
                        }
                    }
                }
                .padding(.bottom, Space.s)
            }
            if model.running || model.outcome != nil {
                InlinePanel {
                    VStack(alignment: .leading, spacing: Space.s) {
                        if model.running {
                            ProgressView(model.phase, value: model.fraction ?? 0, total: 1)
                            Text(L("采集已暂停。请不要退出 Candid。")).typography(.secondary).foregroundStyle(.secondary)
                        }
                        if let outcome = model.outcome {
                            InlineMessage(kind: outcome.ok ? .note : .error, text: outcome.text)
                            HStack(spacing: Space.xxs) {
                                Spacer()
                                if outcome.relaunch {
                                    PrimaryButton(title: L("重新启动 Candid")) { DataMigrationModel.relaunch() }
                                } else {
                                    if let path = outcome.reveal {
                                        QuietButton(title: L("在 Finder 中显示")) { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)]) }
                                    }
                                    QuietButton(title: L("好")) { withAnimation(Motion.spring) { model.outcome = nil } }
                                }
                            }
                        }
                    }
                }
                .padding(.bottom, Space.s)
            }
        }
        .disabled(session.isDemo || model.running)
        .animation(Motion.spring, value: model.pending?.message)
        .animation(Motion.spring, value: model.outcome?.text)
    }
}

@MainActor @Observable final class DataMigrationModel {
    enum Operation { case backup, restore, move }
    struct Choice { var title: String; var arguments: [String] }
    struct Pending { var operation: Operation; var message: String; var choices: [Choice]; var destructive: Bool }
    struct Outcome { var ok: Bool; var text: String; var relaunch = false; var reveal: String? }

    var pending: Pending?
    var running = false
    var phase = ""
    var fraction: Double?
    var outcome: Outcome?

    private var support: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("Candid", isDirectory: true)
    }

    /// Where the library lives now: the support folder, or wherever `move-root` pointed its link.
    var location: String {
        let link = support.appendingPathComponent("timeline")
        let target = (try? FileManager.default.destinationOfSymbolicLink(atPath: link.path)) ?? link.path
        return L("时间线库现在在 %@。可以移到外置盘或其他位置。", target)
    }

    // MARK: Choosing

    func chooseBackup() {
        guard let folder = pick(message: L("选择存放备份的文件夹")) else { return }
        pending = Pending(operation: .backup,
                          message: L("备份到 %@？会在里面新建一个「candid-backup-时间」文件夹。完整备份包括截图和录音原件，可能需要几分钟。", folder.path),
                          choices: [Choice(title: L("只备份数据库"), arguments: ["backup", folder.path, "--no-media"]),
                                    Choice(title: L("完整备份"), arguments: ["backup", folder.path])],
                          destructive: false)
    }

    func chooseRestore() {
        guard let folder = pick(message: L("选择一个 Candid 备份文件夹（里面有 manifest.json）")) else { return }
        let manifestURL = folder.appendingPathComponent("manifest.json")
        guard let data = try? Data(contentsOf: manifestURL),
              let manifest = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            outcome = Outcome(ok: false, text: L("%@ 不是 Candid 备份：没有 manifest.json。", folder.path)); return
        }
        let created = manifest["created_at"] as? String ?? "?"
        let events = manifest["events"] as? Int ?? 0
        let bytes = ByteCountFormatter.string(fromByteCount: Int64(manifest["bytes"] as? Int ?? 0), countStyle: .file)
        let media = (manifest["media"] as? Bool ?? true) ? "" : L("这份备份不含截图和录音。")
        pending = Pending(operation: .restore,
                          message: L("用 %@ 的备份（%@ 条事件，%@）替换现有数据？现有数据会先移到「before-restore-时间」文件夹，不会删除。%@恢复期间核心会停止，完成后 Candid 重新启动。", created, events, bytes, media),
                          choices: [Choice(title: L("恢复"), arguments: ["restore", folder.path, "--move-existing"])],
                          destructive: true)
    }

    func chooseMove() {
        guard let folder = pick(message: L("选择新位置（例如外置盘）。会在里面新建「Candid timeline」文件夹。")) else { return }
        let target = folder.appendingPathComponent("Candid timeline", isDirectory: true)
        pending = Pending(operation: .move,
                          message: L("把时间线库移到 %@？复制并逐个校验后，原位置改为指向新位置的链接。期间核心会停止，完成后 Candid 重新启动。外置盘没连接时 Candid 打不开库。", target.path),
                          choices: [Choice(title: L("移动并删除旧数据"), arguments: ["move-root", target.path, "--remove-old"]),
                                    Choice(title: L("移动，保留旧数据"), arguments: ["move-root", target.path])],
                          destructive: true)
    }

    private func pick(message: String) -> URL? {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true; panel.canChooseFiles = false; panel.canCreateDirectories = true
        panel.allowsMultipleSelection = false; panel.message = message; panel.prompt = L("选择")
        return panel.runModal() == .OK ? panel.url : nil
    }

    // MARK: Running

    func run(_ arguments: [String], operation: Operation, session: DesktopSession) {
        pending = nil; outcome = nil; running = true; fraction = nil; phase = L("准备中…")
        let wasRecording = session.isRecording
        if wasRecording { session.pauseRecording() }
        // Restore and move need the data root's writer lock: the App's core must stop first.
        let stopsCore = operation != .backup
        if stopsCore { TimelineService.shared.stop() }
        let executable = TimelineService.corePath
        result = nil
        Task {
            await Self.execute(executable, arguments + ["--json"]) { text in self.received(text) }
            running = false
            outcome = describe(result, operation: operation, relaunch: stopsCore)
            if !stopsCore && wasRecording { session.startRecording() }
        }
    }

    @ObservationIgnored private var result: [String: Any]?

    private func received(_ text: String) {
        guard let line = try? JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any] else { return }
        if line["ok"] != nil { result = line; return }
        guard let phase = line["phase"] as? String else { return }
        let done = (line["done"] as? Double) ?? Double(line["done"] as? Int ?? 0)
        let total = (line["total"] as? Double) ?? Double(line["total"] as? Int ?? 0)
        self.phase = switch phase {
        case "snapshot": L("正在给数据库做快照…")
        case "copy": L("正在复制文件…")
        case "verify": L("正在校验…")
        case "restore": L("正在恢复文件…")
        case "upgrade": L("正在打开恢复的库…")
        default: phase
        }
        fraction = total > 0 ? min(1, done / total) : nil
    }

    private func describe(_ result: [String: Any]?, operation: Operation, relaunch: Bool) -> Outcome {
        guard let result, result["ok"] as? Bool == true else {
            let error = result?["error"] as? String ?? L("命令没有正常结束。")
            return Outcome(ok: false, text: relaunch ? L("%@ 重新启动 Candid 以继续使用。", error) : error, relaunch: relaunch)
        }
        let bytes = ByteCountFormatter.string(fromByteCount: Int64(result["bytes"] as? Int ?? 0), countStyle: .file)
        switch operation {
        case .backup:
            let path = result["path"] as? String ?? ""
            let missing = result["missing"] as? Int ?? 0
            let note = missing > 0 ? L("有 %@ 个原件在快照后已被清理，没有带上。", missing) : ""
            return Outcome(ok: true, text: L("已备份 %@ 个文件（%@）到 %@。%@", result["files"] as? Int ?? 0, bytes, path, note), reveal: path)
        case .restore:
            let moved = (result["moved_aside"] as? [String])?.first.map { L("原有数据在 %@。", $0) } ?? ""
            return Outcome(ok: true, text: L("已恢复 %@ 个文件（%@）。%@重新启动后生效。", result["files"] as? Int ?? 0, bytes, moved), relaunch: true)
        case .move:
            let old = (result["old"] as? String).map { L("旧数据保留在 %@，确认无误后可以删除。", $0) } ?? ""
            return Outcome(ok: true, text: L("时间线库已移到 %@。%@重新启动后生效。", result["to"] as? String ?? "", old), relaunch: true)
        }
    }

    /// Runs the helper and hands every output line (progress, then the result) to `line` on the main actor.
    nonisolated private static func execute(_ executable: URL, _ arguments: [String], line: @escaping @MainActor (String) -> Void) async {
        let process = Process()
        process.executableURL = executable
        process.arguments = arguments
        let out = Pipe()
        process.standardOutput = out
        process.standardError = FileHandle.nullDevice
        do { try process.run() } catch {
            let failure = (try? JSONSerialization.data(withJSONObject: ["ok": false, "error": error.localizedDescription])).flatMap { String(data: $0, encoding: .utf8) } ?? ""
            await line(failure)
            return
        }
        do {
            for try await text in out.fileHandleForReading.bytes.lines { await line(text) }
        } catch {}
        process.waitUntilExit()
    }

    /// Opens a fresh copy of the App once this one has quit, so the core starts against the restored or moved data.
    static func relaunch() {
        let bundle = Bundle.main.bundleURL.path
        let script = "while kill -0 \(ProcessInfo.processInfo.processIdentifier) 2>/dev/null; do sleep 0.2; done; /usr/bin/open \"$0\""
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = ["-c", script, bundle]
        try? process.run()
        NSApp.terminate(nil)
    }
}
