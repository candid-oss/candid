/// Pure trailing-edge timing policy. `now` is monotonic seconds supplied by the
/// monitor; tests advance it without sleeping or observing real applications.
struct DesktopAXQuietPeriod: Sendable {
    struct Ready: Sendable, Equatable {
        let trigger: String
        fileprivate let revision: UInt64
    }
    var maximumWait: Double = 10
    private(set) var revision: UInt64 = 0
    private var burstStarted: Double?
    private var deadline: Double?
    private var textQuietUntil: Double?
    private var latestTrigger: String?

    mutating func note(trigger: String, at now: Double) {
        revision &+= 1
        let isText = trigger == "content" || trigger == "selection"
        if isText {
            if burstStarted == nil { burstStarted = now }
            let until = min(now + 1.5, (burstStarted ?? now) + maximumWait)
            latestTrigger = trigger; textQuietUntil = until; deadline = until
        } else if let textQuietUntil {
            // Keep the most recent text trigger during a pending input burst.
            // Window/focus signals neither replace it nor postpone its deadline.
            deadline = textQuietUntil
        } else {
            latestTrigger = trigger; deadline = now + 0.3
        }
    }

    func isTextSettling(at now: Double) -> Bool { textQuietUntil.map { now < $0 } ?? false }

    func remaining(at now: Double) -> Double? { deadline.map { max(0, $0 - now) } }

    mutating func takeReady(at now: Double) -> Ready? {
        guard let deadline, now >= deadline, let trigger = latestTrigger else { return nil }
        self.deadline = nil; textQuietUntil = nil; latestTrigger = nil; burstStarted = nil
        return Ready(trigger: trigger, revision: revision)
    }

    func isCurrent(_ ready: Ready) -> Bool { ready.revision == revision }

    mutating func cancel() {
        revision &+= 1
        deadline = nil; textQuietUntil = nil; latestTrigger = nil; burstStarted = nil
    }
}
