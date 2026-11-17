import AIProviders
import Foundation
import CandidClient
import SQLite3
import Testing
@testable import Candid

/// The connections page against a real core: synthetic browser history, a loopback IMAP account and an
/// in-memory Keychain. Nothing reads the user's own mail or history.
@MainActor @Suite(.serialized) struct ConnectorsModelTests {
    private func core() async throws -> (URL, ConnectorsModel.Transport) {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("connectors-" + UUID().uuidString, isDirectory: true)
        let channel = try await TestCore.ready(root, cutover: false).channel!
        return (root, { body in try await channel.send("POST", "/local/connectors", body: body).checked() })
    }

    /// A Chrome-format history database with three visits to two web pages and one non-web URL.
    private func syntheticChromeHistory(in root: URL) throws -> String {
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let path = root.appendingPathComponent("History").path
        var db: OpaquePointer?
        #expect(sqlite3_open(path, &db) == SQLITE_OK)
        defer { sqlite3_close(db) }
        let now = Int64(Date().timeIntervalSince1970 * 1_000_000) + 11_644_473_600_000_000
        let sql = """
        CREATE TABLE urls(id INTEGER PRIMARY KEY, url LONGVARCHAR, title LONGVARCHAR, visit_count INTEGER DEFAULT 0, typed_count INTEGER DEFAULT 0, last_visit_time INTEGER, hidden INTEGER DEFAULT 0);
        CREATE TABLE visits(id INTEGER PRIMARY KEY, url INTEGER NOT NULL, visit_time INTEGER NOT NULL, from_visit INTEGER, transition INTEGER DEFAULT 0);
        INSERT INTO urls VALUES(1,'https://docs.example.test/a?token=x','合成文档',2,0,0,0);
        INSERT INTO urls VALUES(2,'https://news.example.test/','合成新闻',1,0,0,0);
        INSERT INTO urls VALUES(3,'chrome://settings','设置',1,0,0,0);
        INSERT INTO visits(url,visit_time) VALUES(1,\(now - 7_200_000_000)),(1,\(now - 3_600_000_000)),(2,\(now - 60_000_000)),(3,\(now - 30_000_000));
        """
        #expect(sqlite3_exec(db, sql, nil, nil, nil) == SQLITE_OK)
        return path
    }

    @Test func browserConnectsAndCountsWithoutImporting() async throws {
        let (root, transport) = try await core()
        defer { TestCore.finish(root) }
        let model = ConnectorsModel(secrets: InMemorySecretStore(), transport: transport)
        await model.refresh()
        #expect(model.loaded && model.sources.isEmpty && model.browserStatus.text == "未连接")

        await model.connectBrowser("chrome", historyPath: try syntheticChromeHistory(in: root.appendingPathComponent("chrome")))
        #expect(model.error == nil)
        let source = try #require(model.browserSource("chrome"))
        #expect(ConnectorsModel.state(source).text == "可读取" && model.browserStatus.text == "已连接，未导入")
        let preview = try #require(model.browserPreviews[source.id])
        #expect(preview.visits == 3 && preview.domains == 2 && preview.skipped_urls == 1)
        #expect(source.events == 0)                                  // counted, never written

        await model.connectBrowser("chrome", historyPath: root.appendingPathComponent("chrome/History").path)
        #expect(model.error == "这个来源已经连接")

        await model.revoke(source)
        #expect(model.browserSource("chrome")?.status == "revoked" && model.browserPreviews.isEmpty && model.browserStatus.text == "未连接")
    }

    @Test func imapPasswordStaysInKeychainAndReachesCoreInMemory() async throws {
        let (root, transport) = try await core()
        defer { TestCore.finish(root) }
        let keychain = InMemorySecretStore()
        let model = ConnectorsModel(secrets: keychain, transport: transport)
        await model.refresh()

        await model.addIMAP(address: "Me@Example.test", host: "127.0.0.1", port: 9, password: "app-password", tls: "none")
        #expect(model.error == nil)
        let account = try #require(model.imapAccounts.first)
        let ref = try #require(account.secret_ref)
        #expect(account.account_ref == "me@example.test" && ref == "mail.imap:me@example.test@127.0.0.1")
        #expect(try keychain.secret(for: ref) == "app-password" && account.has_secret == true)
        if case .object(let config) = account.config { #expect(!String(describing: config).contains("app-password")) }

        // A restarted core has forgotten the password; the App pushes it again from the Keychain.
        _ = try await transport(JSONEncoder().encode(["action": CoreJSON.string("secrets"), "secrets": .object([ref: .null])]))
        await model.refresh()
        #expect(model.imapAccounts.first?.has_secret == false)
        await model.pushSecrets()
        #expect(model.imapAccounts.first?.has_secret == true)

        // Rules apply to every mail source and keep the rest of the configuration.
        await model.setBuiltinRules(false)
        await model.setAIJudge(true)
        #expect(model.error == nil && !model.builtinRules && model.aiJudge)

        // Insecure transport to a remote host is refused before anything is stored.
        await model.addIMAP(address: "other@example.test", host: "imap.example.test", port: 143, password: "x", tls: "none")
        #expect(try model.error == "只允许加密连接" && keychain.secret(for: "mail.imap:other@example.test@imap.example.test") == nil)

        await model.revoke(account)
        #expect(try model.imapAccounts.isEmpty && keychain.secret(for: ref) == nil)
    }

    @Test func syntheticSessionNeverCallsTheCore() async throws {
        let model = ConnectorsModel(secrets: InMemorySecretStore(), transport: { _ in Issue.record("demo reached the core"); return Data() }, demo: true)
        #expect(model.mailStatus.state == .attention && model.browserStatus.text == "已连接，未导入")
        await model.authorizeAppleMail()
        await model.connectBrowser("chrome")
        await model.refresh()
        #expect(model.notice == "合成样例不连接真实数据。")
        await model.setBuiltinRules(false)
        #expect(!model.builtinRules)
    }
}
