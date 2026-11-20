//! The desktop ledger's record JSON (Swift `DesktopRecord`, `DesktopRecognitionResult`), read-only.
//!
//! Dates are Swift's default `JSONEncoder` form: seconds since 2001-01-01 as a double. The AX evidence attached to a
//! screenshot is re-encoded exactly as Swift's default encoder does (declaration key order, `/` escaped), because its
//! SHA-256 becomes the `derived` blob's identity and `payload.ax_evidence_ref`. Swift encodes it with `.sortedKeys`
//! (its default key order varied per process); `/` stays escaped as the Swift encoder does.

use serde::Deserialize;
use serde_json::Value;

/// Seconds between 1970-01-01 and 2001-01-01.
pub const REFERENCE_DATE_OFFSET: f64 = 978_307_200.0;

/// Swift `Migration.ms(date)` for a date decoded from its reference-date seconds.
pub fn ms_from_reference(seconds: f64) -> i64 {
    ((seconds + REFERENCE_DATE_OFFSET) * 1000.0).round() as i64
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordKind {
    Application,
    Window,
    Context,
    Screenshot,
    Marker,
    System,
    Gap,
}

impl RecordKind {
    pub const ALL: [RecordKind; 7] =
        [RecordKind::Application, RecordKind::Window, RecordKind::Context, RecordKind::Screenshot, RecordKind::Marker, RecordKind::System, RecordKind::Gap];

    pub fn raw(self) -> &'static str {
        match self {
            RecordKind::Application => "application",
            RecordKind::Window => "window",
            RecordKind::Context => "context",
            RecordKind::Screenshot => "screenshot",
            RecordKind::Marker => "marker",
            RecordKind::System => "system",
            RecordKind::Gap => "gap",
        }
    }

    pub fn event_kind(self) -> &'static str {
        match self {
            RecordKind::Application => "app.focus",
            RecordKind::Window => "window.focus",
            RecordKind::Context => "screen.text",
            RecordKind::Screenshot => "screen.shot",
            RecordKind::Marker => "marker",
            RecordKind::System => "system.state",
            RecordKind::Gap => "coverage.gap",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct AxNode {
    pub parent: Option<i64>,
    pub role: String,
    pub subrole: Option<String>,
    pub label: Option<String>,
    pub text: Option<String>,
    pub detail: Option<String>,
    pub placeholder: Option<String>,
    pub identifier: Option<String>,
    pub bounds: Option<Vec<i64>>,
    #[serde(rename = "reportedVisible")]
    pub reported_visible: Option<bool>,
    pub selected: Option<bool>,
    pub focused: Option<bool>,
    pub expanded: Option<bool>,
    pub enabled: Option<bool>,
    pub destination: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AxEvidence {
    pub nodes: Vec<AxNode>,
    pub partial: bool,
    #[serde(rename = "observedAt")]
    pub observed_at: Option<f64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSnapshot {
    pub record_evidence: Option<crate::records::Evidence>,
    pub focused_role: Option<String>,
    pub focused_label: Option<String>,
    pub selected_text: Option<String>,
    pub readable_text: Option<String>,
    pub ax_evidence: Option<AxEvidence>,
    pub text_status: Option<String>,
    pub is_protected: Option<bool>,
    pub session_active: Option<bool>,
    pub on_battery: Option<bool>,
    pub low_power: Option<bool>,
    pub thermal_state: Option<String>,
    pub idle_seconds: Option<f64>,
    pub is_composing: Option<bool>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DesktopRecord {
    pub id: String,
    /// Seconds since 2001-01-01.
    pub timestamp: f64,
    pub kind: RecordKind,
    #[serde(rename = "appName")]
    pub app_name: String,
    #[serde(rename = "bundleID")]
    pub bundle_id: String,
    #[serde(rename = "windowTitle")]
    pub window_title: String,
    pub detail: String,
    #[serde(rename = "imageHash")]
    pub image_hash: Option<String>,
    #[serde(rename = "imageWidth")]
    pub image_width: Option<i64>,
    #[serde(rename = "imageHeight")]
    pub image_height: Option<i64>,
    #[serde(rename = "parentID")]
    pub parent_id: Option<String>,
    #[serde(rename = "runID")]
    pub run_id: String,
    pub trigger: Option<String>,
    pub context: Option<ContextSnapshot>,
    #[serde(rename = "activityID")]
    pub activity_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecognitionResult {
    pub source: String,
    pub text: String,
    /// Seconds since 2001-01-01.
    pub completed_at: f64,
    #[allow(dead_code)]
    #[serde(rename = "usedOCRReference")]
    pub used_ocr_reference: bool,
    /// Line boxes; kept on `image.text` as `boxes` (see `livesync::ocr_boxes`).
    pub ocr_lines: Option<Value>,
}

/// Swift's default `JSONEncoder` string escaping (it also escapes `/`).
fn swift_default_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '/' => out.push_str("\\/"),
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

struct ObjectWriter<'a> {
    out: &'a mut String,
    first: bool,
}

impl<'a> ObjectWriter<'a> {
    fn new(out: &'a mut String) -> Self {
        out.push('{');
        ObjectWriter { out, first: true }
    }
    fn key(&mut self, k: &str) {
        if !self.first {
            self.out.push(',');
        }
        self.first = false;
        swift_default_string(k, self.out);
        self.out.push(':');
    }
    fn str(&mut self, k: &str, v: &Option<String>) {
        if let Some(v) = v {
            self.key(k);
            swift_default_string(v, self.out);
        }
    }
    fn bool(&mut self, k: &str, v: Option<bool>) {
        if let Some(v) = v {
            self.key(k);
            self.out.push_str(if v { "true" } else { "false" });
        }
    }
    fn end(self) {
        self.out.push('}');
    }
}

impl AxEvidence {
    /// The bytes Swift's `JSONEncoder` with `.sortedKeys` produces for the evidence.
    pub fn swift_encoded(&self) -> Vec<u8> {
        let mut out = String::new();
        let mut w = ObjectWriter::new(&mut out);
        w.key("nodes");
        w.out.push('[');
        for (i, n) in self.nodes.iter().enumerate() {
            if i > 0 {
                w.out.push(',');
            }
            let mut o = ObjectWriter::new(&mut *w.out);
            // keys in sorted order
            if let Some(b) = &n.bounds {
                o.key("bounds");
                o.out.push('[');
                o.out.push_str(&b.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(","));
                o.out.push(']');
            }
            o.str("destination", &n.destination);
            o.str("detail", &n.detail);
            o.bool("enabled", n.enabled);
            o.bool("expanded", n.expanded);
            o.bool("focused", n.focused);
            o.str("identifier", &n.identifier);
            o.str("label", &n.label);
            if let Some(p) = n.parent {
                o.key("parent");
                o.out.push_str(&p.to_string());
            }
            o.str("placeholder", &n.placeholder);
            o.bool("reportedVisible", n.reported_visible);
            o.key("role");
            swift_default_string(&n.role, o.out);
            o.bool("selected", n.selected);
            o.str("subrole", &n.subrole);
            o.str("text", &n.text);
            o.end();
        }
        w.out.push(']');
        if let Some(t) = self.observed_at {
            w.key("observedAt");
            w.out.push_str(&crate::json::format_double(t));
        }
        w.key("partial");
        w.out.push_str(if self.partial { "true" } else { "false" });
        w.end();
        out.into_bytes()
    }
}
