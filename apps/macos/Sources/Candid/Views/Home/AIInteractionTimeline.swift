import SwiftUI
import CandidLocalization
import CandidClient
import TimelineConnectors

/// Reads only the authorised evidence view. Replies are fetched explicitly and never persisted here.
@MainActor @Observable final class AIInteractionTimeline {
    struct Item: Identifiable, Decodable, Equatable {
        var event_id: String
        var kind: String
        var start: String?
        var end: String?
        /// The session this instruction belongs to.
        var thread_id: String?
        var id: String { event_id }
        var date: Date? { Self.date(start) }
        /// Formatters are expensive to make (ICU tables); one of each, reused. ISO8601DateFormatter is thread safe.
        nonisolated(unsafe) private static let fractional: ISO8601DateFormatter = {
            let f = ISO8601DateFormatter(); f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]; return f
        }()
        nonisolated(unsafe) private static let whole = ISO8601DateFormatter()
        static func date(_ value: String?) -> Date? {
            guard let value else { return nil }
            return fractional.date(from: value) ?? whole.date(from: value)
        }
    }
    struct Entry: Decodable, Equatable { var event: Item?; var preview: String? }
    struct Page: Decodable { var entries: [Entry]; var next_cursor: String? }
    struct Paragraph: Decodable { var text: String }
    struct Detail: Decodable { var event: Item?; var payload: [String: CoreJSON]; var paragraphs: [Paragraph] }
    private(set) var entries: [Entry] = []
    private(set) var detail: Detail?
    private(set) var reply: String?
    /// True while the reply is being read from the session file.
    private(set) var replyLoading = false
    private(set) var error: String?
    /// A soft note under content that is shown anyway (the stored words, when the source file can't be reread).
    private(set) var sourceNote: String?
    /// Where an instruction was sent: tool, project folder and session title. Fetched on first hover, then kept.
    struct PromptSource: Equatable { var agent: String; var project: String?; var session: String? }
    private(set) var sources: [String: PromptSource] = [:]
    @ObservationIgnored private var sourceLoading: Set<String> = []
    var selected: String?
    private var generation = 0
    /// Synthetic instructions for `--mock`; nothing is read from real sessions.
    private(set) var isDemo = false
    private var selectionGeneration = 0
    typealias Query = @Sendable (String, Data) async throws -> Data
    private let query: Query
    typealias ReplyReader = @Sendable (String, String, String?) async throws -> AgentReadResult
    private let reader: ReplyReader
    init(query: Query? = nil, reader: ReplyReader? = nil) {
        self.reader = reader ?? { agent, ref, cursor in
            guard let channel = await MainActor.run(body: { TimelineService.shared.channel }) else { throw CoreError.unreachable(L("尚未就绪")) }
            let client = AgentCoreClient(transport: { path, body in
                try await channel.send(body == nil ? "GET" : "POST", path, body: body, timeout: 15).checked()
            })
            guard let connector = AgentConnectors.connector(agent: agent, client: client) else { throw CoreError.unreachable(L("来源不可读取")) }
            return try await connector.read(ref: ref, cursor: cursor, limit: 200)
        }
        self.query = query ?? { name, data in
            guard let channel = await MainActor.run(body: { TimelineService.shared.channel }) else { throw CoreError.unreachable(L("尚未就绪")) }
            return try await channel.tool(name, arguments: data)
        }
    }
    func loadSource(_ id: String) async {
        guard sources[id] == nil, sourceLoading.insert(id).inserted else { return }
        defer { sourceLoading.remove(id) }
        struct Answer: Decodable { var payload: [String: CoreJSON]; var thread: Thread?; struct Thread: Decodable { var title: String? } }
        guard let data = try? await query("get_event", JSONEncoder().encode(["event_id": CoreJSON.string(id)])),
              let answer = try? JSONDecoder().decode(Answer.self, from: data) else { return }
        func string(_ key: String) -> String? { if case .string(let v)? = answer.payload[key], !v.isEmpty { return v }; return nil }
        let agent = ["codex": "Codex", "claude_code": "Claude Code", "pi": "pi"][string("agent") ?? ""] ?? "AI"
        let project = string("project").map { URL(fileURLWithPath: $0).lastPathComponent }
        let title = answer.thread?.title?.trimmingCharacters(in: .whitespacesAndNewlines)
        sources[id] = PromptSource(agent: agent, project: project, session: title?.isEmpty == false ? title : nil)
    }

    /// Ways back to the original conversation: continue it in Terminal, or show its file.
    struct SourceLinks: Equatable {
        var resumeCommand: String?
        var file: URL?
        var project: URL?
        var isEmpty: Bool { resumeCommand == nil && file == nil && project == nil }
    }
    var sourceLinks: SourceLinks {
        guard let payload = detail?.payload, case .string(let agent)? = payload["agent"] else { return SourceLinks() }
        var links = SourceLinks()
        if case .array(let refs)? = payload["messages"], case .object(let first)? = refs.first,
           case .string(let path)? = first["source_file"], !path.isEmpty, FileManager.default.fileExists(atPath: path) {
            links.file = URL(fileURLWithPath: path)
        }
        var project: String?
        if case .string(let p)? = payload["project"], p.hasPrefix("/"), FileManager.default.fileExists(atPath: p) { project = p; links.project = URL(fileURLWithPath: p) }
        if case .string(let sessionRef)? = payload["session_ref"] {
            links.resumeCommand = Self.resumeCommand(agent: agent, sessionRef: sessionRef, project: project)
        }
        return links
    }
    /// Only plain session ids and quoted folders reach the shell.
    static func resumeCommand(agent: String, sessionRef: String, project: String?) -> String? {
        let key = String(sessionRef.split(separator: ":", maxSplits: 2).last ?? "")
        guard !key.isEmpty, key.unicodeScalars.allSatisfy({ CharacterSet.alphanumerics.contains($0) || $0 == "-" || $0 == "_" }) else { return nil }
        let run: String
        switch agent {
        case "codex": run = "codex resume \(key)"
        case "claude_code": run = "claude --resume \(key)"
        default: return nil
        }
        guard let project else { return run }
        return "cd '\(project.replacingOccurrences(of: "'", with: "'\\''"))' && \(run)"
    }

    func close() { selectionGeneration += 1; generation += 1; selected = nil; detail = nil; reply = nil; replyLoading = false }
    func clear() { selectionGeneration += 1; generation += 1; entries = []; selected = nil; detail = nil; reply = nil }
    func refresh(day: Date, search: String = "") async {
        generation += 1; let ticket = generation
        do {
            let start = Calendar.current.startOfDay(for: day)
            let end = Calendar.current.date(byAdding: .day, value: 1, to: start)!
            var args: [String: CoreJSON] = ["start": .string(start.ISO8601Format()), "end": .string(end.ISO8601Format()), "kinds": .array([.string("ai.prompt"), .string("ai.work"), .string("note")]), "level": .string("event"), "limit": .int(50)]
            var result: [Entry] = []
            repeat {
                let page = try JSONDecoder().decode(Page.self, from: await query("timeline", JSONEncoder().encode(args)))
                result += page.entries
                args["cursor"] = page.next_cursor.map(CoreJSON.string)
            } while args["cursor"] != nil && result.count < 5000 && !Task.isCancelled
            guard ticket == generation, !Task.isCancelled else { return }
            // Unchanged pages are not reassigned: this refresh runs every few seconds and the home page draws from it.
            let shown = search.isEmpty ? result : result.filter { $0.preview?.localizedCaseInsensitiveContains(search) == true }
            if shown != entries { entries = shown }
            if error != nil { error = nil }
            if let selected {
                if entries.contains(where: { $0.event?.id == selected }) { await open(selected, preserveReply: true) }
                else { self.selected = nil; detail = nil; reply = nil }
            }
        } catch {
            guard ticket == generation else { return }
            entries = []; detail = nil; reply = nil; selected = nil; self.error = L("AI 时间线暂不可用，点击重试")
        }
    }
    func open(_ id: String, preserveReply: Bool = false) async {
        if selected != id { selectionGeneration += 1 }
        let ticket = selectionGeneration
        selected = id; if !preserveReply { detail = nil; reply = nil; sourceNote = nil; replyLoading = false }
        do {
            var value = try JSONDecoder().decode(Detail.self, from: await query("get_event", JSONEncoder().encode(["event_id": CoreJSON.string(id), "include": CoreJSON.array([.string("text")])])))
            guard ticket == selectionGeneration, selected == id else { return }
            if value.event?.kind == "ai.prompt", case .string("transcript")? = value.payload["origin"],
               case .string(let agent)? = value.payload["agent"], case .array(let refs)? = value.payload["messages"] {
                // The words are stored; the source is reread only to confirm and refresh them. If that fails, the
                // stored words stay on screen with a note instead of the whole detail disappearing.
                do {
                    let wanted = refs.compactMap { raw -> String? in
                        guard case .object(let m) = raw, case .string(let ref)? = m["ref"] else { return nil }; return ref
                    }
                    guard wanted.count <= 20 else { throw CoreError.unreachable(L("追问较多，请从来源查看完整会话")) }
                    let expected = Dictionary(uniqueKeysWithValues: refs.compactMap { raw -> (String, String)? in
                        guard case .object(let m) = raw, case .string(let ref)? = m["ref"], case .string(let hash)? = m["source_hash"] else { return nil }
                        return (ref, hash)
                    })
                    var fetched: [String: AgentMessage] = [:]
                    for ref in wanted where fetched[ref] == nil {
                        // Long sessions span several pages; follow them (bounded) until the message turns up.
                        var cursor: String?, pages = 0
                        repeat {
                            let page = try await reader(agent, ref, cursor)
                            guard ticket == selectionGeneration, selected == id else { return }
                            for m in page.messages where wanted.contains(m.ref) && m.role == "user" && !m.reasoning && !m.truncated && (expected[m.ref] == nil || m.source_hash == expected[m.ref]) { fetched[m.ref] = m }
                            cursor = page.nextCursor; pages += 1
                        } while fetched[ref] == nil && cursor != nil && pages < 20
                    }
                    guard wanted.allSatisfy({ fetched[$0]?.text != nil }) else { throw CoreError.unreachable(L("原话来源暂不可读取")) }
                    value.paragraphs = wanted.compactMap { fetched[$0]?.text.map { Paragraph(text: $0) } }
                    // Recheck the persisted event after any source read.
                    _ = try await query("get_event", JSONEncoder().encode(["event_id": CoreJSON.string(id), "include": .array([.string("text")])]))
                    sourceNote = nil
                } catch {
                    guard ticket == selectionGeneration, selected == id else { return }
                    sourceNote = value.paragraphs.isEmpty ? nil : L("会话文件暂时读不到，显示的是保存时的原话")
                    if value.paragraphs.isEmpty { throw error }
                }
            }
            guard ticket == selectionGeneration, selected == id else { return }; detail = value; error = nil
        } catch { if ticket == selectionGeneration, selected == id { detail = nil; reply = nil; self.error = L("内容已不可读取") } }
    }
    var canReadReply: Bool {
        guard let detail, case .string(let agent)? = detail.payload["agent"], ["codex", "claude_code", "pi"].contains(agent),
              case .string? = detail.payload["message_ref"] ?? detail.payload["session_ref"] else { return false }
        return true
    }
    func readReply() async {
        guard let id = selected, !replyLoading else { return }
        let ticket = selectionGeneration
        replyLoading = true
        defer { if ticket == selectionGeneration { replyLoading = false } }
        // Recheck event visibility before following the reference.
        await open(id, preserveReply: true)
        guard let detail, case .string(let agent)? = detail.payload["agent"], case .string(let ref)? = detail.payload["message_ref"] ?? detail.payload["session_ref"] else { return }
        let start = entries.compactMap(\.event).first { $0.id == id }?.date ?? .distantPast
        let end = AIInteractionTimeline.Item.date(detail.event?.end).map { $0.addingTimeInterval(0.001) } ?? entries.compactMap(\.event).filter { $0.kind == "ai.prompt" && ($0.date ?? .distantPast) > start }.compactMap(\.date).min() ?? .distantFuture
        do {
            var messages: [AgentMessage] = []
            var cursor: String?
            // The core reads forward from this instruction; stop once the next instruction's time is reached.
            repeat {
                let result = try await reader(agent, ref, cursor)
                guard ticket == selectionGeneration, selected == id else { return }
                messages += result.messages
                cursor = result.nextCursor
                if let last = messages.last, Date(timeIntervalSince1970: Double(last.tsMs) / 1000) >= end { break }
            } while cursor != nil && messages.count < 2000 && !Task.isCancelled
            guard !Task.isCancelled, ticket == selectionGeneration, selected == id else { return }
            // A late source result must not repopulate a revoked or closed view.
            await open(id, preserveReply: true)
            guard ticket == selectionGeneration, selected == id, self.detail != nil else { return }
            reply = messages.filter { $0.role == "assistant" && !$0.reasoning && Date(timeIntervalSince1970: Double($0.tsMs) / 1000) >= start && Date(timeIntervalSince1970: Double($0.tsMs) / 1000) < end }.compactMap(\.text).joinedReply
            if reply?.isEmpty == true { reply = L("这条指令之后没有文字回复") }
        } catch { if ticket == selectionGeneration, selected == id { reply = nil; sourceNote = L("回复暂时读不到：会话文件不可读或已变化。可以从「打开来源」回到原会话。") } }
    }
}

