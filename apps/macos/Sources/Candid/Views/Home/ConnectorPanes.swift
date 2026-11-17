import AppKit
import CandidLocalization
import SwiftUI

/// Mail sources: the Mail.app data on this Mac and any IMAP accounts, then the rules every mail source shares.
struct MailConnectorPane: View {
    let model: ConnectorsModel
    @ViewState private var adding = false
    @ViewState private var address = ""
    @ViewState private var host = ""
    @ViewState private var port = "993"
    @ViewState private var password = ""
    @ViewState private var revoking: ConnectorsModel.Source?
    @ViewState private var passwordFor: ConnectorsModel.Source?
    @ViewState private var newPassword = ""

    var body: some View {
        VStack(alignment: .leading, spacing: Space.l) {
            if model.needsConfirmation {
                InlineMessage(kind: .note, text: L("已连接「邮件」App，但还没有导入任何邮件。先选好范围、看预览，点「导入」后才开始。"))
            }
            if !model.mailSources.isEmpty {
                HStack(spacing: Space.s) {
                    ConnectorStat(value: "\(model.mailSources.reduce(0) { $0 + $1.events })", label: L("已存邮件"))
                    ConnectorStat(value: "\(model.mailSources.count)", label: L("来源"))
                    ConnectorStat(value: lastSync, label: L("最近同步"))
                }
            }
            SettingsGroup(title: L("来源")) {
                appleRow
                if let source = model.appleMail, revoking?.id == source.id { revokeConfirm(source) }
                ForEach(model.imapAccounts) { account in
                    HairlineDivider()
                    imapRow(account)
                    if passwordFor?.id == account.id { passwordForm(account) }
                    if revoking?.id == account.id { revokeConfirm(account) }
                }
                HairlineDivider()
                if adding { addForm } else {
                    SettingRow(title: L("IMAP 登录"), detail: L("Gmail 等邮箱用应用专用密码；密码存在钥匙串")) {
                        QuietButton(title: L("添加账户"), emphasized: true) { withAnimation(Motion.spring) { adding = true } }
                    }
                }
            } accessory: {
                if !model.mailSources.isEmpty {
                    QuietButton(title: model.busy == "sync" ? L("同步中…") : L("立即同步"), emphasized: true) { Task { await model.sync() } }
                        .disabled(model.busy != nil)
                }
            }
            MailScopePanel(model: model)
            MailStoragePanel(model: model)
            MailRulesPanel(model: model)
            MailRecentPanel(model: model)
            messages
        }
        .task {
            if model.appleMail?.connected == true, !model.scopeLoaded { await model.loadScope() }
            await model.loadRecent()
        }
    }

    private var appleRow: some View {
        let source = model.appleMail.flatMap { $0.status == "revoked" ? nil : $0 }
        let state = ConnectorsModel.state(source)
        return SettingRow(title: L("本机「邮件」App"), detail: source == nil ? L("读取其中登录的全部账户；需要给 Candid 开「完全磁盘访问」") : detail(source!)) {
            HStack(spacing: Space.s) {
                StatusDot(state: state.state, text: state.text)
                if let source {
                    if source.needs == "full_disk_access" {
                        SecondaryButton(title: L("打开系统设置")) { openFullDiskAccess() }
                        QuietButton(title: L("重新检查"), emphasized: true) { Task { await model.sync(source) } }
                    }
                    sourceMenu(source)
                } else {
                    PrimaryButton(title: model.busy == "mail.apple" ? L("连接中…") : L("授权")) { Task { await model.authorizeAppleMail() } }
                        .disabled(model.busy != nil)
                }
            }
        }
    }

    private func imapRow(_ source: ConnectorsModel.Source) -> some View {
        let state = ConnectorsModel.state(source)
        return SettingRow(title: source.account_ref ?? source.display_name, detail: detail(source)) {
            HStack(spacing: Space.s) {
                StatusDot(state: state.state, text: state.text)
                if source.needs == "credentials" { SecondaryButton(title: L("输入密码")) { passwordFor = source } }
                sourceMenu(source)
            }
        }
    }

    private func sourceMenu(_ source: ConnectorsModel.Source) -> some View {
        MenuButton(title: L("管理")) {
            Button(L("立即同步")) { Task { await model.sync(source) } }
            if source.kind == "mail.imap" { Button(L("更新密码…")) { passwordFor = source } }
            Divider()  // style-exempt: menu separator
            Button(L("断开…"), role: .destructive) { revoking = source }
        }
        .disabled(model.busy != nil)
    }

