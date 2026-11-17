import Foundation

enum ISOTime {
    /// Parses RFC 3339 / ISO 8601 (`2026-11-10T02:15:00.123Z`, `+08:00`). A missing zone is rejected: guessing it would
    /// silently shift events. Returns epoch milliseconds and the offset written in the text (nil for `Z`/UTC).
    static func parse(_ s: String) -> (ms: Int64, offsetMin: Int?)? {
        let u = Array(s.utf8)
        func num(_ from: Int, _ n: Int) -> Int? {
            guard from + n <= u.count else { return nil }
            var v = 0
            for i in from..<from + n { guard u[i] >= 48 && u[i] <= 57 else { return nil }; v = v * 10 + Int(u[i] - 48) }
            return v
        }
        guard u.count >= 20, let y = num(0, 4), u[4] == 45, let mo = num(5, 2), u[7] == 45, let d = num(8, 2),
              u[10] == 84 || u[10] == 116 || u[10] == 32, let h = num(11, 2), u[13] == 58, let mi = num(14, 2),
              u[16] == 58, let sec = num(17, 2), (1...12).contains(mo), (1...31).contains(d), h < 24, mi < 60, sec < 61
        else { return nil }
        var i = 19
        var frac: Int64 = 0
        if i < u.count, u[i] == 46 {
            i += 1
            var digits = 0, v: Int64 = 0
            while i < u.count, u[i] >= 48, u[i] <= 57 {
                if digits < 3 { v = v * 10 + Int64(u[i] - 48); digits += 1 }
                i += 1
            }
            guard digits > 0 else { return nil }
            while digits < 3 { v *= 10; digits += 1 }
            frac = v
        }
        var offset: Int? = nil
        guard i < u.count else { return nil }
        if u[i] == 90 || u[i] == 122 { i += 1 }
        else if u[i] == 43 || u[i] == 45 {
            let sign = u[i] == 45 ? -1 : 1
            guard let oh = num(i + 1, 2) else { return nil }
            var om = 0
            if i + 3 < u.count, u[i + 3] == 58 { guard let m = num(i + 4, 2) else { return nil }; om = m; i += 6 }
            else if let m = num(i + 3, 2) { om = m; i += 5 } else { i += 3 }
            offset = sign * (oh * 60 + om)
        } else { return nil }
        guard i == u.count else { return nil }
        let days = daysFromCivil(y, mo, d)
        let secs = days * 86400 + Int64(h * 3600 + mi * 60 + sec) - Int64((offset ?? 0) * 60)
        return (secs * 1000 + frac, offset)
    }

    // Howard Hinnant's days_from_civil.
    private static func daysFromCivil(_ y0: Int, _ m: Int, _ d: Int) -> Int64 {
        let y = Int64(m <= 2 ? y0 - 1 : y0)
        let era = (y >= 0 ? y : y - 399) / 400
        let yoe = y - era * 400
        let doy = Int64((153 * (m + (m > 2 ? -3 : 9)) + 2) / 5 + d - 1)
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy
        return era * 146097 + doe - 719468
    }
}

extension TimeZone {
    func offsetMinutes(atMs ms: Int64) -> Int {
        secondsFromGMT(for: Date(timeIntervalSince1970: Double(ms) / 1000)) / 60
    }
}

func epochMs(_ d: Date) -> Int64 { Int64((d.timeIntervalSince1970 * 1000).rounded(.down)) }
