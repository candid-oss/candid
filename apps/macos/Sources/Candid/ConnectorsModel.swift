import AIProviders
import Foundation
import Observation
import CandidClient

/// The App side of the core's ingest connectors (`/local/connectors`): mail (Mail.app, IMAP) and browser history.
/// The core only ever answers with counts and codes. IMAP passwords live in the Keychain and reach the core in
/// memory through `secrets`, again on every core start.
@MainActor @Observable final class ConnectorsModel {
    struct Source: Decodable, Identifiable, Equatable {
        var source_id: String
        var kind: String
        var display_name: String
        var account_ref: String? = nil
        var secret_ref: String? = nil
        var has_secret: Bool? = nil
        var config: CoreJSON
        var policy: CoreJSON? = nil
        var status: String
        var last_sync_ms: Int64? = nil
        var last_error: String? = nil
        var needs: String? = nil
        var events: Int
        var id: String { source_id }
        /// Connected, including a source whose last sync failed (`error`); only revoking disconnects.
        var connected: Bool { status != "revoked" }
        var browser: String? { if case .object(let o) = config, case .string(let b)? = o["browser"] { return b }; return nil }
    }

    struct BrowserPreview: Decodable, Equatable {
        var visits: Int
        var distinct_urls: Int
        var domains: Int
        var first_ms: Int64?
        var last_ms: Int64?
        var skipped_urls: Int
    }

    struct MailPreview: Decodable, Equatable {
        struct Stats: Decodable, Equatable { var scanned: Int; var levels: [String: Int]; var more: Bool }
        var would_insert: Int
        var threads: Int
        var stats: Stats
        /// Of the mail in scope, how much is stored already (an import only adds the rest).
        var already_stored: Int? = nil
    }

    typealias Transport = @Sendable (Data) async throws -> Data

    static let browsers: [(id: String, name: String)] = [("safari", "Safari"), ("chrome", "Chrome")]

    var sources: [Source] = []
    var browserPreviews: [String: BrowserPreview] = [:]
    var mailPreviews: [String: MailPreview] = [:]
    /// The rules shown in the editor; refreshed from the stored configuration, ids kept for rules that did not change.
    var mailRules: [MailRule] = []
    /// Mail scope under review (not saved until the user confirms): the local Mail app's folders and the draft choice.
    var scopeOptions: MailScopeOptions?
    var scopeDraft = MailScope()
    var scopeLoaded = false
    /// Newest stored mail, the one opened (body read live from Mail), and whether the rules changed since the last re-level.
    var recentMail: [MailMessage] = []
    var openedMail: OpenedMail?
    var relevelPending = false
    var busy: String?
    var error: String?
    var notice: String?
    var loaded = false
    let isDemo: Bool
    private let secrets: SecretStore
    private let transport: Transport?

    /// `transport` nil means the running core's local channel.
    init(secrets: SecretStore = ConnectorsModel.keychain, transport: Transport? = nil, demo: Bool = false) {
        self.secrets = secrets; self.transport = transport; isDemo = demo
        if demo { applyDemo() }
    }

    nonisolated static let keychain = KeychainSecretStore(service: "io.github.candid-oss.candid.connectors")

    var appleMail: Source? { sources.first { $0.kind == "mail.apple" } }
    var imapAccounts: [Source] { sources.filter { $0.kind == "mail.imap" && $0.status != "revoked" } }
    var mailSources: [Source] { sources.filter { $0.kind.hasPrefix("mail.") && $0.connected } }
    func browserSource(_ browser: String) -> Source? { sources.first { $0.kind == "browser.history" && $0.browser == browser } }

    // MARK: Status for the list

    var mailStatus: (state: StatusDot.State, text: String) {
        let live = mailSources
        if live.isEmpty { return (.off, loaded || isDemo ? "未连接" : "检查中…") }
        if live.contains(where: { $0.needs != nil }) { return (.attention, "需要处理") }
        return (.on, live.count == 1 ? "已连接" : "\(live.count) 个来源")
    }

    var browserStatus: (state: StatusDot.State, text: String) {
        let live = sources.filter { $0.kind == "browser.history" && $0.connected }
        if live.isEmpty { return (.off, loaded || isDemo ? "未连接" : "检查中…") }
        if live.contains(where: { $0.needs != nil }) { return (.attention, "需要处理") }
        return (.on, "已连接，未导入")
    }

