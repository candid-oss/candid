//! JSON text in the shape the Swift `TimelineCoding` encoder produces: sorted keys, `/` not escaped, and doubles with
//! an integral value written without a fraction (Swift's `JSONEncoder` writes `12.0` as `12`, and its `JSONValue`
//! reads it back as an integer). Payloads, policies and scopes are stored in this form, and fingerprints compare it.

use serde::Serialize;
use serde_json::Value;

/// Canonical text of a JSON value (sorted keys, Swift number style).
pub fn canonical(v: &Value) -> String {
    let mut out = String::with_capacity(64);
    write_value(v, &mut out);
    out
}

/// Canonical text of any serializable value.
pub fn canonical_of<T: Serialize + ?Sized>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(value) => canonical(&value),
        Err(_) => "null".into(),
    }
}

/// Parses JSON text; `None` when it is not valid JSON.
pub fn parse(text: &str) -> Option<Value> {
    serde_json::from_str(text).ok()
}

/// Pretty-printed form for the command line (sorted keys, two-space indent).
pub fn pretty(v: &Value) -> String {
    let mut out = String::new();
    write_pretty(v, 0, &mut out);
    out
}

fn write_value(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => write_number(n, out),
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(k, out);
                out.push(':');
                write_value(&map[k.as_str()], out);
            }
            out.push('}');
        }
    }
}

fn write_pretty(v: &Value, indent: usize, out: &mut String) {
    let pad = |n: usize, out: &mut String| out.extend(std::iter::repeat_n(' ', n));
    match v {
        Value::Array(items) if !items.is_empty() => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                pad(indent + 2, out);
                write_pretty(item, indent + 2, out);
                if i + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            pad(indent, out);
            out.push(']');
        }
        Value::Object(map) if !map.is_empty() => {
            out.push_str("{\n");
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for (i, k) in keys.iter().enumerate() {
                pad(indent + 2, out);
                write_string(k, out);
                out.push_str(" : ");
                write_pretty(&map[k.as_str()], indent + 2, out);
                if i + 1 < keys.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            pad(indent, out);
            out.push('}');
        }
        other => write_value(other, out),
    }
}

/// Swift `Double` text: integral values below 1e16 have no fraction; others use the shortest round-trip form.
pub fn format_double(d: f64) -> String {
    if !d.is_finite() {
        return "null".into();
    }
    if d.fract() == 0.0 && d.abs() < 1e16 {
        return format!("{}", d as i64);
    }
    
    serde_json::Number::from_f64(d).map(|n| n.to_string()).unwrap_or_else(|| "null".into())
}

fn write_number(n: &serde_json::Number, out: &mut String) {
    if let Some(i) = n.as_i64() {
        out.push_str(&i.to_string());
    } else if let Some(u) = n.as_u64() {
        out.push_str(&u.to_string());
    } else if let Some(f) = n.as_f64() {
        out.push_str(&format_double(f));
    } else {
        out.push_str("null");
    }
}

pub(crate) fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Normalises a value the way Swift's `JSONValue` would hold it after decoding: integral doubles become integers.
pub fn normalise(v: Value) -> Value {
    match v {
        Value::Number(n) => {
            if n.is_f64() {
                let f = n.as_f64().unwrap_or(0.0);
                if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.2e18 {
                    return Value::from(f as i64);
                }
            }
            Value::Number(n)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(normalise).collect()),
        Value::Object(map) => Value::Object(map.into_iter().map(|(k, v)| (k, normalise(v))).collect()),
        other => other,
    }
}

/// `value[key]` as a string, like Swift's `JSONValue.stringValue`.
pub fn str_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_matches_swift_sorted_keys_style() {
        let v = json!({"b": 1, "a": [1.0, 2.5, "x/y"], "c": {"z": null, "y": true}});
        assert_eq!(canonical(&v), r#"{"a":[1,2.5,"x/y"],"b":1,"c":{"y":true,"z":null}}"#);
        assert_eq!(canonical(&json!("tab\tquote\"")), r#""tab\tquote\"""#);
    }
}
