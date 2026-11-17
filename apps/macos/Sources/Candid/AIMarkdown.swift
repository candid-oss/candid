import Foundation

// Markdown in Pi's answers: headings, paragraphs, bullet and numbered lists, quotes, code blocks, rules and tables as
// blocks; bold, italic, inline code and links inline (Foundation's inline parser). Record citations become links with
// the `candid-record` scheme, which the page draws as chips that open the record.

enum AIMarkdownBlock: Equatable {
    case heading(level: Int, text: String)
    case paragraph(String)
    /// `ordered` items keep their number; `depth` 0 is the outer level.
    case listItem(marker: String, depth: Int, text: String)
    case quote(String)
    case code(language: String, text: String)
    case table([[String]])
    case rule
}

enum AIMarkdown {
    static let recordScheme = "candid-record"

    /// Splits text into blocks (a small CommonMark subset that models actually produce).
    static func blocks(_ text: String) -> [AIMarkdownBlock] {
        var blocks: [AIMarkdownBlock] = []
        var paragraph: [String] = []
        var quote: [String] = []
        var table: [[String]] = []
        var code: (language: String, lines: [String])?
        func flush() {
            if !paragraph.isEmpty { blocks.append(.paragraph(paragraph.joined(separator: "\n"))); paragraph = [] }
            if !quote.isEmpty { blocks.append(.quote(quote.joined(separator: "\n"))); quote = [] }
            if !table.isEmpty { blocks.append(.table(table)); table = [] }
        }
        for raw in text.replacingOccurrences(of: "\r\n", with: "\n").components(separatedBy: "\n") {
            let line = raw.trimmingCharacters(in: .whitespaces)
            if var open = code {
                if line.hasPrefix("```") { blocks.append(.code(language: open.language, text: open.lines.joined(separator: "\n"))); code = nil }
                else { open.lines.append(raw); code = open }
                continue
            }
            if line.hasPrefix("```") { flush(); code = (String(line.dropFirst(3)).trimmingCharacters(in: .whitespaces), []); continue }
            if line.isEmpty { flush(); continue }
            if line.hasPrefix("#"), let space = line.firstIndex(of: " "), line[..<space].allSatisfy({ $0 == "#" }), line.distance(from: line.startIndex, to: space) <= 6 {
                flush(); blocks.append(.heading(level: line.distance(from: line.startIndex, to: space), text: String(line[line.index(after: space)...]))); continue
            }
            if line.count >= 3, ["-", "*", "_"].contains(where: { c in line.allSatisfy { String($0) == c || $0 == " " } && line.filter({ String($0) == c }).count >= 3 }) {
                flush(); blocks.append(.rule); continue
            }
            if line.hasPrefix("|") {
                if !paragraph.isEmpty || !quote.isEmpty { let t = table; flush(); table = t }
                let cells = line.trimmingCharacters(in: CharacterSet(charactersIn: "|")).components(separatedBy: "|").map { $0.trimmingCharacters(in: .whitespaces) }
                if !cells.allSatisfy({ !$0.isEmpty && $0.allSatisfy { "-:".contains($0) } }) { table.append(cells) }
                continue
            }
            if line.hasPrefix(">") {
                if !paragraph.isEmpty || !table.isEmpty { let q = quote; flush(); quote = q }
                quote.append(String(line.dropFirst()).trimmingCharacters(in: .whitespaces)); continue
            }
            if let item = listItem(raw) { flush(); blocks.append(item); continue }
            if !quote.isEmpty || !table.isEmpty { flush() }
            // A line indented under a list item continues it.
            if paragraph.isEmpty, raw.hasPrefix("  "), case .listItem(let marker, let depth, let text)? = blocks.last {
                blocks[blocks.count - 1] = .listItem(marker: marker, depth: depth, text: text + "\n" + line); continue
            }
            paragraph.append(line)
        }
        if let open = code { blocks.append(.code(language: open.language, text: open.lines.joined(separator: "\n"))) }
        flush()
        return blocks
    }

