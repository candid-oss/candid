import SwiftUI

/// AI text read from a session file (an instruction or the reply after it), made compact and readable: the parts
/// are trimmed, empty parts (tool-call-only messages) dropped, runs of blank lines collapsed, and the result split into
/// Markdown blocks — paragraphs, headings, lists, quotes and fenced code. Pure, so it is testable.
enum AIReplyText {
    enum Block: Equatable {
        case paragraph(String)
        case heading(level: Int, text: String)
        /// List lines with their markers normalised to "•" or "1."; nesting kept as leading spaces.
        case list([String])
        case quote(String)
        case code(String)
    }

    /// Joins message texts with one blank line between non-empty ones; no line ends in spaces and there are never
    /// two blank lines in a row (fenced code is kept as written).
    static func normalize(_ parts: [String]) -> String {
        parts.map(clean).filter { !$0.isEmpty }.joined(separator: "\n\n")
    }

    static func normalize(_ text: String) -> String { normalize([text]) }

    private static func clean(_ text: String) -> String {
        var out: [String] = []
        var inCode = false
        for raw in text.replacingOccurrences(of: "\r\n", with: "\n").replacingOccurrences(of: "\r", with: "\n")
            .replacingOccurrences(of: "\u{00A0}", with: " ").split(separator: "\n", omittingEmptySubsequences: false) {
            var line = String(raw)
            if line.trimmingCharacters(in: .whitespaces).hasPrefix("```") { inCode.toggle() }
            if !inCode { while line.last?.isWhitespace == true { line.removeLast() } }
            if !inCode, line.isEmpty, out.last?.isEmpty ?? true { continue }
            out.append(line)
        }
        while out.last?.trimmingCharacters(in: .whitespaces).isEmpty == true { out.removeLast() }
        return out.joined(separator: "\n")
    }

    static func blocks(_ text: String) -> [Block] {
        var blocks: [Block] = []
        var lines: [String] = []
        var code: [String]?
        func flush() {
            defer { lines.removeAll() }
            guard !lines.isEmpty else { return }
            if lines.allSatisfy({ listMarker($0) != nil || $0.hasPrefix("  ") }), listMarker(lines[0]) != nil {
                blocks.append(.list(lines.map(listLine)))
            } else if lines.allSatisfy({ $0.hasPrefix(">") }) {
                blocks.append(.quote(lines.map { String($0.dropFirst()).trimmingCharacters(in: .whitespaces) }.joined(separator: "\n")))
            } else {
                blocks.append(.paragraph(lines.joined(separator: "\n")))
            }
        }
        for line in normalize(text).split(separator: "\n", omittingEmptySubsequences: false).map(String.init) {
            if line.trimmingCharacters(in: .whitespaces).hasPrefix("```") {
                if let open = code { blocks.append(.code(open.joined(separator: "\n"))); code = nil } else { flush(); code = [] }
                continue
            }
            if code != nil { code?.append(line); continue }
            if line.isEmpty { flush(); continue }
            if let heading = heading(line) { flush(); blocks.append(heading); continue }
            lines.append(line)
        }
        if let open = code { blocks.append(.code(open.joined(separator: "\n"))) }
        flush()
        return blocks
    }

    private static func heading(_ line: String) -> Block? {
        let hashes = line.prefix { $0 == "#" }.count
        guard (1...6).contains(hashes), line.dropFirst(hashes).first == " " else { return nil }
        return .heading(level: hashes, text: line.dropFirst(hashes + 1).trimmingCharacters(in: .whitespaces))
    }

    private static func listMarker(_ line: String) -> String? {
        let body = line.drop { $0 == " " }
        for bullet in ["- ", "* ", "+ "] where body.hasPrefix(bullet) { return "•" }
        let digits = body.prefix { $0.isNumber }
        if !digits.isEmpty, digits.count <= 3, body.dropFirst(digits.count).hasPrefix(". ") { return String(digits) + "." }
        return nil
    }

    private static func listLine(_ line: String) -> String {
        let indent = String(repeating: " ", count: line.prefix { $0 == " " }.count / 2 * 3)
        guard let marker = listMarker(line) else { return line.trimmingCharacters(in: .whitespaces) }
        let body = line.drop { $0 == " " }
        let rest = marker == "•" ? body.dropFirst(2) : body.dropFirst(marker.count + 1)
        return indent + marker + " " + rest.trimmingCharacters(in: .whitespaces)
    }

    /// Inline Markdown (bold, italic, code, links); falls back to the plain text when it does not parse.
    static func inline(_ text: String) -> AttributedString {
        (try? AttributedString(markdown: text, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(text)
    }
}

/// Renders AI text as compact Markdown blocks with even spacing; selectable.
struct AIReplyView: View {
    let text: String
    var body: some View {
        let blocks = AIReplyText.blocks(text)
        VStack(alignment: .leading, spacing: Space.xs) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                switch block {
                case .paragraph(let value):
                    Text(AIReplyText.inline(value)).typography(.body)
                case .heading(let level, let value):
                    Text(AIReplyText.inline(value)).typography(level <= 2 ? .headline : .body, .semibold).padding(.top, Space.xxs)
                case .list(let items):
                    VStack(alignment: .leading, spacing: 2) {
                        ForEach(Array(items.enumerated()), id: \.offset) { _, item in Text(AIReplyText.inline(item)).typography(.body) }
                    }
                case .quote(let value):
                    Text(AIReplyText.inline(value)).typography(.body).foregroundStyle(.secondary)
                        .padding(.leading, Space.s)
                        .overlay(alignment: .leading) { Capsule().fill(Surface.selected.color).frame(width: 2) }
                case .code(let value):
                    Text(value).typography(.secondary).monospaced()
                        .padding(Space.s).frame(maxWidth: .infinity, alignment: .leading)
                        .background(RoundedRectangle(cornerRadius: Radius.small, style: .continuous).fill(Surface.fill.color))
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .textSelection(.enabled)
        .fixedSize(horizontal: false, vertical: true)
    }
}
