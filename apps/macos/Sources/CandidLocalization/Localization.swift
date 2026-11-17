import Foundation

/// UI text is written in Chinese in the source, and that Chinese text is the key. When the app runs in English the
/// key is looked up in `Localizable.en.json` (bundled in Contents/Resources); a missing entry falls back to the key,
/// so an untranslated string shows in Chinese rather than disappearing.
///
/// Language: `CANDID_LANG` (`zh`/`en`, for tests and screenshots), then the in-app setting (`app.language`), then the
/// system's preferred languages. Changing it takes effect on the next launch.
public enum Localization {
    public enum Language: String, Sendable { case zh, en }

    public static let languageKey = "app.language"

    public static let language: Language = resolve(environment: ProcessInfo.processInfo.environment,
                                                   setting: UserDefaults.standard.string(forKey: languageKey),
                                                   preferred: Locale.preferredLanguages)

    static func resolve(environment: [String: String], setting: String?, preferred: [String]) -> Language {
        if let value = environment["CANDID_LANG"].flatMap(Language.init(rawValue:)) { return value }
        if let value = setting.flatMap(Language.init(rawValue:)) { return value }
        return preferred.first.map { $0.hasPrefix("zh") ? .zh : .en } ?? .zh
    }

    static let table: [String: String] = {
        guard language == .en else { return [:] }
        let url = ProcessInfo.processInfo.environment["CANDID_LOCALIZATION_TABLE"].map { URL(fileURLWithPath: $0) }
            ?? Bundle.main.url(forResource: "Localizable.en", withExtension: "json")
        guard let url, let data = try? Data(contentsOf: url),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: String] else { return [:] }
        return json
    }()

    /// The translated text for a key, or the key itself.
    public static func text(_ key: String) -> String { table[key] ?? key }

    /// Fills `%@` (in order) or `%1$@` (by position) placeholders; `%%` is a literal percent sign.
    public static func format(_ template: String, _ arguments: [String]) -> String {
        var result = ""
        var next = 0
        var index = template.startIndex
        while index < template.endIndex {
            let character = template[index]
            guard character == "%" else { result.append(character); index = template.index(after: index); continue }
            let rest = template[template.index(after: index)...]
            if rest.hasPrefix("%") { result.append("%"); index = template.index(index, offsetBy: 2); continue }
            if rest.hasPrefix("@") {
                result += next < arguments.count ? arguments[next] : ""
                next += 1
                index = template.index(index, offsetBy: 2)
                continue
            }
            let digits = rest.prefix { $0.isNumber }
            if !digits.isEmpty, rest.dropFirst(digits.count).hasPrefix("$@"), let position = Int(digits), position >= 1 {
                result += position <= arguments.count ? arguments[position - 1] : ""
                index = template.index(index, offsetBy: 1 + digits.count + 2)
                continue
            }
            result.append(character)
            index = template.index(after: index)
        }
        return result
    }
}

/// UI text: `L("已暂停")`.
public func L(_ key: String) -> String { Localization.text(key) }

/// UI text with values: `L("已保存 %@ 段", count)`; each value is shown as string interpolation would show it.
public func L(_ key: String, _ values: Any...) -> String {
    Localization.format(Localization.text(key), values.map { String(describing: $0) })
}
