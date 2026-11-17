import Foundation

// Skills the attached Pi can run (agent-platform-v1 §4.1 hooks and skills). The text lives in agents/skills
// (bundled as Contents/Resources/Pi/skills), shared with external agents; the AI page lists them and runs one by
// sending its steps to Pi as a turn of the chat. Timed and event hooks are listed but stay off until the context gate
// is met (§4.1); every skill can be run by hand.

struct AISkill: Identifiable, Equatable, Sendable {
    enum Trigger: Equatable, Sendable {
        case manual
        case timed(String)
        case event(String)
    }
    /// The folder name (`daily-review`).
    var id: String
    var title: String
    var summary: String
    var trigger: Trigger
    /// The hook (timed or event) is on. Manual skills are always available.
    var enabled: Bool
    /// What the skill needs from the user (a recording, a topic); nil when it can run as is.
    var input: String?
    /// The instructions after the front matter.
    var body: String

    var triggerLabel: String {
        switch trigger {
        case .manual: "手动"
        case .timed(let at): "定时 · 每天 \(at)"
        case .event(let what): "事件 · " + what
        }
    }

    /// What Pi receives when the skill runs: its steps, the date, and what the user brought in.
    func prompt(today: Date, context: [AIContextItem], extra: String = "") -> String {
        let f = DateFormatter(); f.locale = Locale(identifier: "zh_CN"); f.dateFormat = "yyyy-MM-dd EEEE"
        var text = "运行技能「\(title)」。今天是 \(f.string(from: today))（本地时区）。按下面的步骤做：\n\n" + body
        if !context.isEmpty { text += "\n\n用户带进来的上下文：\n" + context.map(\.promptLine).joined(separator: "\n") }
        if !extra.isEmpty { text += "\n\n用户补充：" + extra }
        return text
    }

    /// Parses a SKILL.md: `key: value` front matter between `---` lines, then the body.
    static func parse(id: String, text: String) -> AISkill? {
        let lines = text.replacingOccurrences(of: "\r\n", with: "\n").components(separatedBy: "\n")
        guard lines.first == "---", let close = lines.dropFirst().firstIndex(of: "---") else { return nil }
        var meta: [String: String] = [:]
        for line in lines[1..<close] {
            guard let colon = line.firstIndex(of: ":") else { continue }
            meta[line[..<colon].trimmingCharacters(in: .whitespaces)] = line[line.index(after: colon)...]
                .trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "\""))
        }
        let body = lines[(close + 1)...].joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
        let heading = body.components(separatedBy: "\n").first { $0.hasPrefix("# ") }.map { String($0.dropFirst(2)) }
        guard let title = meta["title"] ?? heading else { return nil }
        let trigger: Trigger
        switch meta["trigger"] {
        case "timed": trigger = .timed(meta["schedule"] ?? "21:00")
        case "event": trigger = .event(meta["event"] ?? "事件发生时")
        default: trigger = .manual
        }
        return AISkill(id: id, title: title, summary: meta["summary"] ?? meta["description"] ?? "", trigger: trigger,
                       enabled: trigger == .manual || meta["enabled"] == "true", input: meta["input"], body: body)
    }

    /// Every skill under the first folder that has any (the bundled copy, or the repository's in development).
    static func load(from folders: [URL]) -> [AISkill] {
        let fm = FileManager.default
        for folder in folders {
            let names = ((try? fm.contentsOfDirectory(atPath: folder.path)) ?? []).sorted()
            let skills = names.compactMap { name -> AISkill? in
                guard let text = try? String(contentsOf: folder.appendingPathComponent(name).appendingPathComponent("SKILL.md"), encoding: .utf8) else { return nil }
                return parse(id: name, text: text)
            }
            if !skills.isEmpty { return skills.sorted { order($0.id) < order($1.id) } }
        }
        return []
    }

    /// 日回顾 and 资料与回复检索 first (the two the page names), then the rest by name.
    private static func order(_ id: String) -> String {
        ["daily-review": "0", "references-replies": "1"][id] ?? "2" + id
    }

    static func folders(for install: PiInstall) -> [URL] {
        // The installed copy, a development checkout (`agents/skills` next to `pi-extension`), then the App's own skeleton,
        // so skills are listed even before the runtime is installed.
        [install.home.appendingPathComponent("skills"), install.home.deletingLastPathComponent().appendingPathComponent("skills"),
         PiComponent.skeleton.appendingPathComponent("skills")]
    }
}
