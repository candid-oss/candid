import Foundation
import CandidClient

/// Mail scope, level rules and live bodies on top of the connector channel (agent-platform-v1 §2.0.2).
/// The core decides every level; this side edits the configuration, asks for previews and shows what comes back.
extension ConnectorsModel {
    // MARK: Scope and preview

    /// The Mail app is connected but nothing is imported yet: the user still has to pick a scope and confirm.
    var needsConfirmation: Bool {
        guard let source = appleMail, source.connected, case .object(let config) = source.config, case .bool(false)? = config["import_confirmed"] else { return false }
        return true
    }

    var scopeChanged: Bool { appleMail.map { scopeDraft != MailScope(config: $0.config) } ?? false }

    var preview: MailPreviewCounts? { appleMail.flatMap { mailPreviews[$0.id] }.map(MailPreviewCounts.init) }

    /// Reads the Mail app's accounts and folders and starts the draft from the saved scope.
    func loadScope() async {
        guard !isDemo, let source = appleMail, source.connected else { return }
        do {
            scopeOptions = try JSONDecoder().decode(MailScopeOptions.self, from: await call(["action": "mail_scope", "source_id": .string(source.id)]))
            scopeDraft = MailScope(config: source.config)
            scopeLoaded = true; error = nil
        } catch { self.error = Self.describe(error) }
        await previewScope()
    }

    /// Counts what the draft scope would bring, from mail headers only. Nothing is saved or stored.
    func previewScope() async {
        guard !isDemo, let source = appleMail, source.connected else { return }
        var config = source.config
        scopeDraft.apply(to: &config)
        do {
            mailPreviews[source.id] = try JSONDecoder().decode(MailPreview.self, from: await call(["action": "dry_run", "source_id": .string(source.id), "config": config]))
        } catch is CancellationError { return } catch { mailPreviews[source.id] = nil; self.error = Self.describe(error) }
    }

    /// Saves the draft scope; with `confirm` the import starts (it never starts without this).
    func saveScope(confirm: Bool) async {
        guard let source = appleMail else { return }
        var config = source.config
        scopeDraft.apply(to: &config)
        if confirm { config.set(["import_confirmed"], .bool(true)) }
        await run("scope") {
            _ = try await self.call(["action": "update", "source_id": .string(source.id), "config": config])
            self.notice = confirm ? "已开始导入。" : "范围已保存。"
        }
        if error == nil, !isDemo { await loadScope(); await loadRecent() }
    }

    // MARK: Rules

    var builtinReplied: Bool { mailSources.first.map { flag($0.config, ["builtin_replied"]) ?? true } ?? true }

    func setBuiltinReplied(_ on: Bool) async { await updateMail { $0.set(["builtin_replied"], .bool(on)) }; markRulesChanged() }

    /// Replaces the ordered rule list on every mail source (rules are shared). Incomplete rules are not saved.
    func setRules(_ rules: [MailRule]) async {
        let saved = rules.filter(\.isValid)
        let json = CoreJSON.array(saved.map(\.json))
        await updateMail { $0.set(["rules"], json) }
        if isDemo { mailRules = saved }
        markRulesChanged()
    }

    func moveRule(from: Int, to: Int) async {
        guard mailRules.indices.contains(from), mailRules.indices.contains(to), from != to else { return }
        var rules = mailRules
        rules.insert(rules.remove(at: from), at: to)
        await setRules(rules)
    }

    func saveRule(_ rule: MailRule) async {
        var rules = mailRules
        if let i = rules.firstIndex(where: { $0.id == rule.id }) { rules[i] = rule } else { rules.append(rule) }
        await setRules(rules)
    }

    func removeRule(_ rule: MailRule) async { await setRules(mailRules.filter { $0.id != rule.id }) }

    func markRulesChanged() { relevelPending = error == nil && mailSources.contains { $0.events > 0 } }

    func syncRules() {
        let stored: [MailRule]
        if let source = mailSources.first, case .object(let config) = source.config, case .array(let list)? = config["rules"] { stored = list.compactMap(MailRule.init(json:)) } else { stored = [] }
        // Keep ids (and with them the editor's open state) for rules that did not change.
        mailRules = stored.enumerated().map { i, rule in
            var rule = rule
            if mailRules.indices.contains(i), mailRules[i].json == rule.json { rule.id = mailRules[i].id }
            return rule
        }
    }

    /// Levels the stored mail again under the current rules.
    func relevel() async {
        await run("relevel") {
            struct Answer: Decodable {
                struct Source: Decodable { struct Stats: Decodable { var changed: Int }; var stats: Stats? }
                var sources: [Source]
            }
            let answer = try JSONDecoder().decode(Answer.self, from: await self.call(["action": "relevel"]))
            let changed = answer.sources.reduce(0) { $0 + ($1.stats?.changed ?? 0) }
            self.relevelPending = false
            self.notice = changed == 0 ? "已有邮件的级别都符合新规则。" : "已按新规则重新定级：\(changed) 封改了级别。"
        }
        if error == nil { await loadRecent() }
    }

    // MARK: Stored mail and live bodies

    func loadRecent() async {
        guard !isDemo, !mailSources.isEmpty else { return }
        struct Listing: Decodable { var messages: [MailMessage] }
        do { recentMail = try JSONDecoder().decode(Listing.self, from: await call(["action": "mail_recent", "limit": .int(30)])).messages }
        catch { self.error = Self.describe(error) }
    }

    /// Opens one mail: its body is read from the Mail app now and never stored.
    func open(_ message: MailMessage) async {
        if isDemo { notice = "合成样例不连接真实数据。"; return }
        do {
            openedMail = try JSONDecoder().decode(OpenedMail.self, from: await call(["action": "mail_body", "event_id": .string(message.id)]))
            error = nil
        } catch {
            openedMail = nil
            self.error = Self.describe(error)
        }
    }

    func closeMail() { openedMail = nil }

    /// Raises (or lowers) the opened mail, or its whole thread, by hand. A manual level is not overridden by rules.
    func setLevel(_ level: MailLevel, thread: Bool) async {
        guard let message = openedMail?.message else { return }
        await run("level") {
            _ = try await self.call(["action": "set_level", "event_id": .string(message.id), "level": .string(level.rawValue), "scope": .string(thread ? "thread" : "message")])
            self.notice = thread ? "整个会话已设为「\(level.title)」。" : "已设为「\(level.title)」。"
        }
        if error == nil { await loadRecent(); await open(message) }
    }
}
