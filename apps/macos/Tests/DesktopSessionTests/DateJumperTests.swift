import AppKit
import DesktopCore
import Foundation
import SwiftUI
import Testing
@testable import Candid

/// The home page's date jumper: month grid, amounts, keyboard movement, loading, and the per-day counts read from a
/// throwaway core. Synthetic data only.
@Suite struct DateJumperTests {
    private var calendar: Calendar {
        var c = Calendar(identifier: .gregorian)
        c.timeZone = TimeZone(identifier: "America/New_York")!
        c.firstWeekday = 2   // Monday, as in zh_CN
        c.locale = Locale(identifier: "zh_CN")
        return c
    }
    private func day(_ y: Int, _ m: Int, _ d: Int, hour: Int = 0) -> Date {
        calendar.date(from: DateComponents(year: y, month: m, day: d, hour: hour))!
    }

    @Test func gridHasSixMondayFirstWeeksWithTodaySelectedAndFutureMarked() {
        let amounts = [day(2026, 11, 3): DayAmount(screenMinutes: 200), day(2026, 11, 5): DayAmount(screenMinutes: 5)]
        let grid = DateJumperGrid(month: day(2026, 11, 20), selected: day(2026, 11, 3, hour: 12), today: day(2026, 11, 13, hour: 15),
                                  amounts: amounts, calendar: calendar)
        #expect(grid.month == day(2026, 11, 1))
        #expect(grid.weeks.count == 6 && grid.weeks.allSatisfy { $0.count == 7 })
        // 2026-11-01 is a Sunday: the grid starts on Monday 26 October.
        #expect(grid.weeks[0][0].date == day(2026, 10, 26) && !grid.weeks[0][0].inMonth)
        #expect(grid.weeks[0][6].date == day(2026, 11, 1) && grid.weeks[0][6].inMonth)
        let cells = grid.weeks.flatMap { $0 }
        #expect(cells.filter(\.isToday).map(\.day) == [13])
        #expect(cells.filter(\.isSelected).map(\.day) == [3])
        #expect(cells.first { $0.date == day(2026, 11, 14) }?.isFuture == true)
        #expect(cells.first { $0.date == day(2026, 11, 13) }?.isFuture == false)
        #expect(cells.first { $0.date == day(2026, 11, 3) }?.level == 3)
        #expect(cells.first { $0.date == day(2026, 11, 5) }?.level == 1)
        #expect(cells.first { $0.date == day(2026, 11, 4) }?.level == 0)
        #expect(DateJumperGrid.weekdaySymbols(calendar: calendar) == ["一", "二", "三", "四", "五", "六", "日"])
        #expect(DateJumperGrid.title(grid.month, calendar: calendar) == "2026年11月")
    }

    @Test func daysAreRealLocalDaysAcrossDaylightSaving() {
        // New York leaves daylight saving on 2026-11-01: that day lasts 25 hours.
        let days = DateJumperGrid.days(for: day(2026, 11, 1), calendar: calendar)
        #expect(days.count == 42)
        let sunday = days.first { $0.start == day(2026, 11, 1) }!
        #expect(sunday.duration == 25 * 3600)
        #expect(zip(days, days.dropFirst()).allSatisfy { $0.end == $1.start }, "no gaps or overlaps")
    }

    @Test func amountLevelsUseFixedSteps() {
        #expect(DayAmount().level == 0)
        #expect(DayAmount(screenMinutes: 19).level == 1)
        #expect(DayAmount(screenMinutes: 20).level == 2)
        #expect(DayAmount(media: 2).level == 2, "a recording weighs ten minutes")
        #expect(DayAmount(screenMinutes: 239).level == 3)
        #expect(DayAmount(screenMinutes: 240).level == 4)
        #expect(DayAmount().summary == "没有记录")
        #expect(DayAmount(screenMinutes: 90, media: 1, ai: 2).summary.contains("1 条录音或图片"))
    }

    @Test func keyboardMovesByDayAndWeekButNeverPastToday() {
        let today = day(2026, 11, 13, hour: 15)
        #expect(DateJumperGrid.move(day(2026, 11, 13), by: -1, today: today, calendar: calendar) == day(2026, 11, 12))
        #expect(DateJumperGrid.move(day(2026, 11, 3), by: -7, today: today, calendar: calendar) == day(2026, 10, 27))
        #expect(DateJumperGrid.move(day(2026, 11, 10), by: 7, today: today, calendar: calendar) == day(2026, 11, 13))
        #expect(DateJumperGrid.move(day(2026, 11, 13), by: 1, today: today, calendar: calendar) == day(2026, 11, 13))
    }

