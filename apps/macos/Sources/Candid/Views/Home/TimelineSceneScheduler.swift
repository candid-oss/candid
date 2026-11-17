import Foundation

/// One scene build at a time. Hidden windows retain no queued UI work; showing them requests fresh inputs.
@MainActor final class TimelineSceneScheduler {
    private var visible = true
    private var task: Task<Void, Never>?
    private var queued = false
    private var generation: UInt64 = 0
    private let delay: Duration
    init(delay: Duration = .milliseconds(400)) { self.delay = delay }

    func setVisible(_ value: Bool) {
        visible = value
        if !value { stop() }
    }

    func stop() {
        generation &+= 1; task?.cancel(); task = nil; queued = false
    }

    func request(_ build: @escaping @MainActor () async -> Void) {
        guard visible else { return }
        queued = true
        guard task == nil else { return }
        let expected = generation
        task = Task { [weak self] in
            guard let self else { return }
            repeat {
                guard !Task.isCancelled, self.visible, self.generation == expected else { break }
                self.queued = false
                await build()
                do { try await Task.sleep(for: self.delay) } catch { break }
            } while self.queued
            if self.generation == expected { self.task = nil }
        }
    }
}
