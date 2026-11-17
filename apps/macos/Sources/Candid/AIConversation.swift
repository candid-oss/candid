import Foundation

// Conversations with Pi on the AI page (agent-platform-v1 §4.2): kept only on this Mac, one JSON file each, listed
// under 「历史对话」. They are working notes of the interaction, not facts of the timeline (CONTRIBUTING.md: agents).

/// Something from the 「今天」 column brought into the chat with 「问 AI」.
struct AIContextItem: Codable, Identifiable, Equatable, Hashable, Sendable {
    /// e.g. `event:<id>`, `reminder:<id>`, `proposal:<id>`, `commitment:<id>`, `speaker:<rec>|<key>`, `review:<id>`.
    var id: String
    /// 日程, 提醒, 提案, 承诺, 说话人, 回顾.
    var kind: String
    var title: String
    var detail: String = ""

    var chip: String { "\(kind) · \(title)" }
    var promptLine: String { "- \(kind)：\(title)" + (detail.isEmpty ? "" : "（\(detail)）") + (refID.map { " [\($0)]" } ?? "") }
    /// What Pi can use: a store id (proposal, memory, event as `[[id]]`-citable record), or a system calendar item
    /// as `event:<item>@<start>` / `reminder:<item>` for `item_id` in an edit proposal.
    var refID: String? {
        if id.hasPrefix("event:") || id.hasPrefix("reminder:") { return id }
        for prefix in ["proposal:", "commitment:", "review:", "record:"] where id.hasPrefix(prefix) { return String(id.dropFirst(prefix.count)) }
        return nil
    }
}

/// A write or a proposal Pi made during a turn, drawn as a small card under its answer.
struct AIChatAction: Codable, Identifiable, Equatable, Sendable {
    enum Kind: String, Codable, Sendable {
        /// Inside the store: already done, can be undone.
        case write
        /// Outside the App: waits for 批准.
        case proposal
    }
    /// `write_id` or `proposal_id`.
    var id: String
    var kind: Kind
    var tool: String
    var summary: String
    /// write: `done`, `undone`; proposal: the proposal's status (`pending`, `approved`, `executed`, `failed`, `rejected`).
    var status: String
    var detail: String?
}

/// A timeline record Pi looked at; cited in its answer as `[[event_id]]` and drawn as a chip (「录音 11月10日 10:14」).
struct AIRecordRef: Codable, Equatable, Hashable, Sendable {
    var eventID: String
    var kind: String
    var start: String?
    var title: String?

    var kindLabel: String {
        if kind.hasPrefix("speech") || kind.hasPrefix("audio") || kind.contains("recording") { return "录音" }
        if kind.hasPrefix("mail") { return "邮件" }
        if kind.hasPrefix("image") { return "截图" }
        if kind.hasPrefix("screen") || kind.hasPrefix("app") || kind.hasPrefix("window") { return "屏幕" }
        if kind.hasPrefix("calendar") || kind.hasPrefix("reminder") { return "日程" }
        if kind.hasPrefix("agent") { return "AI 会话" }
        if kind.hasPrefix("browser") || kind.hasPrefix("web") { return "网页" }
        if kind.hasPrefix("note") { return "笔记" }
        return "记录"
    }

    private static let time: DateFormatter = { let f = DateFormatter(); f.locale = Locale(identifier: "zh_CN"); f.dateFormat = "M月d日 HH:mm"; return f }()
    var label: String { kindLabel + (AgendaDate.parse(start).map { " " + Self.time.string(from: $0) } ?? "") }
}

/// One tool call of a turn: shown live while it runs, then folded into the steps line (expandable to the arguments).
struct AIToolCall: Codable, Identifiable, Equatable, Sendable {
    enum State: String, Codable, Sendable { case running, done, failed }
    var id: String
    var tool: String
    var arguments: String
    var state: State = .running
    /// 「12 条」, or the error.
    var outcome: String?
    var records: [AIRecordRef] = []
}

struct AIChatMessage: Codable, Identifiable, Equatable, Sendable {
    enum Role: String, Codable, Sendable { case user, assistant, note }
    var id = UUID()
    var role: Role
    var text: String
    var at = Date()
    var context: [AIContextItem] = []
    /// What Pi looked up during the turn (tool labels).
    var steps: [String] = []
    var actions: [AIChatAction] = []
    /// Tool calls of this answer (nil in conversations saved before they were kept).
    var calls: [AIToolCall]? = nil
    /// The user stopped this answer.
    var stopped: Bool? = nil

