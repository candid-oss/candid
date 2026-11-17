import SwiftUI

struct DesktopMarkdownResultView: View {
    let markdown: String

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                switch block {
                case .heading(let title): inline(title).typography(.headline).padding(.top, 4)
                case .text(let text): inline(text)
                case .quote(let text):
                    inline(text).foregroundStyle(.secondary).padding(.leading, 10)
                        .overlay(alignment: .leading) { Rectangle().fill(.quaternary).frame(width: 2) }
                case .code(let code):
                    ScrollView(.horizontal) {
                        Text(verbatim: code).typography(.secondary).monospaced().padding(8)
                    }.background(.background.secondary, in: RoundedRectangle(cornerRadius: Radius.small))
                case .table(let rows):
                    ScrollView(.horizontal) {
                        Grid(alignment: .leading, horizontalSpacing: 14, verticalSpacing: 6) {
                            ForEach(Array(rows.enumerated()), id: \.offset) { index, row in
                                if index > 0 { HairlineDivider() }
                                GridRow {
                                    ForEach(Array(row.enumerated()), id: \.offset) { _, cell in
                                        inline(cell).fontWeight(index == 0 ? .semibold : .regular)
                                            .fixedSize(horizontal: true, vertical: false)
                                    }
                                }
                            }
                        }.padding(8)
                    }.background(.background.secondary, in: RoundedRectangle(cornerRadius: Radius.small))
                case .rule: HairlineDivider()
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .textSelection(.enabled)
        .environment(\.openURL, OpenURLAction { _ in .discarded })
    }

    private func inline(_ text: String) -> Text {
        let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        guard let value = try? AttributedString(markdown: text, options: options) else { return Text(verbatim: text) }
        return Text(value)
    }

    private enum Block {
        case heading(String), text(String), quote(String), code(String), table([[String]]), rule
    }

    private var blocks: [Block] {
        let lines = markdown.components(separatedBy: .newlines)
        var result: [Block] = []
        var index = 0
        while index < lines.count {
            let line = lines[index].trimmingCharacters(in: .whitespaces)
            if line.hasPrefix("```") {
                index += 1
                var code: [String] = []
                while index < lines.count && !lines[index].trimmingCharacters(in: .whitespaces).hasPrefix("```") {
                    code.append(lines[index]); index += 1
                }
                result.append(.code(code.joined(separator: "\n")))
            } else if line.hasPrefix("|"), index + 1 < lines.count,
                      lines[index + 1].contains("|"), lines[index + 1].contains("---") {
                var rows = [cells(line)]
                index += 2
                while index < lines.count && lines[index].trimmingCharacters(in: .whitespaces).hasPrefix("|") {
                    rows.append(cells(lines[index])); index += 1
                }
                result.append(.table(rows))
                continue
            } else if line == "---" || line == "***" {
                result.append(.rule)
            } else if line.first == "#", let space = line.firstIndex(of: " "),
                      line[..<space].allSatisfy({ $0 == "#" }) {
                result.append(.heading(String(line[line.index(after: space)...])))
            } else if line.hasPrefix("> ") {
                result.append(.quote(String(line.dropFirst(2))))
            } else if !line.isEmpty {
                result.append(.text(line.hasPrefix("- ") ? "• " + line.dropFirst(2) : line))
            }
            index += 1
        }
        return result
    }

    private func cells(_ line: String) -> [String] {
        line.trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "|"))
            .replacingOccurrences(of: "\\|", with: "\u{1F}")
            .split(separator: "|", omittingEmptySubsequences: false)
            .map { $0.trimmingCharacters(in: .whitespaces).replacingOccurrences(of: "\u{1F}", with: "|") }
    }
}
