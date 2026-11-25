import AppKit
import CandidLocalization
import SwiftUI

/// Settings → 关于: version, license, the privacy promise and the third-party notices shipped in the bundle.
struct AboutSettingsView: View {
    static let repository = URL(string: "https://github.com/candid-oss/candid")!
    @ViewState private var notices: String?

    private var version: String {
        let info = Bundle.main.infoDictionary
        let short = info?["CFBundleShortVersionString"] as? String ?? "dev"
        let build = info?["CFBundleVersion"] as? String
        return build.map { "\(short) (\($0))" } ?? short
    }

    var body: some View {
        SettingsPage(title: L("关于")) {
            SettingsGroup(title: "Candid") {
                SettingRow(title: L("版本"), detail: version) { EmptyView() }
                HairlineDivider()
                SettingRow(title: L("许可证"), detail: L("Apache License 2.0，开源")) {
                    QuietButton(title: L("查看")) { open("LICENSE") }
                }
                HairlineDivider()
                SettingRow(title: L("源代码"), detail: "github.com/candid-oss/candid") {
                    QuietButton(title: L("打开"), emphasized: true) { NSWorkspace.shared.open(Self.repository) }
                }
                HairlineDivider()
                SettingRow(title: L("首次使用引导"), detail: L("重新选择记录范围、授权和 AI 连接")) {
                    QuietButton(title: L("打开引导")) { OnboardingLinks.showGuide() }
                }
            }
            HealthSettingsGroup()
            SettingsGroup(title: L("隐私")) {
                Text(L("没有遥测、统计或账号。资料只保存在这台 Mac 上；只有你为某个功能配置了远程模型时，那部分内容才会发给该服务。每次启动默认暂停采集，菜单栏圆点始终显示是否在记录。"))
                    .typography(.secondary).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    .padding(.vertical, Space.row)
            }
            SettingsGroup(title: L("第三方软件")) {
                if let notices {
                    ScrollView {
                        Text(notices).font(.system(.caption, design: .monospaced)) // style-exempt: license text, not UI
                            .textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .frame(height: 320)
                } else {
                    SettingRow(title: L("开源组件许可"), detail: L("Rust 核心使用的开源组件及其许可证全文")) {
                        QuietButton(title: L("显示")) { notices = Self.text("THIRD_PARTY_LICENSES.txt") ?? L("这个构建里没有附带许可清单。") }
                    }
                }
            }
        }
    }

    private func open(_ name: String) {
        if let url = Bundle.main.url(forResource: name, withExtension: nil) { NSWorkspace.shared.open(url) }
        else { NSWorkspace.shared.open(Self.repository.appendingPathComponent("blob/main/\(name)")) }
    }

    static func text(_ name: String) -> String? {
        Bundle.main.url(forResource: name, withExtension: nil).flatMap { try? String(contentsOf: $0, encoding: .utf8) }
    }
}
