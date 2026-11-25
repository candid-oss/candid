import SwiftUI

extension View {
    /// Runs `action` when the view appears and again after the change-feed version moves, folding a burst of changes
    /// into one call. This replaces timer-driven refreshes: while nothing changes, nothing runs. Pass
    /// `TimelineService.shared.dataVersion` (new or removed records) or `.jobsVersion` (that plus job progress).
    func refresh(on version: Int, debounce: Duration = .milliseconds(300), _ action: @escaping @MainActor () async -> Void) -> some View {
        modifier(FeedRefresh(version: version, debounce: debounce, action: action))
    }
}

private struct FeedRefresh: ViewModifier {
    let version: Int
    let debounce: Duration
    let action: @MainActor () async -> Void
    @ViewState private var appeared = false

    func body(content: Content) -> some View {
        content.task(id: version) {
            if appeared {
                do { try await Task.sleep(for: debounce) } catch { return }
            }
            appeared = true
            await action()
        }
    }
}
