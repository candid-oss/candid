import CandidLocalization
import SwiftUI

/// 设置 → 快捷操作 → 系统: the login item and the switches for Shortcuts, `candid://` links and the Focus filter.
struct SystemIntegrationSettings: View {
    @AppStorage(SystemHooks.intentsKey) private var intents = true
    @AppStorage(SystemHooks.urlKey) private var links = true
    @AppStorage(SystemHooks.urlResumeKey) private var linkResume = false
    @AppStorage(SystemHooks.focusKey) private var focus = true
    private let login = LoginItem.shared

    var body: some View {
        SettingsGroup(title: L("系统")) {
            SwitchRow(title: L("登录时打开 Candid"), detail: L("开机后只在菜单栏待命，记录仍是暂停的。"),
                      isOn: Binding(get: { login.enabled }, set: { login.set($0) }))
            if login.needsApproval {
                SettingRow(title: L("等待系统允许"), detail: L("在系统设置的登录项里打开 Candid。")) {
                    QuietButton(title: L("打开登录项")) { login.openSystemSettings() }
                }
            }
            if let error = login.error { Text(error).typography(.secondary).foregroundStyle(.red) }
            HairlineDivider()
            SwitchRow(title: L("快捷指令"), detail: L("在快捷指令和聚焦里提供：开始、暂停、暂停一段时间、记录状态、今天做了什么。"), isOn: $intents)
            HairlineDivider()
            SwitchRow(title: L("candid:// 链接"), detail: L("candid://pause、pause?minutes=30、open?date=2026-11-25、search?q=…，供 Raycast、Alfred 和脚本调用。"), isOn: $links)
            SwitchRow(title: L("允许链接开始记录"), detail: L("candid://resume。关闭时链接只能暂停，网页或其他应用打开链接也不会开始记录。"), isOn: $linkResume)
                .disabled(!links).opacity(links ? 1 : 0.5)
            HairlineDivider()
            SwitchRow(title: L("专注模式过滤"), detail: L("在系统设置 → 专注模式里给某个模式添加 Candid 过滤器，打开「暂停电脑记录」。"), isOn: $focus)
        }
        .onAppear { login.refresh() }
    }
}
