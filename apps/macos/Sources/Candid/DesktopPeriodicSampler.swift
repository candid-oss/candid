import Foundation

/// One cancellable sleeper and one awaited sample, with no catch-up after sleep.
@MainActor final class DesktopPeriodicSampler {
    typealias Sleep = @Sendable (Duration) async throws -> Void
    private let sleep: Sleep
    private var task: Task<Void, Never>?
    private var generation: UInt64 = 0
    init(sleep: @escaping Sleep = { try await Task.sleep(for: $0) }) { self.sleep = sleep }
    func start(interval: Double, sample: @escaping @MainActor () async -> Void) {
        stop()
        let expected = generation, sleep = self.sleep
        task = Task { [weak self] in
            while !Task.isCancelled {
                do { try await sleep(.seconds(interval)) } catch { return }
                guard !Task.isCancelled, self?.generation == expected else { return }
                await sample()
            }
        }
    }
    func stop() { generation &+= 1; task?.cancel(); task = nil }
}