    /// 「查了 4 条记录 · 录音 11月12日 15:30、邮件 2 条」, or the tools used when nothing was found.
    var stepsSummary: String? {
        let calls = calls ?? []
        guard !calls.isEmpty else { return steps.isEmpty ? nil : "查了 " + steps.joined(separator: "、") }
        var seen = Set<String>(), records: [AIRecordRef] = []
        for r in calls.flatMap(\.records) where seen.insert(r.eventID).inserted { records.append(r) }
        if records.isEmpty {
            var tools: [String] = []
            for c in calls where !tools.contains(PiToolResult.step(c.tool)) { tools.append(PiToolResult.step(c.tool)) }
            return "用了 " + tools.joined(separator: "、")
        }
        var kinds: [String] = [], byKind: [String: [AIRecordRef]] = [:]
        for r in records { if byKind[r.kindLabel] == nil { kinds.append(r.kindLabel) }; byKind[r.kindLabel, default: []].append(r) }
        let parts = kinds.prefix(3).map { k in byKind[k]!.count == 1 ? byKind[k]![0].label : "\(k) \(byKind[k]!.count) 条" }
        return "查了 \(records.count) 条记录 · " + parts.joined(separator: "、") + (kinds.count > 3 ? " 等" : "")
    }
}

struct AIConversation: Codable, Identifiable, Equatable, Sendable {
    var id = UUID()
    var title = "新对话"
    var created = Date()
    var updated = Date()
    var messages: [AIChatMessage] = []
}

extension AIConversation {
    /// The last answer in one line, without Markdown marks or citations.
    var preview: String {
        guard let last = messages.last(where: { $0.role == .assistant && !$0.text.isEmpty }) ?? messages.last else { return "" }
        var t = last.text
        for (pattern, with) in [("\\[\\[[^\\]]*\\]\\]", "〔记录〕"), ("\\[([^\\]]*)\\]\\([^)]*\\)", "$1"), ("[*_`#>|]", ""), ("(?m)^\\s*([-+•]|\\d+[.)])\\s+", ""), ("\\s+", " ")] {
            t = t.replacingOccurrences(of: pattern, with: with, options: .regularExpression)
        }
        return t.trimmingCharacters(in: .whitespaces)
    }

    /// How many different records Pi looked at or cited in this conversation.
    var recordCount: Int {
        var ids = Set(messages.flatMap { $0.calls ?? [] }.flatMap(\.records).map(\.eventID))
        let cited = try! NSRegularExpression(pattern: "\\[\\[\\s*([^\\]\\s]+)\\s*\\]\\]")
        for m in messages where m.role == .assistant {
            let ns = m.text as NSString
            for r in cited.matches(in: m.text, range: NSRange(location: 0, length: ns.length)) { ids.insert(ns.substring(with: r.range(at: 1))) }
        }
        return ids.count
    }

    func matches(_ query: String) -> Bool {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !q.isEmpty else { return true }
        return title.lowercased().contains(q) || messages.contains { $0.text.lowercased().contains(q) || $0.context.contains { $0.title.lowercased().contains(q) } }
    }
}

/// 「历史对话」 in groups: 今天, 昨天, 更早 (newest first within each).
struct AIHistoryGroups: Equatable {
    struct Group: Equatable, Identifiable { var title: String; var items: [AIConversation]; var id: String { title } }
    var groups: [Group]

    static func make(_ conversations: [AIConversation], now: Date, query: String = "", calendar: Calendar = .current) -> AIHistoryGroups {
        let today = calendar.startOfDay(for: now)
        let yesterday = calendar.date(byAdding: .day, value: -1, to: today)!
        var buckets: [String: [AIConversation]] = [:]
        for c in conversations where c.matches(query) {
            let key = c.updated >= today ? "今天" : c.updated >= yesterday ? "昨天" : "更早"
            buckets[key, default: []].append(c)
        }
        return AIHistoryGroups(groups: ["今天", "昨天", "更早"].compactMap { k in
            buckets[k].map { Group(title: k, items: $0.sorted { $0.updated > $1.updated }) }
        })
    }
}

/// One file per conversation under the App's data folder (tests pass their own folder).
struct AIConversationStore: Sendable {
    let directory: URL

    static var standard: AIConversationStore {
        AIConversationStore(directory: FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Candid/conversations", isDirectory: true))
    }

    private var encoder: JSONEncoder { let e = JSONEncoder(); e.dateEncodingStrategy = .iso8601; e.outputFormatting = [.sortedKeys]; return e }
    private var decoder: JSONDecoder { let d = JSONDecoder(); d.dateDecodingStrategy = .iso8601; return d }

    func save(_ conversation: AIConversation) throws {
        guard !conversation.messages.isEmpty else { return }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let url = directory.appendingPathComponent(conversation.id.uuidString + ".json")
        try encoder.encode(conversation).write(to: url, options: [.atomic])
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
    }

    /// Newest first; unreadable files are skipped.
    func list() -> [AIConversation] {
        let files = (try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)) ?? []
        return files.filter { $0.pathExtension == "json" }
            .compactMap { try? decoder.decode(AIConversation.self, from: Data(contentsOf: $0)) }
            .sorted { $0.updated > $1.updated }
    }

    func delete(_ id: UUID) {
        try? FileManager.default.removeItem(at: directory.appendingPathComponent(id.uuidString + ".json"))
    }
}

// MARK: Pi's tool results