    private func revokeConfirm(_ source: ConnectorsModel.Source) -> some View {
        InlinePanel {
            VStack(alignment: .leading, spacing: Space.s) {
                Text(L("断开%@后不再读取。已存邮件可以保留在时间线里，也可以一并删除。", name(source))).typography(.secondary).fixedSize(horizontal: false, vertical: true)
                HStack {
                    Spacer()
                    QuietButton(title: L("取消")) { revoking = nil }
                    QuietButton(title: L("断开并删除已存邮件"), role: .destructive) { revoking = nil; Task { await model.revoke(source, deleteStored: true) } }
                    PrimaryButton(title: L("断开，保留邮件")) { revoking = nil; Task { await model.revoke(source) } }
                }
            }
        }
        .padding(.bottom, Space.s)
    }

    private func passwordForm(_ source: ConnectorsModel.Source) -> some View {
        InlinePanel {
            HStack(alignment: .bottom, spacing: Space.s) {
                QuietField(title: L("%@ 的应用专用密码", source.account_ref ?? ""), text: $newPassword, secure: true)
                QuietButton(title: L("取消")) { passwordFor = nil; newPassword = "" }
                PrimaryButton(title: L("保存"), isDefault: true) {
                    let value = newPassword; passwordFor = nil; newPassword = ""
                    Task { await model.updatePassword(source, password: value) }
                }
                .disabled(newPassword.isEmpty)
            }
        }
        .padding(.bottom, Space.s)
    }

    private var addForm: some View {
        VStack(alignment: .leading, spacing: Space.s) {
            Text(L("添加 IMAP 账户")).typography(.body, .medium).padding(.top, Space.row)
            HStack(spacing: Space.s) {
                QuietField(title: L("邮箱地址"), text: $address, placeholder: "name@example.com")
                QuietField(title: L("服务器"), text: $host, placeholder: "imap.gmail.com")
                QuietField(title: L("端口"), text: $port, mono: true).frame(width: 80)
            }
            QuietField(title: L("应用专用密码"), text: $password, secure: true)
            Text(L("只读，加密连接。密码只存在这台 Mac 的钥匙串里，核心进程只在内存中使用。")).typography(.secondary).foregroundStyle(.secondary)
            HStack {
                Spacer()
                QuietButton(title: L("取消")) { reset() }
                PrimaryButton(title: model.busy == "mail.imap" ? L("添加中…") : L("添加"), isDefault: true) {
                    let values = (address, host, Int(port) ?? 993, password)
                    Task {
                        await model.addIMAP(address: values.0, host: values.1, port: values.2, password: values.3)
                        if model.error == nil { reset() }
                    }
                }
                .disabled(!address.contains("@") || host.isEmpty || password.isEmpty || Int(port) == nil || model.busy != nil)
            }
        }
        .padding(.bottom, Space.row)
        .onChange(of: address) { _, value in
            if host.isEmpty, let domain = value.split(separator: "@").last, value.contains("@"), domain.contains(".") {
                host = domain == "gmail.com" ? "imap.gmail.com" : domain == "qq.com" ? "imap.qq.com" : ""
            }
        }
    }

    @ViewBuilder private var messages: some View {
        if let error = model.error { InlineMessage(kind: .error, text: error) }
        if let notice = model.notice { InlineMessage(kind: .note, text: notice) }
    }

    private var lastSync: String {
        guard let ms = model.mailSources.compactMap(\.last_sync_ms).max() else { return L("尚未同步") }
        return Date(timeIntervalSince1970: Double(ms) / 1000).formatted(.relative(presentation: .named))
    }

    private func detail(_ source: ConnectorsModel.Source) -> String {
        let count = source.events > 0 ? L("已存 %@ 封", source.events) : L("尚未入库")
        guard let ms = source.last_sync_ms else { return count }
        return count + " · " + Date(timeIntervalSince1970: Double(ms) / 1000).formatted(.relative(presentation: .named)) + L("同步")
    }

    private func name(_ source: ConnectorsModel.Source) -> String { source.kind == "mail.apple" ? L("「邮件」App") : source.account_ref ?? source.display_name }

    private func reset() {
        withAnimation(Motion.spring) { adding = false }
        address = ""; host = ""; port = "993"; password = ""
    }

