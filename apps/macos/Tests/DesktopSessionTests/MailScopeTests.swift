import AIProviders
import Foundation
import CandidClient
import SQLite3
import Testing
@testable import Candid

/// A synthetic `~/Library/Mail` (Envelope Index + .emlx). Nothing here reads the user's own mail.
private final class SyntheticMail {
    let root: URL
    private var db: OpaquePointer?
    let account = "SYNTHETIC-ACCOUNT"
    private(set) var inbox: Int64 = 0, sent: Int64 = 0, junk: Int64 = 0, trash: Int64 = 0
    private var count = 0

    init(in parent: URL) throws {
        root = parent.appendingPathComponent("Mail", isDirectory: true)
        try FileManager.default.createDirectory(at: root.appendingPathComponent("V10/MailData"), withIntermediateDirectories: true)
        #expect(sqlite3_open(root.appendingPathComponent("V10/MailData/Envelope Index").path, &db) == SQLITE_OK)
        // The core's mail sync may be reading the index while a test edits it; wait instead of failing with SQLITE_BUSY.
        sqlite3_busy_timeout(db, 10_000)
        exec("""
        CREATE TABLE mailboxes(ROWID INTEGER PRIMARY KEY, url TEXT, total_count INTEGER);
        CREATE TABLE subjects(ROWID INTEGER PRIMARY KEY, subject TEXT);
        CREATE TABLE addresses(ROWID INTEGER PRIMARY KEY, address TEXT, comment TEXT);
        CREATE TABLE message_global_data(ROWID INTEGER PRIMARY KEY, message_id INTEGER, message_id_header TEXT);
        CREATE TABLE messages(ROWID INTEGER PRIMARY KEY, message_id INTEGER, global_message_id INTEGER, remote_id INTEGER, sender INTEGER, subject INTEGER, date_sent INTEGER, date_received INTEGER, mailbox INTEGER, flags INTEGER, read INTEGER, deleted INTEGER, size INTEGER);
        """)
        inbox = mailbox("INBOX"); sent = mailbox("Sent Messages"); junk = mailbox("Junk"); trash = mailbox("Deleted Messages")
    }

    deinit { sqlite3_close(db) }

    private func exec(_ sql: String) { #expect(sqlite3_exec(db, sql, nil, nil, nil) == SQLITE_OK) }
    private func lastID() -> Int64 { sqlite3_last_insert_rowid(db) }

    private func mailbox(_ name: String) -> Int64 {
        exec("INSERT INTO mailboxes(url) VALUES('imap://\(account)/\(name.replacingOccurrences(of: " ", with: "%20"))')")
        return lastID()
    }

    /// Adds one message (and its .emlx) `ageDays` old; returns its index row.
    @discardableResult
    func add(_ mid: String, from: String, to: String = "me@example.test", subject: String, body: String, mailbox box: Int64, headers: String = "", ageDays: Double = 1) throws -> Int64 {
        let received = Int64(Date().timeIntervalSince1970 - ageDays * 86_400)
        let raw = "Message-ID: <\(mid)>\r\nFrom: \(from)\r\nTo: \(to)\r\nSubject: \(subject)\r\nDate: Tue, 10 Nov 2026 09:00:00 +0000\r\nMIME-Version: 1.0\r\n\(headers)Content-Type: text/plain; charset=utf-8\r\n\r\n\(body)\r\n"
        exec("INSERT INTO subjects(subject) VALUES('\(subject)')"); let subj = lastID()
        exec("INSERT INTO addresses(address,comment) VALUES('\(from.split(separator: "<").last.map { String($0).replacingOccurrences(of: ">", with: "") } ?? from)','')"); let addr = lastID()
        exec("INSERT INTO message_global_data(message_id,message_id_header) VALUES(1,'<\(mid)>')"); let global = lastID()
        exec("INSERT INTO messages(global_message_id,sender,subject,date_sent,date_received,mailbox,flags,read,deleted,size) VALUES(\(global),\(addr),\(subj),\(received),\(received),\(box),0,0,0,\(raw.utf8.count))")
        let row = lastID()
        count += 1
        let url = try mailboxURL(box)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        try ("\(raw.utf8.count)\n" + raw + "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict></dict></plist>\n").write(to: url.appendingPathComponent("\(row).emlx"), atomically: true, encoding: .utf8)
        return row
    }

    func expunge(_ row: Int64) { exec("DELETE FROM messages WHERE ROWID=\(row)") }
    func move(_ row: Int64, to box: Int64) { exec("UPDATE messages SET mailbox=\(box) WHERE ROWID=\(row)") }

    private func mailboxURL(_ box: Int64) throws -> URL {
        var url: String?
        var stmt: OpaquePointer?
        sqlite3_prepare_v2(db, "SELECT url FROM mailboxes WHERE ROWID=\(box)", -1, &stmt, nil)
        if sqlite3_step(stmt) == SQLITE_ROW { url = String(cString: sqlite3_column_text(stmt, 0)) }
        sqlite3_finalize(stmt)
        let name = try #require(url).components(separatedBy: "\(account)/").last!.replacingOccurrences(of: "%20", with: " ")
        return root.appendingPathComponent("V10/\(account)/\(name).mbox/STORE/Data/Messages", isDirectory: true)
    }
}

/// The mail pane's scope, rules and live bodies against a real core and a synthetic Mail tree.
@MainActor @Suite(.serialized) struct MailScopeTests {
    private func world() async throws -> (URL, SyntheticMail, ConnectorsModel) {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("mail-scope-" + UUID().uuidString, isDirectory: true)
        let channel = try await TestCore.ready(root, cutover: false).channel!
        let transport: ConnectorsModel.Transport = { body in try await channel.send("POST", "/local/connectors", body: body).checked() }
        let mail = try SyntheticMail(in: root)
        return (root, mail, ConnectorsModel(secrets: InMemorySecretStore(), transport: transport))
    }