    private static func listItem(_ raw: String) -> AIMarkdownBlock? {
        let indent = raw.prefix { $0 == " " || $0 == "\t" }.reduce(0) { $0 + ($1 == "\t" ? 4 : 1) }
        let line = raw.trimmingCharacters(in: .whitespaces)
        for bullet in ["- ", "* ", "+ ", "• "] where line.hasPrefix(bullet) {
            return .listItem(marker: "•", depth: indent / 2, text: String(line.dropFirst(bullet.count)))
        }
        let digits = line.prefix { $0.isNumber }
        if !digits.isEmpty, digits.count <= 3 {
            let rest = line.dropFirst(digits.count)
            if rest.hasPrefix(". ") || rest.hasPrefix(") ") { return .listItem(marker: digits + ".", depth: indent / 2, text: String(rest.dropFirst(2))) }
        }
        return nil
    }

    // MARK: Citations

    private static let uuid = try! NSRegularExpression(pattern: "\\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\\b")
    private static let bracketed = try! NSRegularExpression(pattern: "\\[\\[\\s*(?:ev(?:ent)?[:：]\\s*)?([^\\]\\s]+)\\s*\\]\\]")
    private static let linked = try! NSRegularExpression(pattern: "\\[([^\\]]*)\\]\\((?:record|event)[:：]([^)\\s]+)\\)")
    private static let labelled = try! NSRegularExpression(pattern: "(?:event_id|事件\\s*ID|id)\\s*[:：=]\\s*`?(" + "[0-9a-fA-F]{8}-[0-9a-fA-F-]{27}" + ")`?", options: [.caseInsensitive])

    /// Rewrites record citations (`[[id]]`, `[x](record:id)`, `event_id: id` and bare ids) into chip links, outside code.
    static func citations(_ text: String, label: (String) -> String) -> String {
        var out = ""
        var inFence = false
        for (i, line) in text.components(separatedBy: "\n").enumerated() {
            if i > 0 { out += "\n" }
            if line.trimmingCharacters(in: .whitespaces).hasPrefix("```") { inFence.toggle(); out += line; continue }
            if inFence { out += line; continue }
            // 「event_id: `id`」 first, then everything outside inline code; a code span holding nothing but an id is a citation too.
            let ns = line as NSString
            var labelledLine = "", last = 0
            for m in labelled.matches(in: line, range: NSRange(location: 0, length: ns.length)) {
                labelledLine += ns.substring(with: NSRange(location: last, length: m.range.location - last)) + "[[" + ns.substring(with: m.range(at: 1)) + "]]"
                last = m.range.location + m.range.length
            }
            labelledLine += ns.substring(from: last)
            let parts = labelledLine.components(separatedBy: "`")
            out += parts.enumerated().map { j, part in
                guard j % 2 == 1, j < parts.count - 1 else { return j % 2 == 1 ? "`" + part : rewrite(part, label: label) }
                let whole = part.trimmingCharacters(in: .whitespaces)
                return uuid.firstMatch(in: whole, range: NSRange(location: 0, length: (whole as NSString).length))?.range.length == (whole as NSString).length
                    ? rewrite(whole, label: label) : "`" + part + "`"
            }.joined()
        }
        return out
    }

    private static func rewrite(_ text: String, label: (String) -> String) -> String {
        func link(_ id: String) -> String { "[\(label(id))](\(recordScheme)://\(id))" }
        var s = text
        for (regex, idGroup) in [(linked, 2), (bracketed, 1)] {
            let ns = s as NSString
            var result = "", last = 0
            for m in regex.matches(in: s, range: NSRange(location: 0, length: ns.length)) {
                result += ns.substring(with: NSRange(location: last, length: m.range.location - last))
                result += link(ns.substring(with: m.range(at: idGroup)))
                last = m.range.location + m.range.length
            }
            s = result + ns.substring(from: last)
        }
        // Bare ids that are not already inside a chip link.
        let ns = s as NSString
        var result = "", last = 0
        for m in uuid.matches(in: s, range: NSRange(location: 0, length: ns.length)) {
            let before = ns.substring(to: m.range.location)
            if before.hasSuffix(recordScheme + "://") { continue }
            result += ns.substring(with: NSRange(location: last, length: m.range.location - last)) + link(ns.substring(with: m.range))
            last = m.range.location + m.range.length
        }
        return result + ns.substring(from: last)
    }

    /// Inline Markdown (bold, italic, code, links) as an attributed string; falls back to the plain text.
    static func inline(_ text: String) -> AttributedString {
        (try? AttributedString(markdown: text, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace, failurePolicy: .returnPartiallyParsedIfPossible)))
            ?? AttributedString(text)
    }
}
