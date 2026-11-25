import CandidClient
import CoreServices
import Foundation
import TimelineConnectors

/// Watches the agent transcript folders the user allowed (Codex, Claude Code, pi) with FSEvents and asks the core to
/// read just the transcript files that changed (`POST /local/sync {"paths": [...]}`). The core no longer lists these
/// folders on a timer; its full pass is a 10-minute fallback. Events are coalesced by FSEvents for a few seconds,
/// so an agent writing every message costs one small request per burst. Nothing is watched without consent.
@MainActor final class AgentFolderWatcher {
    static let shared = AgentFolderWatcher()

    private var stream: FSEventStreamRef?
    private var roots: [String] = []
    private var channel: LocalChannel?
    private var consentObserver: NSObjectProtocol?
    private let queue = DispatchQueue(label: "io.github.candid-oss.candid.agent-folders", qos: .utility)

    /// Seconds FSEvents waits to collect events into one callback.
    static let latency: CFTimeInterval = 5

    /// Called when the core (re)connects: reads the consents and watches their folders.
    func start(channel: LocalChannel) {
        self.channel = channel
        if consentObserver == nil {
            consentObserver = NotificationCenter.default.addObserver(forName: .candidAgentConsentChanged, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { Task { await self?.reload() } }
            }
        }
        Task { await reload() }
    }

    func stop() {
        if let stream {
            FSEventStreamStop(stream)
            FSEventStreamInvalidate(stream)
            FSEventStreamRelease(stream)
        }
        stream = nil
        roots = []
    }

    private func reload() async {
        guard let channel, let settings = try? await AgentCoreClient(channel: channel).settings() else { return }
        let wanted = Self.roots(settings.consents)
        guard wanted != roots else { return }
        stop()
        roots = wanted
        guard !wanted.isEmpty else { return }
        watch(wanted)
    }

    /// The folders read for each agent with reading and activity on; empty paths mean the format's default.
    nonisolated static func roots(_ consents: [AgentConsent]) -> [String] {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        let defaults = ["codex": "~/.codex/sessions", "claude_code": "~/.claude/projects", "pi": "~/.pi/agent/sessions"]
        let paths = consents.filter { $0.read && $0.activity }.flatMap { c in c.paths.isEmpty ? [defaults[c.agent] ?? ""] : c.paths }
        let expanded = paths.filter { !$0.isEmpty }.map { $0.hasPrefix("~/") ? home + "/" + $0.dropFirst(2) : $0 }
        return Array(Set(expanded)).sorted()
    }

    private func watch(_ roots: [String]) {
        let box = Unmanaged.passRetained(Callback { [weak self] paths in
            Task { @MainActor in self?.changed(paths) }
        })
        var context = FSEventStreamContext(version: 0, info: box.toOpaque(), retain: nil, release: { info in
            if let info { Unmanaged<Callback>.fromOpaque(info).release() }
        }, copyDescription: nil)
        let flags = FSEventStreamCreateFlags(kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagUseCFTypes | kFSEventStreamCreateFlagIgnoreSelf)
        guard let stream = FSEventStreamCreate(nil, Self.received, &context, roots as CFArray,
                                               FSEventStreamEventId(kFSEventStreamEventIdSinceNow), Self.latency, flags) else {
            box.release()
            return
        }
        FSEventStreamSetDispatchQueue(stream, queue)
        FSEventStreamStart(stream)
        self.stream = stream
    }

    private func changed(_ paths: [String]) {
        let transcripts = paths.filter { $0.hasSuffix(".jsonl") }
        guard !transcripts.isEmpty, let channel else { return }
        Task { try? await channel.syncFiles(transcripts) }
    }

    private final class Callback: @unchecked Sendable {
        let deliver: @Sendable ([String]) -> Void
        init(_ deliver: @escaping @Sendable ([String]) -> Void) { self.deliver = deliver }
    }

    private static let received: FSEventStreamCallback = { _, info, count, paths, _, _ in
        guard let info, count > 0 else { return }
        let list = (unsafeBitCast(paths, to: NSArray.self) as? [String]) ?? []
        Unmanaged<Callback>.fromOpaque(info).takeUnretainedValue().deliver(list)
    }
}
