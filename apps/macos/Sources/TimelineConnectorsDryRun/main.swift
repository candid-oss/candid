import Foundation
import TimelineConnectors
import CandidClient

// Read-only coverage check for ONE agent's local transcripts. Prints counts, time ranges and issue codes only; it never
// prints, stores or forwards transcript content. Because running it reads that agent's files, it needs an explicit id.
// Usage: TimelineConnectorsDryRun --agent codex|claude_code|pi --socket <isolated core socket>

var agent: String?
var socket: String?
var args = CommandLine.arguments.dropFirst()
while let a = args.popFirst() {
    switch a {
    case "--agent": agent = args.popFirst()
    case "--socket": socket = args.popFirst()
    default: FileHandle.standardError.write(Data("unknown argument \(a)\n".utf8)); exit(2)
    }
}
guard let agent else {
    FileHandle.standardError.write(Data("missing --agent <codex|claude_code|pi>; nothing was read\n".utf8)); exit(2)
}

// Uses only the running core's existing explicit consent; never grants read or activity.
guard let socket else {
    FileHandle.standardError.write(Data("missing --socket <isolated core socket>\n".utf8)); exit(2)
}
let client = AgentCoreClient(channel: LocalChannel(socketPath: socket))
guard let connector = AgentConnectors.connector(agent: agent, client: client) else { exit(2) }

func day(_ ms: Int64?) -> String {
    guard let ms else { return "-" }
    let f = DateFormatter(); f.dateFormat = "yyyy-MM-dd"; f.timeZone = TimeZone(identifier: "UTC")
    return f.string(from: Date(timeIntervalSince1970: Double(ms) / 1000))
}

let det = await connector.detect()
print("== \(agent): \(det.state), format \(det.formatVersion ?? "-")")
guard det.state != .notFound else { exit(0) }
let all = DateInterval(start: .distantPast, end: .distantFuture)
do {
    let list = try await connector.sessions(range: all, project: nil)
    let turns = list.sessions.reduce(0) { $0 + $1.turns }
    let starts = list.sessions.compactMap(\.startMs), ends = list.sessions.compactMap(\.endMs)
    print("sessions \(list.sessions.count), turns \(turns), with title \(list.sessions.filter { $0.title != nil }.count), with project \(list.sessions.filter { $0.project != nil }.count)")
    print("range \(day(starts.min())) … \(day(ends.max())) UTC")
    let u = try await connector.usage(range: all)
    print("usage input \(u.totals.input) output \(u.totals.output) cache_read \(u.totals.cacheRead) cache_write \(u.totals.cacheWrite), sessions with usage \(u.sessions)")
    var codes: [String: Int] = [:]
    for i in list.issues { codes["\(i.code)@line\(i.line ?? 0)", default: 0] += 1 }
    print("parse issues \(codes.isEmpty ? "none" : codes.sorted { $0.key < $1.key }.map { "\($0.key)×\($0.value)" }.joined(separator: ", "))")
} catch { print("failed: \(error)") }
