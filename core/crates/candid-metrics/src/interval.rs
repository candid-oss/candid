use crate::Span;

pub(crate) fn union(spans: &[Span], a: i64, b: i64) -> Vec<Span> {
    let mut s: Vec<_> = spans
        .iter()
        .map(|s| Span {
            start_ms: s.start_ms.max(a),
            end_ms: s.end_ms.min(b),
        })
        .filter(|s| s.start_ms < s.end_ms)
        .collect();
    s.sort_by_key(|s| s.start_ms);
    let mut out: Vec<Span> = Vec::new();
    for s in s {
        if let Some(last) = out.last_mut() {
            if s.start_ms <= last.end_ms {
                last.end_ms = last.end_ms.max(s.end_ms);
                continue;
            }
        }
        out.push(s);
    }
    out
}
pub(crate) fn duration(s: &[Span]) -> i64 {
    s.iter().map(|s| s.end_ms - s.start_ms).sum()
}
pub(crate) fn intersection(a: &[Span], b: &[Span]) -> i64 {
    a.iter()
        .map(|s| duration(&union(b, s.start_ms, s.end_ms)))
        .sum()
}
pub(crate) fn inside(t: i64, a: i64, b: i64) -> bool {
    t >= a && t < b
}
pub(crate) fn lines(s: &str) -> impl Iterator<Item = &str> {
    s.lines().map(str::trim).filter(|s| !s.is_empty())
}