    @MainActor @Test func modelLoadsEachGridOnceUntilRefreshedAndKeepsDataOnFailure() async {
        var calls = 0, fail = false
        let calendar = self.calendar
        let model = DateJumperModel(calendar: calendar) { days in
            calls += 1
            if fail { throw CancellationError() }
            return [days[10].start: DayAmount(screenMinutes: 30)]
        }
        let month = day(2026, 11, 1)
        await model.load(month)
        await model.load(day(2026, 11, 20))
        #expect(calls == 1)
        #expect(model.amounts.values.filter { $0.level > 0 }.count == 1)
        fail = true
        model.refresh()
        await model.load(month)
        #expect(calls == 2 && model.failed)
        #expect(model.amounts.values.filter { $0.level > 0 }.count == 1, "what was shown stays after a failed read")
        fail = false
        await model.load(month)
        #expect(calls == 3 && !model.failed, "a failed grid is read again next time")
    }

    /// Through a real throwaway core: the synthetic demo day counts for today, yesterday has nothing.
    @MainActor @Test func coreCountsTheSyntheticDemoDay() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("date-jumper-synthetic-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { TestCore.finish(root) }
        let session = DesktopSession(isDemo: true, dataRoot: root, core: try await TestCore.ready(root))
        defer { session.prepareToQuit() }
        await session.bootstrap()
        for _ in 0..<1000 where session.timelineEvents.isEmpty { try await Task.sleep(for: .milliseconds(10)) }
        let cal = Calendar.current
        let today = cal.dateInterval(of: .day, for: Date())!
        let yesterday = cal.dateInterval(of: .day, for: today.start.addingTimeInterval(-3600))!
        var amounts: [Date: DayAmount] = [:]
        for _ in 0..<200 {
            amounts = try await session.dayAmounts([yesterday, today])
            if (amounts[today.start]?.screenMinutes ?? 0) > 30 { break }
            try await Task.sleep(for: .milliseconds(25))
        }
        #expect((amounts[today.start]?.screenMinutes ?? 0) > 30)
        #expect(amounts[today.start]?.level ?? 0 >= 2)
        #expect(amounts[yesterday.start] == nil)
    }

    /// Offscreen render in light and dark with synthetic amounts (written to `$CANDID_RENDER_DIR` when set).
    @MainActor @Test func rendersInLightAndDark() async throws {
        _ = NSApplication.shared
        let cal = Calendar.current
        let model = DateJumperModel { days in
            var result: [Date: DayAmount] = [:]
            for (index, day) in days.enumerated() where day.start <= Date() && index % 5 != 2 {
                result[day.start] = DayAmount(screenMinutes: [8, 45, 150, 320, 0][index % 5], media: index % 7 == 0 ? 1 : 0)
            }
            return result
        }
        let selected = cal.date(byAdding: .day, value: -3, to: Date())!
        await model.load(selected)
        for dark in [false, true] {
            let content = DateJumperView(selected: selected, model: model, onPick: { _ in }, onClose: {})
                .background(dark ? Color(white: 0.16) : Color(white: 0.97))
                .environment(\.colorScheme, dark ? .dark : .light)
            let view = NSHostingView(rootView: content)
            view.frame = NSRect(origin: .zero, size: view.fittingSize)
            let window = NSWindow(contentRect: view.frame, styleMask: [.borderless], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            window.contentView = view; view.layoutSubtreeIfNeeded()
            let bitmap = try #require(view.bitmapImageRepForCachingDisplay(in: view.bounds))
            view.cacheDisplay(in: view.bounds, to: bitmap)
            let png = try #require(bitmap.representation(using: .png, properties: [:]))
            #expect(view.fittingSize.width > 250 && view.fittingSize.width < 360, "compact: \(view.fittingSize)")
            if let dir = ProcessInfo.processInfo.environment["CANDID_RENDER_DIR"] {
                try png.write(to: URL(fileURLWithPath: dir).appendingPathComponent(dark ? "date-jumper-dark.png" : "date-jumper-light.png"))
            }
            window.close()
        }
    }
}