extension AIInteractionTimeline {
    /// A synthetic set of instructions, work stretches and one note placed in
    /// the last hours before `now`, labeled 合成样例. Used only by `--mock`.
    static func demo(now: Date) -> AIInteractionTimeline {
        struct Mark { let id: String; let kind: String; let minutesAgo: Double; let length: Double?; let text: String; let agent: String }
        let marks: [Mark] = [
            Mark(id: "demo-p1", kind: "ai.prompt", minutesAgo: 300, length: 7, text: L("解释一下 WAL 模式下读写冲突"), agent: "claude_code"),
            Mark(id: "demo-p2", kind: "ai.prompt", minutesAgo: 262, length: 14, text: L("给时间轴加双指缩放，缩放中心跟随指针"), agent: "claude_code"),
            Mark(id: "demo-p3", kind: "ai.prompt", minutesAgo: 246, length: 5, text: L("跑 swift test，把失败的用例列出来"), agent: "claude_code"),
            Mark(id: "demo-n1", kind: "note", minutesAgo: 190, length: nil, text: L("截图密的时候合并显示"), agent: "codex"),
            Mark(id: "demo-p4", kind: "ai.prompt", minutesAgo: 75, length: 12, text: L("把 AI 行改成指令和工作两排"), agent: "codex"),
            Mark(id: "demo-p5", kind: "ai.prompt", minutesAgo: 60, length: 14, text: L("修复连接页卡片顶部对不齐"), agent: "codex"),
            Mark(id: "demo-p6", kind: "ai.prompt", minutesAgo: 44, length: 2, text: L("顺便把资料页空状态缩小"), agent: "codex"),
            Mark(id: "demo-p7", kind: "ai.prompt", minutesAgo: 18, length: nil, text: L("生成多轨时间线的静态样张，沿用 DesignSystem"), agent: "codex"),
        ]
        func iso(_ date: Date) -> String { date.ISO8601Format() }
        var entries: [[String: Any]] = []
        for mark in marks {
            let start = now.addingTimeInterval(-mark.minutesAgo * 60)
            entries.append(["event": ["event_id": mark.id, "kind": mark.kind, "start": iso(start), "thread_id": "demo-" + mark.agent], "preview": mark.text + L("（合成样例）")])
            if mark.kind == "ai.prompt" {
                var work: [String: Any] = ["event_id": mark.id + ":work", "kind": "ai.work", "start": iso(start)]
                if let length = mark.length { work["end"] = iso(start.addingTimeInterval(length * 60)) }
                entries.append(["event": work, "preview": L("工作")])
            }
        }
        let page = (try? JSONSerialization.data(withJSONObject: ["entries": entries])) ?? Data()
        let details = Dictionary(uniqueKeysWithValues: marks.map { mark -> (String, Data) in
            let event: [String: Any] = ["event_id": mark.id, "kind": mark.kind, "start": iso(now.addingTimeInterval(-mark.minutesAgo * 60))]
            let payload: [String: Any] = ["agent": mark.agent, "project": "/SYNTHETIC/candid", "status": mark.length == nil ? "running" : "done",
                                          "tool_calls": ["shell": 3 + mark.text.count % 5, "edit": 1 + mark.text.count % 3]]
            let body: [String: Any] = ["event": event, "payload": payload, "paragraphs": [["text": mark.text + L("（合成样例）")]],
                                       "thread": ["thread_id": "demo-" + mark.agent, "kind": "agent_session", "title": L("多轨时间线（合成）")]]
            return (mark.id, (try? JSONSerialization.data(withJSONObject: body)) ?? Data())
        })
        let model = AIInteractionTimeline(query: { name, arguments in
            if name == "get_event", let object = try? JSONSerialization.jsonObject(with: arguments) as? [String: Any],
               let id = object["event_id"] as? String, let data = details[id] { return data }
            return page
        }, reader: { _, _, _ in throw CoreError.unreachable(L("合成样例没有会话文件")) })
        model.isDemo = true
        return model
    }
}

private extension Array where Element == String {
    /// Assistant messages joined into one compact text (tool-only messages and blank runs dropped).
    var joinedReply: String { AIReplyText.normalize(self) }
}