    static func state(_ source: Source?) -> (state: StatusDot.State, text: String) {
        guard let source, source.status != "revoked" else { return (.off, "未连接") }
        switch source.needs {
        case "full_disk_access": return (.attention, "需要完全磁盘访问")
        case "credentials": return (.attention, "需要密码")
        case "unsupported_format": return (.attention, "格式暂不支持")
        default: break
        }
        if source.status == "paused" { return (.off, "已暂停") }
        if let error = source.last_error { return (.attention, message(error)) }
        return (.on, source.kind == "browser.history" ? "可读取" : "已连接")
    }

    // MARK: Commands

    func refresh() async {
        guard !isDemo else { return }
        do {
            struct Listing: Decodable { var sources: [Source] }
            sources = try JSONDecoder().decode(Listing.self, from: await call(["action": "list"])).sources
            syncRules()
            loaded = true; error = nil
        } catch { self.error = Self.describe(error) }
    }

    /// Re-sends every stored IMAP password to the core (it keeps them in memory only).
    func pushSecrets() async {
        guard !isDemo else { return }
        if !loaded { await refresh() }
        let refs = sources.filter { $0.kind == "mail.imap" && $0.connected }.compactMap(\.secret_ref), secrets = self.secrets
        // Off the main thread and without a prompt, like every background Keychain read.
        let found = await Task.detached(priority: .utility) { () -> [String: String] in
            var found: [String: String] = [:]
            for ref in refs { if let value = try? secrets.secretWithoutPrompt(for: ref) { found[ref] = value } }
            return found
        }.value
        let values = found.mapValues { CoreJSON.string($0) }
        guard !values.isEmpty else { return }
        _ = try? await call(["action": "secrets", "secrets": .object(values)])
        await refresh()
    }

    /// Connects the local Mail app. Nothing is imported until the scope is confirmed (`confirmImport`).
    /// `mailRoot` points tests at a synthetic tree; the App always reads Mail's own data.
    func authorizeAppleMail(mailRoot: String? = nil) async {
        await run("mail.apple") {
            var config: [String: CoreJSON] = ["import_confirmed": .bool(false)]
            if let mailRoot { config["mail_root"] = .string(mailRoot) }
            _ = try await self.call(["action": "authorize", "kind": "mail.apple", "config": .object(config)])
            self.notice = "已连接「邮件」App。先选范围、看预览，确认后才开始导入。"
        }
        await loadScope()
    }

    /// Stores the password in the Keychain first; the core only learns its reference and, in memory, its value.
    func addIMAP(address: String, host: String, port: Int, password: String, tls: String = "tls") async {
        let address = address.trimmingCharacters(in: .whitespaces).lowercased()
        let host = host.trimmingCharacters(in: .whitespaces).lowercased()
        let ref = "mail.imap:\(address)@\(host)"
        await run("mail.imap") {
            try self.secrets.setSecret(password, for: ref)
            _ = try await self.call(["action": "secrets", "secrets": .object([ref: .string(password)])])
            let config: CoreJSON = .object(["imap": .object(["host": .string(host), "port": .int(Int64(port)), "tls": .string(tls)])])
            do {
                _ = try await self.call(["action": "authorize", "kind": "mail.imap", "account_ref": .string(address), "secret_ref": .string(ref), "config": config])
            } catch {
                try? self.secrets.deleteSecret(for: ref)
                throw error
            }
            self.notice = "已添加 \(address)。首次同步时登录邮箱。"
        }
    }

    func updatePassword(_ source: Source, password: String) async {
        guard let ref = source.secret_ref else { return }
        await run(source.id) {
            try self.secrets.setSecret(password, for: ref)
            _ = try await self.call(["action": "secrets", "secrets": .object([ref: .string(password)])])
            _ = try await self.call(["action": "sync", "source_id": .string(source.id)])
        }
    }

    /// `historyPath` points tests at a synthetic database; the App always reads the browser's own file.
    func connectBrowser(_ browser: String, historyPath: String? = nil) async {
        await run("browser:" + browser) {
            struct Answer: Decodable { var source_id: String }
            var config: [String: CoreJSON] = ["browser": .string(browser)]
            if let historyPath { config["path"] = .string(historyPath) }
            let answer = try JSONDecoder().decode(Answer.self, from: await self.call(["action": "authorize", "kind": "browser.history", "config": .object(config)]))
            await self.refresh()
            await self.preview(answer.source_id)
        }
    }

