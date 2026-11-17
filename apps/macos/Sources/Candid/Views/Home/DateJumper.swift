import Observation
import SwiftUI

/// How much was recorded on one day, from the core's per-day counts (`app_day_counts`). Counts only.
struct DayAmount: Equatable, Sendable {
    var screenMinutes = 0
    var media = 0
    var ai = 0

    /// Minutes-equivalent: a recording or image weighs ten minutes of screen, an AI instruction three.
    var score: Int { screenMinutes + 10 * media + 3 * ai }

    /// 0 nothing · 1 a glance · 2 under 1½ h · 3 under 4 h · 4 a full day. Fixed steps, so a month never
    /// looks busier just because the rest of it was quiet.
    var level: Int {
        switch score {
        case 0: 0
        case ..<20: 1
        case ..<90: 2
        case ..<240: 3
        default: 4
        }
    }

    var summary: String {
        var parts: [String] = []
        if screenMinutes > 0 { parts.append("屏幕约 " + TimeInterval(screenMinutes * 60).spokenDuration) }
        if media > 0 { parts.append("\(media) 条录音或图片") }
        if ai > 0 { parts.append("\(ai) 条 AI 指令") }
        return parts.isEmpty ? "没有记录" : parts.joined(separator: "，")
    }
}

/// The month grid of the date jumper as values: six weeks starting on the calendar's first weekday, each day
/// marked today / selected / outside the month / in the future, with its amount. Pure, so it is testable.
struct DateJumperGrid: Equatable {
    struct Cell: Identifiable, Equatable {
        let date: Date
        let day: Int
        let inMonth: Bool
        let isToday: Bool
        let isSelected: Bool
        let isFuture: Bool
        let amount: DayAmount
        var id: Date { date }
        var level: Int { amount.level }
    }

    let month: Date
    let weeks: [[Cell]]

    init(month: Date, selected: Date, today: Date, amounts: [Date: DayAmount], calendar: Calendar = .current) {
        let first = Self.monthStart(month, calendar: calendar)
        self.month = first
        let todayStart = calendar.startOfDay(for: today), selectedStart = calendar.startOfDay(for: selected)
        let days = Self.days(for: first, calendar: calendar)
        let cells = days.map { day in
            Cell(date: day.start, day: calendar.component(.day, from: day.start),
                 inMonth: calendar.isDate(day.start, equalTo: first, toGranularity: .month),
                 isToday: day.start == todayStart, isSelected: day.start == selectedStart, isFuture: day.start > todayStart,
                 amount: amounts[day.start] ?? DayAmount())
        }
        weeks = stride(from: 0, to: cells.count, by: 7).map { Array(cells[$0..<min($0 + 7, cells.count)]) }
    }

    static func monthStart(_ date: Date, calendar: Calendar = .current) -> Date {
        calendar.dateInterval(of: .month, for: date)?.start ?? calendar.startOfDay(for: date)
    }

    /// The 42 local days the grid of `month` shows, as the intervals the core is asked about. Each is a real local
    /// day, so a daylight-saving change gives a 23- or 25-hour interval.
    static func days(for month: Date, calendar: Calendar = .current) -> [DateInterval] {
        let first = monthStart(month, calendar: calendar)
        let weekday = calendar.component(.weekday, from: first)
        let lead = (weekday - calendar.firstWeekday + 7) % 7
        guard let start = calendar.date(byAdding: .day, value: -lead, to: first) else { return [] }
        return (0..<42).compactMap { offset in
            guard let day = calendar.date(byAdding: .day, value: offset, to: start) else { return nil }
            return calendar.dateInterval(of: .day, for: day)
        }
    }

    /// Weekday initials in grid order (一 二 … 日 in Chinese with Monday first).
    static func weekdaySymbols(calendar: Calendar = .current) -> [String] {
        let symbols = calendar.veryShortStandaloneWeekdaySymbols
        return (0..<7).map { symbols[(calendar.firstWeekday - 1 + $0) % 7] }
    }

    static func title(_ month: Date, calendar: Calendar = .current) -> String {
        "\(calendar.component(.year, from: month))年\(calendar.component(.month, from: month))月"
    }

    /// Keyboard movement: ←→ one day, ↑↓ one week; never past today.
    static func move(_ focus: Date, by days: Int, today: Date, calendar: Calendar = .current) -> Date {
        let target = calendar.date(byAdding: .day, value: days, to: calendar.startOfDay(for: focus)) ?? focus
        return min(target, calendar.startOfDay(for: today))
    }
}

