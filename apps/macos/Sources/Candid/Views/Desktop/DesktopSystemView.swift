import SwiftUI
import DesktopCore

/// 诊断: environment, run state and sampling counters. Values are selectable for bug reports.
struct DesktopSystemView: View {
    @Bindable var session: DesktopSession

    var body: some View {
        SettingsPage(title: "诊断") {
            SettingsGroup(title: "环境") {
                ForEach(session.systemRows) { row in
                    value(row.label, row.value)
                    HairlineDivider()
                }
            }
            SettingsGroup(title: "运行") {
                value("状态", session.statusText); HairlineDivider()
                value("原始记录", "\(session.statistics.recordCount)"); HairlineDivider()
                value("截图", "\(session.statistics.screenshotCount)"); HairlineDivider()
                value("占用", ByteCountFormatter.string(fromByteCount: session.statistics.storageBytes, countStyle: .file))
            } accessory: {
                QuietButton(title: "刷新", action: session.refresh)
            }
            SettingsGroup(title: "采样") {
                let d = session.samplingDiagnostics
                value("触发信号", "\(d.signals)"); HairlineDivider()
                value("内容读取", "\(d.contentReads)"); HairlineDivider()
                value("无变化检查", "\(d.unchangedChecks)"); HairlineDivider()
                value("保存记录", "\(d.snapshotsSaved)"); HairlineDivider()
                value("截图尝试", "\(d.imageAttempts)"); HairlineDivider()
                value("截图保存", "\(d.imageSaved)"); HairlineDivider()
                value("列表刷新", "\(d.listReloads)")
            }
        }
    }

    private func value(_ title: String, _ text: String) -> some View {
        SettingRow(title: title) {
            Text(text).typography(.body, digits: true).foregroundStyle(.secondary)
                .multilineTextAlignment(.trailing).textSelection(.enabled)
        }
    }
}
