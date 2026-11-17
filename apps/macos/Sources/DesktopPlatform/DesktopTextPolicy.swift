import ApplicationServices
import Foundation

/// Pure byte and role policy, testable without accessing any real application.
public enum DesktopTextPolicy {
    public static func boundedUTF16Length(requested: Int, maxBytes: Int) -> Int {
        max(0, min(requested, maxBytes / 4))
    }
    public static func prefix(_ value: String, maxBytes: Int) -> String {
        guard maxBytes > 0 else { return "" }
        var end = value.startIndex, count = 0
        while end < value.endIndex {
            let next = value.index(after: end), size = value[end..<next].utf8.count
            guard count + size <= maxBytes else { break }
            count += size; end = next
        }
        return String(value[..<end])
    }
    public static func permitsValue(role: String, subrole: String?, protected: Bool) -> Bool {
        !protected && subrole != kAXSecureTextFieldSubrole as String && (role == kAXTextAreaRole as String || role == kAXStaticTextRole as String)
    }
}
