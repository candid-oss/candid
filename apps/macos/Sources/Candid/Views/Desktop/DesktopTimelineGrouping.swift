import DesktopCore
import Foundation

/// A view-only fold over the ordered activity list. Raw records and activity
/// identity stay untouched; filtering never creates new adjacency.
struct DesktopTimelineApplicationGroup: Identifiable, Sendable, Equatable {
    let id: String
    let events: [DesktopTimelineEvent]
    var isCollapsible: Bool { events.count > 1 }
    var bundleID: String { events.first?.record.bundleID ?? "" }
    var appName: String { events.first?.record.appName ?? "" }
    var startDate: Date { events.map(\.startDate).min() ?? .distantPast }
    var endDate: Date { events.map(\.endDate).max() ?? .distantPast }
}

/// Exactly one native List row per value. Flattening avoids nested disclosure
/// containers reusing an activity row's height when expansion changes.
enum DesktopTimelineVisibleRow: Identifiable, Sendable, Equatable {
    case application(DesktopTimelineApplicationGroup, expanded: Bool)
    case activity(DesktopTimelineEvent, nested: Bool)

    var id: String {
        switch self {
        case .application(let group, _): "header:" + group.id
        case .activity(let event, _): "activity:" + event.id
        }
    }
}

enum DesktopTimelineGrouping {
    static func groups(_ events: [DesktopTimelineEvent], searchActive: Bool = false,
                       include: (DesktopTimelineEvent) -> Bool = { _ in true }) -> [DesktopTimelineApplicationGroup] {
        // Search is already a partial database result; it cannot establish
        // whether a different application occurred between matching rows.
        if searchActive {
            return events.filter(include).map { .init(id: "event:" + $0.id, events: [$0]) }
        }
        var runs: [[DesktopTimelineEvent]] = []
        for event in events {
            if let previous = runs.last?.last, canJoin(previous, event) {
                runs[runs.count - 1].append(event)
            } else { runs.append([event]) }
        }
        return runs.compactMap { run in
            let visible = run.filter(include)
            guard !visible.isEmpty, let oldest = run.last else { return nil }
            // The list is newest-first. A new activity at the head does not
            // change the oldest member, so expansion identity remains stable.
            return .init(id: "application:" + oldest.id, events: visible)
        }
    }

    static func rows(_ groups: [DesktopTimelineApplicationGroup], expandedIDs: Set<String>) -> [DesktopTimelineVisibleRow] {
        var rows: [DesktopTimelineVisibleRow] = []
        for group in groups {
            if group.isCollapsible {
                let expanded = expandedIDs.contains(group.id)
                rows.append(.application(group, expanded: expanded))
                if expanded { rows.append(contentsOf: group.events.map { .activity($0, nested: true) }) }
            } else {
                rows.append(contentsOf: group.events.map { .activity($0, nested: false) })
            }
        }
        return rows
    }

    private static func canJoin(_ first: DesktopTimelineEvent, _ second: DesktopTimelineEvent) -> Bool {
        func eligible(_ event: DesktopTimelineEvent) -> Bool {
            guard !event.record.bundleID.isEmpty else { return false }
            switch event.record.kind {
            case .application, .window, .context, .screenshot: return true
            case .marker, .system, .gap: return false
            }
        }
        return eligible(first) && eligible(second) && first.record.bundleID == second.record.bundleID &&
            first.record.runID == second.record.runID
    }
}

/// User collapse wins over background arrivals. A new explicit child selection
/// can reveal its group; it never changes which activity is selected.
struct DesktopTimelineGroupExpansion {
    private(set) var expandedIDs: Set<String> = []
    private(set) var collapsedIDs: Set<String> = []

    mutating func setExpanded(_ expanded: Bool, groupID: String) {
        if expanded { expandedIDs.insert(groupID); collapsedIDs.remove(groupID) }
        else { expandedIDs.remove(groupID); collapsedIDs.insert(groupID) }
    }

    mutating func reconcile(_ groups: [DesktopTimelineApplicationGroup], selectedID: String?, explicitSelection: Bool = false) {
        let live = Set(groups.map(\.id))
        expandedIDs.formIntersection(live); collapsedIDs.formIntersection(live)
        guard let selectedID, let group = groups.first(where: { $0.isCollapsible && $0.events.contains { $0.id == selectedID } }) else { return }
        if explicitSelection || !collapsedIDs.contains(group.id) { expandedIDs.insert(group.id) }
        if explicitSelection { collapsedIDs.remove(group.id) }
    }
}