/// Per-day amounts for the months the jumper has shown, fetched one grid at a time and refreshed when it opens.
@MainActor @Observable final class DateJumperModel {
    typealias Fetch = @MainActor ([DateInterval]) async throws -> [Date: DayAmount]
    private(set) var amounts: [Date: DayAmount] = [:]
    private(set) var failed = false
    @ObservationIgnored private var loaded: Set<Date> = []
    @ObservationIgnored private let fetch: Fetch
    @ObservationIgnored private let calendar: Calendar

    init(calendar: Calendar = .current, fetch: @escaping Fetch) { self.calendar = calendar; self.fetch = fetch }

    /// Loads the grid of `month` unless it was loaded since the jumper opened (`refresh` forgets that).
    func load(_ month: Date) async {
        let first = DateJumperGrid.monthStart(month, calendar: calendar)
        guard loaded.insert(first).inserted else { return }
        let days = DateJumperGrid.days(for: first, calendar: calendar)
        do {
            let found = try await fetch(days)
            var next = amounts
            for day in days { next[day.start] = found[day.start] }
            if next != amounts { amounts = next }
            if failed { failed = false }
        } catch {
            loaded.remove(first)
            failed = true
        }
    }

    /// Opening the jumper again re-reads what it shows (today keeps growing).
    func refresh() { loaded.removeAll() }
}

/// The date jumper: a compact month grid in a popover. Each day shows how much was recorded as a soft tint (none,
/// a glance … a full day); empty days are dimmed and future days cannot be picked. Today has a ring, the shown day
/// is filled. Arrows move, Return jumps, Esc closes; ‹ › change the month.
struct DateJumperView: View {
    let selected: Date
    let model: DateJumperModel
    let onPick: (Date) -> Void
    let onClose: () -> Void

    @ViewState private var month: Date
    @ViewState private var focus: Date
    @ViewState private var hovered: Date?
    @FocusState private var focused: Bool
    private let calendar = Calendar.current

    init(selected: Date, model: DateJumperModel, onPick: @escaping (Date) -> Void, onClose: @escaping () -> Void) {
        self.selected = selected; self.model = model; self.onPick = onPick; self.onClose = onClose
        _month = ViewState(initialValue: DateJumperGrid.monthStart(selected))
        _focus = ViewState(initialValue: Calendar.current.startOfDay(for: selected))
    }

    private static let cell: CGFloat = 34
    private static let tints: [Double] = [0, 0.1, 0.2, 0.32, 0.48]

    var body: some View {
        let today = Date()
        let grid = DateJumperGrid(month: month, selected: selected, today: today, amounts: model.amounts, calendar: calendar)
        let isCurrentMonth = calendar.isDate(month, equalTo: today, toGranularity: .month)
        VStack(alignment: .leading, spacing: Space.s) {
            HStack(spacing: Space.xxs) {
                Text(DateJumperGrid.title(month, calendar: calendar)).typography(.headline, digits: true)
                    .contentTransition(.numericText())
                Spacer(minLength: Space.xs)
                ChromeButton(systemImage: "chevron.left", help: "上个月") { show(month: -1) }
                ChromeButton(systemImage: "chevron.right", help: "下个月") { show(month: 1) }
                    .disabled(isCurrentMonth).opacity(isCurrentMonth ? 0.35 : 1)
            }
            VStack(spacing: Space.xxs) {
                HStack(spacing: Space.xxs) {
                    ForEach(Array(DateJumperGrid.weekdaySymbols(calendar: calendar).enumerated()), id: \.offset) { _, symbol in
                        Text(symbol).typography(.caption).foregroundStyle(.tertiary).frame(width: Self.cell)
                    }
                }
                ForEach(grid.weeks, id: \.first?.id) { week in
                    HStack(spacing: Space.xxs) {
                        ForEach(week) { cell in day(cell) }
                    }
                }
            }
            .id(month)
            .transition(.opacity)
            HStack(spacing: Space.xs) {
                legend
                Spacer(minLength: Space.xs)
                if model.failed { Text("记录量暂时读不到").typography(.caption).foregroundStyle(.secondary) }
                if !calendar.isDateInToday(selected) {
                    QuietButton(title: "今天") { onPick(today) }
                }
            }
        }
        .padding(Space.m)
        .fixedSize()
        .animation(Motion.snappy, value: month)
        .animation(Motion.snappy, value: model.amounts)
        .focusable().focusEffectDisabled().focused($focused)
        .onAppear { focused = true }
        .onKeyPress(.leftArrow) { step(-1) }
        .onKeyPress(.rightArrow) { step(1) }
        .onKeyPress(.upArrow) { step(-7) }
        .onKeyPress(.downArrow) { step(7) }
        .onKeyPress(.return) { onPick(focus); return .handled }
        .onKeyPress(.escape) { onClose(); return .handled }
        .task(id: month) {
            await model.load(month)
            // The month before is usually next; reading it now makes ‹ instant.
            if let previous = calendar.date(byAdding: .month, value: -1, to: month) { await model.load(previous) }
        }
    }