    private func toggle(_ value: Bool, _ set: @escaping (Bool) -> Void) -> some View {
        Toggle("", isOn: Binding(get: { value }, set: { set($0) })).toggleStyle(.switch).labelsHidden().controlSize(.small)
    }
}

/// Browser history: connect per browser and see what is readable. Nothing is written to the timeline in this version.
struct BrowserConnectorPane: View {
    let model: ConnectorsModel
    @ViewState private var revoking: ConnectorsModel.Source?

    var body: some View {
        VStack(alignment: .leading, spacing: Space.l) {
            let previews = ConnectorsModel.browsers.compactMap { model.browserSource($0.id).flatMap { model.browserPreviews[$0.id] } }
            if !previews.isEmpty {
                HStack(spacing: Space.s) {
                    ConnectorStat(value: "\(previews.reduce(0) { $0 + $1.visits })", label: L("可读访问记录"))
                    ConnectorStat(value: "\(previews.reduce(0) { $0 + $1.domains })", label: L("网站"))
                    ConnectorStat(value: L("关闭"), label: L("写入时间线"))
                }
            }
            SettingsGroup(title: L("浏览器")) {
                ForEach(Array(ConnectorsModel.browsers.enumerated()), id: \.offset) { index, browser in
                    if index > 0 { HairlineDivider() }
                    row(browser.id, browser.name)
                    if let source = model.browserSource(browser.id), revoking?.id == source.id {
                        InlinePanel {
                            HStack {
                                Text(L("断开后不再读取 %@ 的历史。", browser.name)).typography(.secondary)
                                Spacer()
                                QuietButton(title: L("取消")) { revoking = nil }
                                QuietButton(title: L("断开"), role: .destructive) { revoking = nil; Task { await model.revoke(source) } }
                            }
                        }
                        .padding(.bottom, Space.s)
                    }
                }
                HairlineDivider()
                SettingRow(title: L("写入时间线"), detail: L("这一版只读取和统计，不导入")) {
                    Toggle("", isOn: .constant(false)).toggleStyle(.switch).labelsHidden().controlSize(.small).disabled(true)
                }
            }
            if let error = model.error { InlineMessage(kind: .error, text: error) }
            if let notice = model.notice { InlineMessage(kind: .note, text: notice) }
        }
        .task {
            for browser in ConnectorsModel.browsers {
                if let source = model.browserSource(browser.id), source.connected, model.browserPreviews[source.id] == nil { await model.preview(source.id) }
            }
        }
    }

    private func row(_ id: String, _ name: String) -> some View {
        let source = model.browserSource(id).flatMap { $0.status == "revoked" ? nil : $0 }
        let state = ConnectorsModel.state(source)
        return SettingRow(title: name, detail: detail(source, id)) {
            HStack(spacing: Space.s) {
                StatusDot(state: state.state, text: state.text)
                if let source {
                    if source.needs == "full_disk_access" { SecondaryButton(title: L("打开系统设置")) { openFullDiskAccess() } }
                    MenuButton(title: L("管理")) {
                        Button(L("重新统计")) { Task { await model.preview(source.id) } }
                        Divider()  // style-exempt: menu separator
                        Button(L("断开…"), role: .destructive) { revoking = source }
                    }
                } else {
                    SecondaryButton(title: model.busy == "browser:" + id ? L("连接中…") : L("连接")) { Task { await model.connectBrowser(id) } }
                        .disabled(model.busy != nil)
                }
            }
        }
    }

    private func detail(_ source: ConnectorsModel.Source?, _ id: String) -> String {
        guard let source else { return id == "safari" ? L("需要「完全磁盘访问」") : L("默认用户") }
        guard let p = model.browserPreviews[source.id] else { return L("统计中…") }
        let since = p.first_ms.map { L("，最早 ") + Date(timeIntervalSince1970: Double($0) / 1000).formatted(.dateTime.month().day()) } ?? ""
        return L("%@ 次访问 · %@ 个网址%@", p.visits, p.distinct_urls, since)
    }

    private func title(_ id: String) -> String { ConnectorsModel.browsers.first { $0.id == id }?.name ?? id }
}

private struct ConnectorStat: View {
    let value: String
    let label: String
    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(value).typography(.section, digits: true)
            Text(label).typography(.secondary).foregroundStyle(.secondary)
        }
        .padding(.horizontal, Space.m).padding(.vertical, Space.s)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: Radius.medium, style: .continuous).fill(Surface.wash.color))
    }
}

private func openFullDiskAccess() {
    if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles") { NSWorkspace.shared.open(url) }
}
