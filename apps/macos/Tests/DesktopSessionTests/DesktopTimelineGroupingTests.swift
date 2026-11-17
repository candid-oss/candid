import DesktopCore
import Foundation
import Testing
@testable import Candid

@Suite struct DesktopTimelineGroupingTests {
    private func event(_ id: String, app: String = "test.synthetic.a", kind: DesktopRecordKind = .context,
                       run: String = "synthetic-run", at: Double = 100, image: Bool = false) -> DesktopTimelineEvent {
        let record = DesktopRecord(id: id, timestamp: Date(timeIntervalSince1970: at), kind: kind,
            appName: app, bundleID: app, windowTitle: "Synthetic " + id, runID: run, activityID: id)
        return DesktopTimelineEvent(record: record, hasScreenshot: image)
    }

    @Test func onlyAdjacentSameApplicationActivitiesFoldWithoutChangingChildIdentity() {
        let events = [event("a3", at: 300), event("a2", at: 200), event("b", app: "test.synthetic.b", at: 150), event("a1", at: 100)]
        let groups = DesktopTimelineGrouping.groups(events)
        #expect(groups.map { $0.events.map(\.id) } == [["a3", "a2"], ["b"], ["a1"]])
        #expect(groups[0].isCollapsible && !groups[1].isCollapsible)
        #expect(groups.flatMap(\.events) == events)
        #expect(groups[0].startDate == Date(timeIntervalSince1970: 200))
        #expect(groups[0].endDate == Date(timeIntervalSince1970: 300))
    }

    @Test func markerSystemGapEmptySourceAndNewRunBreakAdjacency() {
        for kind in [DesktopRecordKind.marker, .system, .gap] {
            let groups = DesktopTimelineGrouping.groups([event("new"), event("boundary", kind: kind), event("old")])
            #expect(groups.count == 3)
        }
        #expect(DesktopTimelineGrouping.groups([event("new", run: "new-run"), event("old")]).count == 2)
        #expect(DesktopTimelineGrouping.groups([event("new", app: ""), event("old", app: "")]).count == 2)
    }

    @Test func filteringCannotCreateAdjacencyAcrossHiddenDifferentApplication() {
        let events = [event("a2", image: true), event("b", app: "test.synthetic.b"), event("a1", image: true)]
        let groups = DesktopTimelineGrouping.groups(events, include: { $0.hasScreenshot })
        #expect(groups.count == 2 && groups.allSatisfy { !$0.isCollapsible })
        #expect(groups.flatMap(\.events).map(\.id) == ["a2", "a1"])
    }

    @Test func searchResultsStayFlatBecauseMissingRowsCannotProveContinuity() {
        let groups = DesktopTimelineGrouping.groups([event("a2"), event("a1")], searchActive: true)
        #expect(groups.count == 2 && groups.allSatisfy { !$0.isCollapsible })
    }

    @Test func groupIdentityUsesOldestUnfilteredMemberWhenNewActivitiesArrive() {
        let first = DesktopTimelineGrouping.groups([event("a2", image: true), event("a1")])
        let next = DesktopTimelineGrouping.groups([event("a3", image: true), event("a2", image: true), event("a1")])
        let filtered = DesktopTimelineGrouping.groups([event("a3", image: true), event("a2", image: true), event("a1")], include: { $0.hasScreenshot })
        #expect(first[0].id == next[0].id && next[0].id == filtered[0].id)
        #expect(filtered[0].events.map(\.id) == ["a3", "a2"])
    }

    @Test func userCollapseSurvivesBackgroundArrivalUntilExplicitChildSelection() {
        let groups = DesktopTimelineGrouping.groups([event("a2"), event("a1")])
        let id = groups[0].id
        var expansion = DesktopTimelineGroupExpansion()
        expansion.reconcile(groups, selectedID: "a1")
        #expect(expansion.expandedIDs.contains(id))
        expansion.setExpanded(false, groupID: id)
        let next = DesktopTimelineGrouping.groups([event("a3"), event("a2"), event("a1")])
        expansion.reconcile(next, selectedID: "a1")
        #expect(!expansion.expandedIDs.contains(id) && expansion.collapsedIDs.contains(id))
        expansion.reconcile(next, selectedID: "a2", explicitSelection: true)
        #expect(expansion.expandedIDs.contains(id) && !expansion.collapsedIDs.contains(id))
    }

    @Test func onlySelectedGroupAutoExpandsAndDisappearedGroupsDoNotAccumulate() {
        let groups = DesktopTimelineGrouping.groups([event("a2"), event("a1"), event("b2", app: "test.synthetic.b"), event("b1", app: "test.synthetic.b")])
        var expansion = DesktopTimelineGroupExpansion()
        expansion.reconcile(groups, selectedID: "b1")
        #expect(expansion.expandedIDs == [groups[1].id])
        expansion.reconcile([], selectedID: nil)
        #expect(expansion.expandedIDs.isEmpty && expansion.collapsedIDs.isEmpty)
    }
    @Test func flatProjectionHasGloballyUniqueParentAndChildRowIDs() {
        // A child ID deliberately resembles another group's structural ID.
        let groups = DesktopTimelineGrouping.groups([event("a2"), event("a1"), event("application:a1", app: "test.synthetic.b")])
        let rows = DesktopTimelineGrouping.rows(groups, expandedIDs: [groups[0].id])
        #expect(rows.count == 4 && Set(rows.map(\.id)).count == rows.count)
        #expect(rows.map(\.id) == ["header:application:a1", "activity:a2", "activity:a1", "activity:application:a1"])
    }

    @Test func singleActivityBecomingGroupedKeepsItsVisibleRowIdentity() throws {
        let single = DesktopTimelineGrouping.groups([event("a1")])
        let singleRow = try #require(DesktopTimelineGrouping.rows(single, expandedIDs: []).first)
        let grouped = DesktopTimelineGrouping.groups([event("a2"), event("a1")])
        let rows = DesktopTimelineGrouping.rows(grouped, expandedIDs: [grouped[0].id])
        let preserved = try #require(rows.first { row in
            if case .activity(let event, _) = row { return event.id == "a1" }; return false
        })
        #expect(singleRow.id == preserved.id)
        #expect(preserved.id == "activity:a1")
    }

    @Test func collapseRemovesOnlyThatGroupsChildrenAndPreservesOtherRowOrder() {
        let groups = DesktopTimelineGrouping.groups([
            event("a2"), event("a1"), event("boundary", app: "", kind: .system),
            event("b2", app: "test.synthetic.b"), event("b1", app: "test.synthetic.b"), event("tail", app: "test.synthetic.c")
        ])
        let expandedIDs = Set(groups.filter(\.isCollapsible).map(\.id))
        let before = DesktopTimelineGrouping.rows(groups, expandedIDs: expandedIDs)
        let after = DesktopTimelineGrouping.rows(groups, expandedIDs: expandedIDs.subtracting([groups[0].id]))
        #expect(after.map(\.id) == before.map(\.id).filter { $0 != "activity:a2" && $0 != "activity:a1" })
        #expect(after.count == before.count - 2)
        if case .application(let group, let expanded) = after[0] {
            #expect(group.id == groups[0].id && !expanded)
        } else { Issue.record("first row must remain the collapsed structural parent") }
    }

}
