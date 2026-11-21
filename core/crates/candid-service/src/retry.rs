//! Shared failure handling for model calls.
//!
//! Timeouts and an unreachable model are retried by the worker after a growing delay; output that is not valid
//! JSON is left to the caller, which can ask once more with [`json_repair_messages`].
use serde_json::{json, Value};

/// Backoff before retry number `attempt` (1 = after the first failed attempt): 30 s, 2 min, 10 min.
pub const RETRY_DELAYS_MS: [i64; 3] = [30_000, 120_000, 600_000];

/// Delay before the next try, or `None` when the error is not transient or retries are used up.
/// `attempt` is the job's claim count, so the first run is attempt 1.
pub fn retry_delay_ms(attempt: u64, error_code: &str) -> Option<i64> {
    if !matches!(error_code, "timeout" | "offline") {
        return None;
    }
    RETRY_DELAYS_MS.get(attempt.checked_sub(1)? as usize).copied()
}

/// The JSON object or array inside a model reply, tolerating a code fence or chatter around it.
pub fn extract_json(text: &str) -> Option<Value> {
    let text = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        return Some(v);
    }
    let start = text.find(['{', '['])?;
    let close = if text.as_bytes()[start] == b'{' { '}' } else { ']' };
    let end = text.rfind(close)?;
    serde_json::from_str(text.get(start..=end)?).ok()
}

/// Messages for the single repair attempt: the original conversation, the bad reply, and a short instruction.
pub fn json_repair_messages(messages: &Value, bad_reply: &str) -> Value {
    let mut all = messages.as_array().cloned().unwrap_or_default();
    all.push(json!({"role":"assistant","content":bad_reply.chars().take(4000).collect::<String>()}));
    all.push(json!({"role":"user","content":"上一条回复不是符合结构的 JSON。只输出符合结构的 JSON，不要任何其他文字。"}));
    json!(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_30s_2min_10min_then_gives_up() {
        assert_eq!(retry_delay_ms(1, "timeout"), Some(30_000));
        assert_eq!(retry_delay_ms(2, "offline"), Some(120_000));
        assert_eq!(retry_delay_ms(3, "timeout"), Some(600_000));
        assert_eq!(retry_delay_ms(4, "timeout"), None);
        assert_eq!(retry_delay_ms(0, "timeout"), None);
        assert_eq!(retry_delay_ms(1, "http_400"), None);
    }

    #[test]
    fn json_is_found_inside_fences_and_chatter() {
        assert_eq!(extract_json("{\"a\":1}"), Some(json!({"a":1})));
        assert_eq!(extract_json("好的：\n```json\n{\"a\":[1,2]}\n```"), Some(json!({"a":[1,2]})));
        assert_eq!(extract_json("[1]"), Some(json!([1])));
        assert_eq!(extract_json("没有 JSON"), None);
        assert_eq!(extract_json("{broken"), None);
    }

    #[test]
    fn repair_keeps_the_conversation_and_appends_two_turns() {
        let m = json_repair_messages(&json!([{"role":"user","content":"x"}]), "oops");
        assert_eq!(m.as_array().unwrap().len(), 3);
        assert_eq!(m[1]["content"], "oops");
    }
}