    private func populate(_ mail: SyntheticMail) throws {
        try mail.add("a", from: "Alice <alice@example.test>", subject: "预算草案", body: "请确认预算。", mailbox: mail.inbox)
        try mail.add("b", from: "me@example.test", to: "alice@example.test", subject: "Re: 预算草案", body: "好的。", mailbox: mail.sent, headers: "In-Reply-To: <a>\r\n")
        try mail.add("s", from: "bob@other.test", subject: "你好", body: "看看这个。", mailbox: mail.inbox)
        try mail.add("n", from: "news@lists.example.test", subject: "周刊", body: "周刊正文。", mailbox: mail.inbox, headers: "List-Unsubscribe: <mailto:u@lists.example.test>\r\n")
        try mail.add("old", from: "bob@other.test", subject: "旧信", body: "很久以前。", mailbox: mail.inbox, ageDays: 200)
        try mail.add("junk", from: "x@other.test", subject: "垃圾", body: "垃圾。", mailbox: mail.junk)
    }

    @Test func scopeIsChosenAndPreviewedBeforeAnythingIsImported() async throws {
        let (root, mail, model) = try await world()
        defer { TestCore.finish(root) }
        try populate(mail)
        await model.refresh()
        await model.authorizeAppleMail(mailRoot: mail.root.path)
        #expect(model.error == nil && model.needsConfirmation)

        // Folders offered: inbox and sent only (junk, trash and drafts are never read); the default scope is both.
        let options = try #require(model.scopeOptions)
        #expect(options.folders.map(\.pattern) == ["role:inbox", "role:sent"])
        #expect(model.scopeDraft == MailScope())
        // Preview from headers only: 4 messages in 90 days (the 200-day-old one is out), counted per level.
        let first = try #require(model.preview)
        #expect(first.total == 4 && first.metadata == 2 && first.normal == 0 && first.important == 2)
        #expect(model.appleMail?.events == 0)                                  // nothing imported before confirmation
        await model.sync()
        #expect(model.appleMail?.events == 0)

        // A wider backfill is previewed without being saved; the saved scope stays 90 days.
        model.scopeDraft.backfillDays = 365
        await model.previewScope()
        #expect(model.preview?.total == 5)
        #expect(MailScope(config: try #require(model.appleMail).config).backfillDays == 90 && model.scopeChanged)
        // Sent only: the replied thread's own mail.
        model.scopeDraft = MailScope(); model.scopeDraft.folders = ["role:sent"]
        await model.previewScope()
        #expect(model.preview?.total == 1)

        // Confirming imports exactly the previewed scope.
        model.scopeDraft = MailScope()
        await model.saveScope(confirm: true)
        #expect(model.error == nil && !model.needsConfirmation)
        for _ in 0..<50 where model.appleMail?.events != 4 { try await Task.sleep(for: .milliseconds(100)); await model.refresh() }
        #expect(model.appleMail?.events == 4)
        await model.loadRecent()
        #expect(model.recentMail.count == 4)
        #expect(model.recentMail.first { $0.title == "你好" }?.mailLevel == .metadata)
        #expect(model.recentMail.first { $0.title == "预算草案" }.map { $0.mailLevel == .normal && $0.important_candidate } == true)
    }