    private func day(_ cell: DateJumperGrid.Cell) -> some View {
        let shape = RoundedRectangle(cornerRadius: Radius.small, style: .continuous)
        let isFocus = focused && cell.date == focus && !cell.isSelected
        let empty = cell.level == 0
        return Button { onPick(cell.date) } label: {
            Text("\(cell.day)")
                .typography(.body, cell.isToday || cell.isSelected ? .semibold : nil, digits: true)
                .foregroundStyle(cell.isSelected ? AnyShapeStyle(.white)
                                 : cell.isToday ? AnyShapeStyle(Palette.accent)
                                 : !cell.inMonth || cell.isFuture ? AnyShapeStyle(.quaternary)
                                 : empty ? AnyShapeStyle(.tertiary) : AnyShapeStyle(.primary))
                .frame(width: Self.cell, height: Self.cell)
                .background {
                    ZStack {
                        if cell.isSelected {
                            shape.fill(Palette.accent)
                        } else {
                            shape.fill(Palette.accent.opacity(Self.tints[cell.level] * (cell.inMonth ? 1 : 0.4)))
                            if hovered == cell.date && !cell.isFuture { shape.fill(Surface.hover.color) }
                        }
                        if cell.isToday && !cell.isSelected { shape.strokeBorder(Palette.accent.opacity(0.6), lineWidth: 1) }
                    }
                }
                .selectionEdge(isFocus, radius: Radius.small, glow: true)
                .contentShape(shape)
        }
        .buttonStyle(.plain)
        .disabled(cell.isFuture)
        .onHover { inside in hovered = inside ? cell.date : (hovered == cell.date ? nil : hovered) }
        .help(cell.isFuture ? "" : "\(calendar.component(.month, from: cell.date))月\(cell.day)日 · \(cell.amount.summary)")
        .accessibilityLabel("\(calendar.component(.month, from: cell.date))月\(cell.day)日")
        .accessibilityValue(cell.amount.summary + (cell.isToday ? "，今天" : "") + (cell.isSelected ? "，正在查看" : ""))
    }

    /// 少 ▢▢▢▢ 多: what the tints mean.
    private var legend: some View {
        HStack(spacing: 3) {
            Text("少").typography(.caption).foregroundStyle(.tertiary)
            ForEach(1..<Self.tints.count, id: \.self) { level in
                RoundedRectangle(cornerRadius: Radius.small / 3, style: .continuous)
                    .fill(Palette.accent.opacity(Self.tints[level])).frame(width: 10, height: 10)
            }
            Text("多").typography(.caption).foregroundStyle(.tertiary)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("颜色越深记录越多")
    }

    private func show(month delta: Int) {
        guard let next = calendar.date(byAdding: .month, value: delta, to: month) else { return }
        let start = DateJumperGrid.monthStart(next, calendar: calendar)
        guard start <= Date() else { return }
        month = start
        // Keep the keyboard focus inside the shown month (same day number where possible, never past today).
        let day = min(calendar.component(.day, from: focus), calendar.range(of: .day, in: .month, for: start)?.count ?? 28)
        let candidate = calendar.date(byAdding: .day, value: day - 1, to: start) ?? start
        focus = min(candidate, calendar.startOfDay(for: Date()))
    }

    private func step(_ days: Int) -> KeyPress.Result {
        focus = DateJumperGrid.move(focus, by: days, today: Date(), calendar: calendar)
        let shown = DateJumperGrid.monthStart(focus, calendar: calendar)
        if shown != month { month = shown }
        return .handled
    }
}
