import Foundation
import Testing
@testable import CandidLocalization

struct LocalizationTests {
    @Test func placeholdersFillInOrderOrByPosition() {
        #expect(Localization.format("%@ of %@", ["3", "10"]) == "3 of 10")
        #expect(Localization.format("%2$@/%1$@", ["2026", "11"]) == "11/2026")
        #expect(Localization.format("about %@%%", ["40"]) == "about 40%")
        #expect(Localization.format("%@ and %@", ["only one"]) == "only one and ")
        #expect(Localization.format("100%", []) == "100%")
    }

    @Test func languageComesFromTheEnvironmentThenTheSettingThenTheSystem() {
        #expect(Localization.resolve(environment: ["CANDID_LANG": "en"], setting: "zh", preferred: ["zh-Hans"]) == .en)
        #expect(Localization.resolve(environment: [:], setting: "en", preferred: ["zh-Hans"]) == .en)
        #expect(Localization.resolve(environment: [:], setting: "system", preferred: ["zh-Hans-CN"]) == .zh)
        #expect(Localization.resolve(environment: [:], setting: nil, preferred: ["en-US", "zh-Hans"]) == .en)
        #expect(Localization.resolve(environment: [:], setting: nil, preferred: []) == .zh)
    }

    @Test func theEnglishTableKeepsEveryPlaceholder() throws {
        let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("../../Resources/Localizable.en.json")
        let table = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: String] ?? [:]
        #expect(table.count > 2000)
        func placeholders(_ s: String) -> Int {
            let text = s.replacingOccurrences(of: "%%", with: "")
            let ordered = text.components(separatedBy: "%@").count - 1
            let positional = Set(text.matches(of: /%(\d+)\$@/).map { String($0.1) }).count
            return ordered + positional
        }
        for (key, value) in table { #expect(placeholders(key) == placeholders(value), "\(key)") }
    }

    @Test func untranslatedKeysFallBackToChinese() {
        #expect(L("这句不在表里") == "这句不在表里")
        #expect(L("共 %@ 条", 3) == "共 3 条")
    }
}

/// Runs only in an English process: `CANDID_LANG=en CANDID_LOCALIZATION_TABLE=Resources/Localizable.en.json`.
struct EnglishModeTests {
    @Test func englishProcessShowsEnglish() {
        guard Localization.language == .en, ProcessInfo.processInfo.environment["CANDID_LOCALIZATION_TABLE"] != nil else { return }
        #expect(L("暂停") == "Pause")
        #expect(L("已保存 · 可回听") == "Saved · playable")
        #expect(L("已处理 %@ / %@", 3, 10) == "Processed 3 / 10")
        #expect(L("%@年%@月", 2026, 11) == "11/2026")
    }
}
