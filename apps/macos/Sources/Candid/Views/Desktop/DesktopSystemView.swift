import SwiftUI
import CandidLocalization
import DesktopCore

/// 诊断: environment, run state and sampling counters. Values are selectable for bug reports.
struct DesktopSystemView: View {
    @Bindable var session: DesktopSession

    var body: some View {
        SettingsPage(title: L("诊断")) {
            SettingsGroup(title: L("环境")) {
                ForEach(session.systemRows) { row in
                    value(row.label, row.value)
                    HairlineDivider()
                }
            }
            SettingsGroup(title: L("运行")) {
                value(L("状态"), session.statusText); HairlineDivider()
                value(L("原始记录"), "\(session.statistics.recordCount)"); HairlineDivider()
                value(L("截图"), "\(session.statistics.screenshotCount)"); HairlineDivider()
                value(L("占用"), ByteCountFormatter.string(fromByteCount: session.statistics.storageBytes, countStyle: .file))
            } accessory: {
                QuietButton(title: L("刷新"), action: session.refresh)
            }
            SettingsGroup(title: L("采样")) {
                let d = session.samplingDiagnostics
                value(L("触发信号"), "\(d.signals)"); HairlineDivider()
                value(L("内容读取"), "\(d.contentReads)"); HairlineDivider()
                value(L("无变化检查"), "\(d.unchangedChecks)"); HairlineDivider()
                value(L("保存记录"), "\(d.snapshotsSaved)"); HairlineDivider()
                value(L("截图尝试"), "\(d.imageAttempts)"); HairlineDivider()
                value(L("截图保存"), "\(d.imageSaved)"); HairlineDivider()
                value(L("列表刷新"), "\(d.listReloads)")
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