    /// Counts what an import would bring, without writing anything.
    func preview(_ id: String) async {
        guard !isDemo, let source = sources.first(where: { $0.id == id }), source.connected else { return }
        do {
            let data = try await call(["action": "dry_run", "source_id": .string(id)])
            if source.kind == "browser.history" {
                struct Answer: Decodable { var result: BrowserPreview }
                browserPreviews[id] = try JSONDecoder().decode(Answer.self, from: data).result
            } else {
                mailPreviews[id] = try JSONDecoder().decode(MailPreview.self, from: data)
            }
        } catch { browserPreviews[id] = nil; mailPreviews[id] = nil; await refresh() }
    }

    func sync(_ source: Source? = nil) async {
        await run(source?.id ?? "sync") {
            var body: [String: CoreJSON] = ["action": "sync"]
            if let source { body["source_id"] = .string(source.id) }
            _ = try await self.call(body)
        }
    }

    /// Revoking stops reading; stored mail stays unless `deleteStored`. An IMAP password leaves the Keychain too.
    func revoke(_ source: Source, deleteStored: Bool = false) async {
        await run(source.id) {
            _ = try await self.call(["action": "revoke", "source_id": .string(source.id), "delete_stored": .bool(deleteStored)])
            if let ref = source.secret_ref {
                try? self.secrets.deleteSecret(for: ref)
                _ = try? await self.call(["action": "secrets", "secrets": .object([ref: .null])])
            }
            self.browserPreviews[source.id] = nil; self.mailPreviews[source.id] = nil
        }
    }

    // MARK: Mail rules (shared by every mail source)

    var builtinRules: Bool { mailSources.first.map { flag($0.config, ["builtin_rules"]) ?? true } ?? true }
    var aiJudge: Bool { mailSources.first.map { flag($0.config, ["ai_judge", "enabled"]) ?? false } ?? false }

    func setBuiltinRules(_ on: Bool) async { await updateMail { $0.set(["builtin_rules"], .bool(on)) }; markRulesChanged() }
    func setAIJudge(_ on: Bool) async { await updateMail { $0.set(["ai_judge", "enabled"], .bool(on)) } }

    /// What happens to mail no rule decides: "metadata" (default) or "normal"; the user's choice.
    var defaultLevel: String {
        guard case .object(let o)? = mailSources.first?.config, case .string(let v)? = o["default_level"] else { return "metadata" }
        return v
    }
    func setDefaultLevel(_ level: String) async { await updateMail { $0.set(["default_level"], .string(level)) }; markRulesChanged() }

    /// Whether AI (the attached agent and MCP clients) may read mail; off until the user allows it.
    var aiCanReadMail: Bool { mailSources.first.flatMap { $0.policy.flatMap { flag($0, ["agent_visible"]) } } ?? false }
    func setAICanReadMail(_ on: Bool) async {
        if isDemo { return }
        await run("rules") {
            for source in self.mailSources {
                _ = try await self.call(["action": "update", "source_id": .string(source.id), "config": source.config, "agent_visible": .bool(on)])
            }
        }
    }

    func updateMail(_ change: @escaping (inout CoreJSON) -> Void) async {
        if isDemo { for i in sources.indices where sources[i].kind.hasPrefix("mail.") { change(&sources[i].config) }; return }
        await run("rules") {
            for source in self.mailSources {
                var config = source.config
                change(&config)
                _ = try await self.call(["action": "update", "source_id": .string(source.id), "config": config])
            }
        }
    }

    func flag(_ json: CoreJSON, _ path: [String]) -> Bool? {
        var node = json
        for key in path { guard case .object(let o) = node, let next = o[key] else { return nil }; node = next }
        if case .bool(let b) = node { return b }
        return nil
    }

    // MARK: Plumbing

    func run(_ key: String, _ work: @escaping () async throws -> Void) async {
        guard !isDemo else { notice = "合成样例不连接真实数据。"; return }
        busy = key; error = nil; notice = nil
        defer { busy = nil }
        var failure: String?
        do { try await work() } catch { failure = Self.describe(error) }
        await refresh()
        if let failure { error = failure }
    }

    func call(_ body: [String: CoreJSON]) async throws -> Data {
        let data = try JSONEncoder().encode(body)
        if let transport { return try await transport(data) }
        guard let channel = TimelineService.shared.channel else { throw CoreError.unreachable("时间线库尚未打开") }
        return try await channel.send("POST", "/local/connectors", body: data, timeout: 90).checked()
    }

