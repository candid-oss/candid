import Foundation
import Testing
import TimelineConnectors
@testable import Candid

/// Power v1 §3.5: the App watches folders and follows the change feed instead of polling.
@Suite struct IdleWakeTests {
    @Test func agentFolderWatcherWatchesOnlyConsentedFoldersWithDefaults() {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        var codex = AgentConsent(agent: "codex"); codex.read = true; codex.activity = true
        var claude = AgentConsent(agent: "claude_code"); claude.read = true; claude.activity = true
        claude.paths = ["~/SYNTHETIC/claude", "/tmp/SYNTHETIC-claude"]
        var pi = AgentConsent(agent: "pi"); pi.read = true // activity off: nothing is synced, nothing watched
        let roots = AgentFolderWatcher.roots([codex, claude, pi])
        #expect(roots == ["/tmp/SYNTHETIC-claude", home + "/.codex/sessions", home + "/SYNTHETIC/claude"].sorted())
        #expect(AgentFolderWatcher.roots([]).isEmpty)
    }
}
