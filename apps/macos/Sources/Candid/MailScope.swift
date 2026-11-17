import Foundation
import CandidLocalization
import CandidClient

// Value types for the mail scope and level rules (agent-platform-v1 §2.0.2). They read and write the core's mail
// configuration (`MailConfig` in candid-connectors) and hold no behaviour of their own: the core decides levels.

enum MailLevel: String, CaseIterable, Hashable, Sendable {
    case ignore, metadata, normal, important
    var title: String {
        switch self { case .ignore: L("忽略"); case .metadata: L("只存元数据"); case .normal: L("普通"); case .important: L("重点") }
    }
    /// Levels a rule or a hand edit may set (a single mail is never "ignored" by hand).
    static let forRules: [MailLevel] = [.metadata, .normal, .important, .ignore]
}

enum MailField: String, CaseIterable, Hashable, Sendable {
    case from, domain, to, subject_contains, list, mailbox
    var title: String {
        switch self {
        case .from: L("发件人是")
        case .domain: L("发件人域名是")
        case .to: L("收件人包含")
        case .subject_contains: L("主题包含")
        case .list: L("群发邮件头")
        case .mailbox: L("文件夹是")
        }
    }
    var placeholder: String {
        switch self {
        case .from: "name@example.com"
        case .domain: "example.com"
        case .to: "me@example.com"
        case .subject_contains: L("报价")
        case .list: ""
        case .mailbox: L("role:inbox 或 文件夹名")
        }
    }
}

struct MailCondition: Identifiable, Equatable, Sendable {
    var id = UUID()
    var field: MailField
    /// For `list`: "yes" (has bulk headers) or "no".
    var value: String
}

/// One user rule: every condition must match; the first matching rule in the list decides.
struct MailRule: Identifiable, Equatable, Sendable {
    var id = UUID()
    var conditions: [MailCondition]
    var level: MailLevel

    var isValid: Bool { !conditions.isEmpty && conditions.allSatisfy { $0.field == .list || !$0.value.trimmingCharacters(in: .whitespaces).isEmpty } }

    var summary: String {
        conditions.map { c in
            c.field == .list ? (c.value == "no" ? L("没有群发邮件头") : L("带群发邮件头")) : "\(c.field.title) \(c.value)"
        }.joined(separator: L(" 且 "))
    }

    init(conditions: [MailCondition], level: MailLevel) { self.conditions = conditions; self.level = level }

    init?(json: CoreJSON) {
        guard case .object(let rule) = json, case .object(let when)? = rule["when"], case .string(let level)? = rule["level"], let lv = MailLevel(rawValue: level) else { return nil }
        var found: [MailCondition] = []
        for field in MailField.allCases {
            switch (field, when[field.rawValue]) {
            case (.list, .bool(let on)?): found.append(MailCondition(field: .list, value: on ? "yes" : "no"))
            case (_, .string(let text)?): found.append(MailCondition(field: field, value: text))
            default: break
            }
        }
        self.init(conditions: found, level: lv)
    }

    var json: CoreJSON {
        var when: [String: CoreJSON] = [:]
        for c in conditions {
            when[c.field.rawValue] = c.field == .list ? .bool(c.value != "no") : .string(c.value.trimmingCharacters(in: .whitespaces))
        }
        return .object(["when": .object(when), "level": .string(level.rawValue)])
    }
}

/// The import scope of one mail source.
struct MailScope: Equatable, Sendable {
    /// Account ids to read; empty means every account.
    var accounts: Set<String> = []
    /// Mailbox names or roles (`role:inbox`) to import from.
    var folders: [String] = ["role:inbox", "role:sent"]
    /// 0 means everything Mail holds.
    var backfillDays = 90

    static let backfillChoices: [(Int, String)] = [(30, L("30 天")), (90, L("90 天")), (365, L("1 年")), (0, L("全部"))]

    init() {}

    init(config: CoreJSON) {
        guard case .object(let c) = config else { return }
        if case .array(let a)? = c["accounts"] { accounts = Set(a.compactMap { if case .string(let s) = $0 { s } else { nil } }) }
        if case .object(let m)? = c["mailboxes"], case .array(let a)? = m["include"] { folders = a.compactMap { if case .string(let s) = $0 { s } else { nil } } }
        switch c["backfill_days"] {
        case .int(let d)?: backfillDays = Int(d)
        case .null?: backfillDays = 0
        default: break
        }
    }