enum PiToolResult {
    /// The JSON inside `<candid_data …>` (the extension wraps every result in it).
    static func data(_ result: String) -> [String: Any]? {
        guard let open = result.range(of: "<candid_data"), let headEnd = result[open.upperBound...].firstIndex(of: ">"),
              let close = result.range(of: "</candid_data>") , headEnd < close.lowerBound else { return nil }
        let body = result[result.index(after: headEnd)..<close.lowerBound].trimmingCharacters(in: .whitespacesAndNewlines)
        return (try? JSONSerialization.jsonObject(with: Data(body.utf8))) as? [String: Any]
    }

    /// The card a finished tool call becomes, if it wrote something or filed a proposal.
    static func action(tool: String, arguments: String, result: String, isError: Bool) -> AIChatAction? {
        guard !isError, let data = data(result) else { return nil }
        let args = (try? JSONSerialization.jsonObject(with: Data(arguments.utf8))) as? [String: Any] ?? [:]
        if tool == "pv_propose", let id = data["proposal_id"] as? String {
            let payload = args["payload"] as? [String: Any] ?? [:]
            let title = payload["title"] as? String ?? "提案"
            let what = AgendaProposal.label(args["kind"] as? String ?? "")
            return AIChatAction(id: id, kind: .proposal, tool: tool, summary: "\(what)：\(title)", status: data["status"] as? String ?? "pending",
                                detail: (payload["due"] ?? payload["start"]) as? String)
        }
        guard let writeID = data["write_id"] as? String else { return nil }
        let summary: String
        switch tool {
        case "pv_annotate":
            if let title = args["title"] as? String { summary = "改标题为「\(title)」" }
            else { summary = "存到备注：" + ((args["notes"] as? String) ?? "") }
        case "pv_mark_reference": summary = "标为资料：" + ((args["label"] as? String) ?? "")
        case "pv_commitment": summary = (args["decision"] as? String == "confirm" ? "确认承诺" : "忽略承诺候选")
        case "pv_save_draft": summary = "保存回顾草稿"
        default: summary = tool
        }
        return AIChatAction(id: writeID, kind: .write, tool: tool, summary: summary, status: "done")
    }

    /// Every record a result mentions (any object with `event_id` and `kind`), in order, without repeats.
    static func records(_ result: String) -> [AIRecordRef] {
        guard let data = data(result) else { return [] }
        var out: [AIRecordRef] = [], seen = Set<String>()
        func walk(_ v: Any) {
            if let d = v as? [String: Any] {
                if let id = d["event_id"] as? String, let kind = d["kind"] as? String, seen.insert(id).inserted {
                    let app = (d["app"] as? [String: Any])?["name"] as? String
                    out.append(AIRecordRef(eventID: id, kind: kind, start: d["start"] as? String, title: (d["title"] as? String) ?? app))
                }
                for (_, x) in d { walk(x) }
            } else if let a = v as? [Any] { a.forEach(walk) }
        }
        walk(data)
        return out
    }

    /// One line on what a finished call returned.
    static func outcome(_ result: String, isError: Bool) -> String {
        if isError { return String(result.prefix(160)) }
        guard let data = data(result) else { return "完成" }
        for key in ["hits", "events", "items", "episodes", "memories", "records", "sessions"] {
            if let list = data[key] as? [Any] { return "\(list.count) 条" }
        }
        return data["write_id"] != nil ? "已写入" : data["proposal_id"] != nil ? "已提出" : "完成"
    }

    /// What the page says while a call runs (「正在搜索「预算」…」).
    static func live(_ tool: String, arguments: String) -> String {
        let args = (try? JSONSerialization.jsonObject(with: Data(arguments.utf8))) as? [String: Any] ?? [:]
        if let q = (args["q"] ?? args["query"]) as? String, !q.isEmpty { return "正在搜索「\(q)」…" }
        let verbs = ["pv_status": "正在看各来源状态…", "pv_overview": "正在看这段时间的概况…", "pv_search": "正在搜索…", "pv_timeline": "正在翻时间线…",
                     "pv_context": "正在看前后的记录…", "pv_get_event": "正在打开一条记录…", "pv_episodes": "正在看片段…", "pv_episode": "正在打开片段…",
                     "pv_records": "正在读记录修订…", "pv_agent_session": "正在读 AI 会话…", "pv_memories": "正在看记忆与候选…",
                     "pv_annotate": "正在写标题或备注…", "pv_mark_reference": "正在标为资料…", "pv_commitment": "正在处理承诺候选…",
                     "pv_save_draft": "正在保存草稿…", "pv_undo": "正在撤销…", "pv_propose": "正在拟提案…"]
        return verbs[tool] ?? "正在处理…"
    }

    /// The short label of a lookup shown above an answer (「查了 时间线、搜索」).
    static func step(_ tool: String) -> String {
        ["pv_status": "状态", "pv_overview": "概览", "pv_search": "搜索", "pv_timeline": "时间线", "pv_context": "前后记录",
         "pv_get_event": "记录", "pv_episodes": "片段", "pv_episode": "片段", "pv_records": "记录修订", "pv_agent_session": "AI 会话",
         "pv_memories": "记忆", "pv_annotate": "备注", "pv_mark_reference": "资料", "pv_commitment": "承诺", "pv_save_draft": "草稿",
         "pv_undo": "撤销", "pv_propose": "提案"][tool] ?? tool
    }
}