    static func describe(_ error: Error) -> String {
        if case CoreError.service(_, let code, _) = error { return message(code) }
        if error is KeychainError { return "无法写入钥匙串" }
        return error.localizedDescription
    }

    static func message(_ code: String) -> String {
        [
            "full_disk_access_required": "需要在系统设置里给 Candid 开「完全磁盘访问」",
            "waiting_authorization": "需要邮箱密码",
            "imap_auth_failed": "邮箱登录失败，检查地址和应用专用密码",
            "imap_unreachable": "连不上邮件服务器",
            "imap_tls_failed": "和邮件服务器的加密连接失败",
            "already_authorized": "这个来源已经连接",
            "account_ref_required": "请填写邮箱地址",
            "imap_settings_required": "请填写服务器地址和端口",
            "insecure_transport_refused": "只允许加密连接",
            "history_not_found": "没有找到这个浏览器的历史记录",
            "unsupported_browser": "暂不支持这个浏览器",
            "unknown_mail_version": "这个版本的「邮件」App 暂不支持",
            "unknown_mail_schema": "这个版本的「邮件」App 暂不支持",
            "unknown_history_schema": "这个浏览器版本暂不支持",
            "source_revoked": "这个来源已断开",
            "source_deleted": "来源已删除，正文无法读取",
            "live_read_unavailable": "这个来源暂不支持现读正文",
            "mail_data_not_found": "没有找到「邮件」App 的数据",
            "unknown_event": "这封邮件已不在时间线里",
        ][code] ?? "操作没有完成（\(code)）"
    }

    // MARK: Demo

    private func applyDemo() {
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        let mailConfig: CoreJSON = .object(["builtin_rules": .bool(true), "ai_judge": .object(["enabled": .bool(false)])])
        sources = [
            Source(source_id: "demo-mail", kind: "mail.apple", display_name: "邮件 App", config: mailConfig, status: "active", last_sync_ms: now - 600_000, events: 1284),
            Source(source_id: "demo-imap", kind: "mail.imap", display_name: "IMAP", account_ref: "me@example.test", secret_ref: "mail.imap:me@example.test@imap.example.test",
                   has_secret: false, config: mailConfig, status: "active", last_error: "waiting_authorization", needs: "credentials", events: 0),
            Source(source_id: "demo-safari", kind: "browser.history", display_name: "safari 历史", config: .object(["browser": .string("safari")]), status: "active", events: 0),
        ]
        browserPreviews["demo-safari"] = BrowserPreview(visits: 1840, distinct_urls: 612, domains: 94, first_ms: now - 90 * 86_400_000, last_ms: now - 120_000, skipped_urls: 37)
        mailRules = [MailRule(conditions: [MailCondition(field: .domain, value: "example-corp.com"), MailCondition(field: .to, value: "me@example.test")], level: .normal),
                     MailRule(conditions: [MailCondition(field: .subject_contains, value: "报价")], level: .important)]
        scopeOptions = MailScopeOptions(accounts: [.init(id: "DEMO-ACCOUNT", scheme: "imap", mailboxes: [
            .init(name: "INBOX", role: "inbox", never: false, count: 2_100), .init(name: "Sent Messages", role: "sent", never: false, count: 640),
            .init(name: "Archive", role: "archive", never: false, count: 500), .init(name: "Junk", role: "junk", never: true, count: 90)])])
        mailPreviews["demo-mail"] = MailPreview(would_insert: 3_240, threads: 1_100, stats: .init(scanned: 3_240, levels: ["metadata": 2_730, "normal": 510, "important_candidate": 80], more: false))
        recentMail = [MailMessage(event_id: "demo-1", title: "Re: 下周例会改到周三", start_ms: now - 3_600_000, thread_key: "t1", level: "metadata", level_basis: "default", important_candidate: false, direction: "in", from_name: "李娜")]
        scopeLoaded = true
        loaded = true
    }
}

extension CoreJSON {
    /// Sets a nested object value, creating objects on the way.
    mutating func set(_ path: [String], _ value: CoreJSON) {
        guard let key = path.first else { self = value; return }
        var object: [String: CoreJSON] = { if case .object(let o) = self { return o }; return [:] }()
        var child = object[key] ?? .object([:])
        child.set(Array(path.dropFirst()), value)
        object[key] = child
        self = .object(object)
    }
}