    func apply(to config: inout CoreJSON) {
        config.set(["accounts"], .array(accounts.sorted().map { .string($0) }))
        config.set(["mailboxes", "include"], .array(folders.map { .string($0) }))
        config.set(["backfill_days"], backfillDays == 0 ? .null : .int(Int64(backfillDays)))
    }
}

/// Accounts and folders in the local Mail app (`mail_scope`): counts only.
struct MailScopeOptions: Decodable, Equatable {
    struct Mailbox: Decodable, Equatable { var name: String; var role: String; var never: Bool; var count: Int }
    struct Account: Decodable, Equatable, Identifiable {
        var id: String; var scheme: String; var mailboxes: [Mailbox]
        var address: String? = nil
        var description: String? = nil
        /// The mailbox's own address when known, so accounts can be told apart.
        var title: String {
            if let address, !address.isEmpty { return address }
            if let description, !description.isEmpty { return description }
            return scheme == "local" ? L("本机邮箱（在我的 Mac 上）") : L("未识别地址的账户")
        }
        var kind: String { ["ews": "Exchange", "imap": "IMAP", "pop": "POP", "local": L("本机"), "imaps": "IMAP"][scheme] ?? scheme.uppercased() }
        var count: Int { mailboxes.filter { !$0.never }.reduce(0) { $0 + $1.count } }
    }
    var accounts: [Account]

    struct Folder: Equatable, Identifiable { var pattern: String; var title: String; var count: Int; var id: String { pattern } }

    /// One choice per role (inbox, sent, archive) and per other folder name, summed over accounts. Junk, trash and drafts are left out.
    var folders: [Folder] {
        var order: [String] = []
        var byPattern: [String: Folder] = [:]
        for account in accounts {
            for box in account.mailboxes where !box.never {
                let known = ["inbox": L("收件箱"), "sent": L("已发送"), "archive": L("归档")]
                let pattern = known[box.role] != nil ? "role:\(box.role)" : box.name
                let title = known[box.role] ?? box.name
                if byPattern[pattern] == nil { order.append(pattern); byPattern[pattern] = Folder(pattern: pattern, title: title, count: 0) }
                byPattern[pattern]!.count += box.count
            }
        }
        let rank = ["role:inbox": 0, "role:sent": 1, "role:archive": 2]
        return order.compactMap { byPattern[$0] }.sorted { (rank[$0.pattern] ?? 3, $0.title) < (rank[$1.pattern] ?? 3, $1.title) }
    }
}

/// What the preview shows, from the dry run's level counts (headers only).
struct MailPreviewCounts: Equatable {
    var total: Int
    var metadata: Int
    var normal: Int
    var important: Int
    var ignored: Int
    var more: Bool
    var stored: Int

    init(_ preview: ConnectorsModel.MailPreview) {
        let l = preview.stats.levels
        let candidates = l["important_candidate"] ?? 0
        metadata = l["metadata"] ?? 0
        normal = max(0, (l["normal"] ?? 0) - candidates)
        important = (l["important"] ?? 0) + candidates
        ignored = l["ignore"] ?? 0
        total = metadata + normal + important + ignored
        more = preview.stats.more
        stored = preview.already_stored ?? 0
    }
}

/// A stored mail as the list shows it (no body).
struct MailMessage: Decodable, Identifiable, Equatable {
    var event_id: String
    var title: String?
    var start_ms: Int64?
    var thread_key: String?
    var level: String
    var level_basis: String?
    var important_candidate: Bool
    var direction: String?
    var from_name: String?
    var id: String { event_id }
    var mailLevel: MailLevel { MailLevel(rawValue: level) ?? .normal }
}

/// A mail opened in the App: its body as the source holds it now (`mail_body`).
struct OpenedMail: Decodable, Equatable {
    struct Body: Decodable, Equatable {
        var state: String
        var subject: String
        var from_name: String?
        var from_address: String?
        var body: String
        var quoted: String
    }
    var message: MailMessage
    var body: Body
    var sourceDeleted: Bool { body.state == "source_deleted" }
}