    @Test func rulesAreOrderedEditedAndReappliedToImportedMail() async throws {
        let (root, mail, model) = try await world()
        defer { TestCore.finish(root) }
        try populate(mail)
        await model.refresh()
        await model.authorizeAppleMail(mailRoot: mail.root.path)
        await model.saveScope(confirm: true)
        for _ in 0..<50 where model.appleMail?.events != 4 { try await Task.sleep(for: .milliseconds(100)); await model.refresh() }
        #expect(model.mailRules.isEmpty && !model.relevelPending)

        // Two rules about the same sender: the first one wins; reordering flips it.
        let toNormal = MailRule(conditions: [MailCondition(field: .domain, value: "other.test")], level: .normal)
        let toImportant = MailRule(conditions: [MailCondition(field: .from, value: "BOB@other.test")], level: .important)
        await model.setRules([toNormal, toImportant])
        #expect(model.error == nil && model.mailRules.map(\.level) == [.normal, .important] && model.relevelPending)
        await model.relevel()
        #expect(model.error == nil && !model.relevelPending)
        await model.loadRecent()
        #expect(model.recentMail.first { $0.title == "你好" }?.level == "normal" && model.recentMail.first { $0.title == "你好" }?.level_basis == "rule:0")
        await model.moveRule(from: 1, to: 0)
        #expect(model.mailRules.map(\.level) == [.important, .normal])
        await model.relevel()
        await model.loadRecent()
        #expect(model.recentMail.first { $0.title == "你好" }?.level == "important")

        // Inline edit keeps the order; a built-in switch off sends the list mail to the default level; invalid rules are not saved.
        var edited = model.mailRules[1]; edited.conditions = [MailCondition(field: .subject_contains, value: "周刊")]; edited.level = .ignore
        await model.saveRule(edited)
        #expect(model.mailRules[1].summary == "主题包含 周刊" && model.mailRules[1].level == .ignore)
        await model.saveRule(MailRule(conditions: [MailCondition(field: .from, value: "  ")], level: .normal))
        #expect(model.mailRules.count == 2)
        await model.setBuiltinReplied(false)
        #expect(!model.builtinReplied && model.builtinRules)
        await model.removeRule(model.mailRules[1])
        #expect(model.mailRules.count == 1)
    }

    @Test func metadataOnlyMailOpensFromMailAndCanBeRaised() async throws {
        let (root, mail, model) = try await world()
        defer { TestCore.finish(root) }
        try mail.add("s", from: "bob@other.test", subject: "你好", body: "看看这个。", mailbox: mail.inbox)
        try mail.add("s2", from: "bob@other.test", subject: "Re: 你好", body: "第二封。", mailbox: mail.inbox, headers: "In-Reply-To: <s>\r\n")
        await model.refresh()
        await model.authorizeAppleMail(mailRoot: mail.root.path)
        await model.saveScope(confirm: true)
        for _ in 0..<50 where model.appleMail?.events != 2 { try await Task.sleep(for: .milliseconds(100)); await model.refresh() }
        await model.loadRecent()
        let message = try #require(model.recentMail.first { $0.title == "你好" })
        #expect(message.mailLevel == .metadata)

        await model.open(message)
        #expect(model.openedMail?.body.state == "ok" && model.openedMail?.body.body == "看看这个。" && model.openedMail?.sourceDeleted == false)
        // Raising the whole thread stores both bodies; opening again still reads live.
        await model.setLevel(.normal, thread: true)
        #expect(model.error == nil && model.openedMail?.message.mailLevel == .normal)
        await model.loadRecent()
        #expect(model.recentMail.allSatisfy { $0.mailLevel == .normal && $0.level_basis == "manual" })
        await model.setLevel(.important, thread: false)
        #expect(model.openedMail?.message.mailLevel == .important)
    }

    @Test func deletedSourceCannotBeRaisedAndIsReported() async throws {
        let (root, mail, model) = try await world()
        defer { TestCore.finish(root) }
        let row = try mail.add("s", from: "bob@other.test", subject: "你好", body: "看看这个。", mailbox: mail.inbox)
        await model.refresh()
        await model.authorizeAppleMail(mailRoot: mail.root.path)
        await model.saveScope(confirm: true)
        for _ in 0..<50 where model.appleMail?.events != 1 { try await Task.sleep(for: .milliseconds(100)); await model.refresh() }
        await model.loadRecent()
        let message = try #require(model.recentMail.first)
        await model.open(message)
        #expect(model.openedMail?.body.state == "ok")
        mail.move(row, to: mail.trash)                                         // trash counts as deleted
        await model.open(message)
        #expect(model.openedMail?.sourceDeleted == true)
        await model.setLevel(.normal, thread: false)
        #expect(model.error == "来源已删除，正文无法读取")
    }

    @Test func rulesAndCountsRoundTripThroughTheConfiguration() throws {
        let rule = MailRule(conditions: [MailCondition(field: .domain, value: "example-corp.com"), MailCondition(field: .list, value: "no")], level: .normal)
        let back = try #require(MailRule(json: rule.json))
        #expect(back.conditions.map(\.field) == [.domain, .list] && back.conditions[1].value == "no" && back.level == .normal && back.summary == "发件人域名是 example-corp.com 且 没有群发邮件头")
        #expect(!MailRule(conditions: [], level: .normal).isValid)
        var config = CoreJSON.object(["builtin_rules": .bool(true)])
        var scope = MailScope(); scope.backfillDays = 0; scope.accounts = ["A"]; scope.folders = ["role:inbox", "归档"]
        scope.apply(to: &config)
        #expect(MailScope(config: config) == scope)
        let preview = ConnectorsModel.MailPreview(would_insert: 10, threads: 3, stats: .init(scanned: 10, levels: ["metadata": 6, "normal": 3, "important": 1, "important_candidate": 2, "ignore": 1], more: false))
        let counts = MailPreviewCounts(preview)
        #expect(counts.metadata == 6 && counts.normal == 1 && counts.important == 3 && counts.ignored == 1 && counts.total == 11)
    }
}
