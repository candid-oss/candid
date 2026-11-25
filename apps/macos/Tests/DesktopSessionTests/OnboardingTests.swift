import Foundation
import Testing
@testable import Candid

/// The guide's one-click Codex hookup: an earlier Candid table is replaced, everything else is kept.
@Suite struct OnboardingTests {
    @Test func codexMergeReplacesOnlyCandidTable() {
        let existing = "model = \"o3\"\n\n[mcp_servers.candid]\ncommand = \"/old\"\n\n[mcp_servers.candid.env]\nX = \"1\"\n\n[mcp_servers.other]\ncommand = \"o\"\n"
        let merged = OnboardingModel.mergeCodexConfig(existing, "[mcp_servers.candid]\ncommand = \"/new\"")
        #expect(merged.contains("model = \"o3\""))
        #expect(merged.contains("[mcp_servers.other]\ncommand = \"o\""))
        #expect(!merged.contains("/old") && !merged.contains("X = \"1\""))
        #expect(merged.hasSuffix("[mcp_servers.candid]\ncommand = \"/new\"\n"))
        #expect(OnboardingModel.mergeCodexConfig("", "[a]") == "[a]\n")
    }

    @Test func presetsAskOnlyForWhatTheyNeed() {
        #expect(CapturePreset.light.permissions == [.accessibility])
        #expect(!CapturePreset.standard.permissions.contains(.microphone))
        #expect(CapturePreset.full.permissions.contains(.calendar))
    }
}
