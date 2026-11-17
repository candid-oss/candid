import Foundation
import Testing
@testable import Candid

@MainActor @Suite struct TimelineSceneSchedulerTests {
    @MainActor private final class Inputs { var value = 0; var shown = -1 }
    private func waitFor(_ test: @MainActor () -> Bool) async throws {
        for _ in 0..<200 {
            if test() { return }
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(test())
    }

    @Test func hiddenChangesDoNoWorkAndShowingUsesLatestInputs() async throws {
        let scheduler = TimelineSceneScheduler(delay: .milliseconds(1))
        defer { scheduler.stop() }
        let inputs = Inputs()
        scheduler.setVisible(false)
        for i in 0..<20 { inputs.value = i; scheduler.request { inputs.shown = inputs.value } }
        try await Task.sleep(for: .milliseconds(20))
        #expect(inputs.shown == -1)
        scheduler.setVisible(true)
        scheduler.request { inputs.shown = inputs.value }
        try await waitFor { inputs.shown == 19 }
    }

    @Test func burstDuringBuildProducesOneFollowup() async throws {
        let scheduler = TimelineSceneScheduler(delay: .milliseconds(1))
        defer { scheduler.stop() }
        var entered = 0
        var continuation: CheckedContinuation<Void, Never>?
        let build: @MainActor () async -> Void = {
            entered += 1
            if entered == 1 { await withCheckedContinuation { continuation = $0 } }
        }
        scheduler.request(build)
        try await waitFor { entered == 1 }
        for _ in 0..<10 { scheduler.request(build) }
        continuation?.resume()
        try await waitFor { entered == 2 }
        try await Task.sleep(for: .milliseconds(20))
        #expect(entered == 2)
    }

    @Test func hidingCancelsOldResultsWithoutCancellingTheNewVisibleBuild() async throws {
        let scheduler = TimelineSceneScheduler(delay: .milliseconds(1))
        defer { scheduler.stop() }
        var continuation: CheckedContinuation<Void, Never>?
        var oldEntered = false, oldApplied = false, freshApplied = false
        scheduler.request {
            oldEntered = true
            await withCheckedContinuation { continuation = $0 }
            if !Task.isCancelled { oldApplied = true }
        }
        try await waitFor { oldEntered }
        scheduler.setVisible(false)
        scheduler.setVisible(true)
        scheduler.request { freshApplied = true }
        continuation?.resume()
        try await waitFor { freshApplied }
        #expect(!oldApplied)
    }
}
