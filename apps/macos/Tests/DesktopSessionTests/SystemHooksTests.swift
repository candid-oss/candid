import Foundation
import Testing
@testable import Candid

@Test func candidURLsParseTheDocumentedCommandsAndRejectTheRest() throws {
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = try #require(TimeZone(identifier: "Asia/Singapore"))
    func parse(_ text: String) -> CandidURL? { URL(string: text).flatMap { CandidURL.parse($0, calendar: calendar) } }

    #expect(parse("candid://pause") == .pause(minutes: nil))
    #expect(parse("candid://pause?minutes=30") == .pause(minutes: 30))
    #expect(parse("CANDID://Pause") == .pause(minutes: nil))
    #expect(parse("candid:///pause") == .pause(minutes: nil))
    #expect(parse("candid://pause?minutes=0") == nil)
    #expect(parse("candid://pause?minutes=abc") == nil)
    #expect(parse("candid://pause?minutes=1441") == nil)
    #expect(parse("candid://resume") == .resume)
    #expect(parse("candid://open") == .open(date: nil))
    let day = try #require(calendar.date(from: DateComponents(year: 2026, month: 11, day: 25)))
    #expect(parse("candid://open?date=2026-11-25") == .open(date: day))
    #expect(parse("candid://open?date=2026-02-30") == nil)
    #expect(parse("candid://open?date=tomorrowish") == nil)
    #expect(parse("candid://search?q=%E5%91%A8%E6%8A%A5") == .search("周报"))
    #expect(parse("candid://search?q=%20") == nil)
    #expect(parse("candid://delete") == nil)
    #expect(parse("https://pause") == nil)
}

@Test func relativeDaysResolveToTheStartOfTheLocalDay() throws {
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = try #require(TimeZone(identifier: "Asia/Singapore"))
    let now = try #require(calendar.date(from: DateComponents(year: 2026, month: 11, day: 25, hour: 15)))
    #expect(CandidURL.day("today", calendar: calendar, now: now) == calendar.date(from: DateComponents(year: 2026, month: 11, day: 25)))
    #expect(CandidURL.day("yesterday", calendar: calendar, now: now) == calendar.date(from: DateComponents(year: 2026, month: 11, day: 24)))
}

@Test func systemHooksDefaultToConservativeValues() throws {
    let name = "synthetic.systemhooks." + UUID().uuidString
    let defaults = try #require(UserDefaults(suiteName: name))
    defer { defaults.removePersistentDomain(forName: name) }
    #expect(SystemHooks.value(SystemHooks.urlResumeKey, default: false, defaults: defaults) == false)
    defaults.set(true, forKey: SystemHooks.urlResumeKey)
    #expect(SystemHooks.value(SystemHooks.urlResumeKey, default: false, defaults: defaults) == true)
    defaults.set(false, forKey: SystemHooks.intentsKey)
    #expect(SystemHooks.value(SystemHooks.intentsKey, default: true, defaults: defaults) == false)
}

@Test func daySummaryListsBlocksInOrderWithSummaries() throws {
    let zone = try #require(TimeZone(identifier: "Asia/Singapore"))
    let json = """
    {"window": {}, "blocks": [
      {"start": "2026-11-25T09:12:00.000+08:00", "end": "2026-11-25T10:05:30.000+08:00", "app": "Xcode", "window": "Candid",
       "title": "写系统集成", "summary": "接入快捷指令和链接", "by": "model"},
      {"start": "2026-11-25T10:10:00.000+08:00", "end": "2026-11-25T10:40:00.000+08:00", "app": "Safari", "window": "文档",
       "title": "", "summary": null, "by": "rule"}
    ], "daily": [{"day": "2026-11-25", "text": "上午主要在写代码。"}]}
    """
    let text = DaySummaryText.format(Data(json.utf8), timeZone: zone)
    #expect(text == "上午主要在写代码。\n09:12–10:05  写系统集成：接入快捷指令和链接\n10:10–10:40  Safari · 文档")
    #expect(DaySummaryText.format(Data(#"{"blocks": [], "daily": []}"#.utf8)) == "这段时间没有整理好的活动")
    #expect(DaySummaryText.format(Data("not json".utf8)) == "没有读到当天的记录")
}
