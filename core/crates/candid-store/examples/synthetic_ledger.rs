//! Writes a synthetic desktop ledger shaped like a real week (kind mix of the 2026-11-10 real-library counts: about
//! 7,900 records, 1,149 screenshots, 439 recognition results), for memory and latency baselines. All text is invented.
//!
//! cargo run --release -p candid-store --example synthetic_ledger -- <dir> [scale]
use candid_store::fixtures::{ledger_append, ledger_db, ledger_save_recognition, LedgerRecord};
use serde_json::json;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(args.get(1).expect("target dir"));
    let scale: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let db = ledger_db(&dir).unwrap();
    db.execute("BEGIN", &[]).unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    let start = now - 7.0 * 86400.0;
    let mix = [("application", 1040), ("window", 1162), ("context", 2723), ("screenshot", 1149), ("system", 1222), ("gap", 202), ("marker", 2)];
    let apps = ["com.example.notes", "com.example.terminal", "com.example.browser", "com.example.mail", "com.example.editor", "com.example.chat"];
    let words = ["预算", "复核", "接口", "联调", "季度", "计划", "release", "checklist", "build", "review", "synthetic", "paragraph", "meeting", "draft"];
    let total: usize = mix.iter().map(|m| (m.1 as f64 * scale) as usize).sum();
    let mut n = 0usize;
    let mut seed: u64 = 0x9e3779b97f4a7c15;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut shots = Vec::new();
    for (kind, count) in mix {
        for i in 0..((count as f64 * scale) as usize) {
            n += 1;
            let t = start + (n as f64 / total as f64) * 7.0 * 86400.0 + (rnd() % 1000) as f64 / 1000.0;
            let app = apps[(rnd() % apps.len() as u64) as usize];
            let title = format!("Synthetic window {}", rnd() % 40);
            let mut r = LedgerRecord {
                id: format!("perf-{kind}-{i}"),
                unix: t,
                kind,
                app_name: app.rsplit('.').next().unwrap().into(),
                bundle_id: app.into(),
                window_title: title,
                run_id: format!("run-{}", n / 500),
                trigger: Some("timer".into()),
                ..Default::default()
            };
            match kind {
                "context" => {
                    let paragraphs: Vec<String> = (0..(3 + rnd() % 12))
                        .map(|p| {
                            let w: Vec<&str> = (0..(8 + rnd() % 30)).map(|_| words[(rnd() % words.len() as u64) as usize]).collect();
                            format!("{} {p}", w.join(" "))
                        })
                        .collect();
                    r.context = Some(json!({"readableText": paragraphs.join("\n\n"), "textStatus": "ok", "focusedRole": "AXTextArea"}));
                }
                "screenshot" => {
                    let bytes: Vec<u8> = (0..30_000).map(|_| (rnd() & 0xff) as u8).collect();
                    let hash = candid_store::sha256_hex(&bytes);
                    let d = dir.join("blobs").join(&hash[..2]);
                    std::fs::create_dir_all(&d).unwrap();
                    std::fs::write(d.join(format!("{hash}.jpg")), &bytes).unwrap();
                    r.image_hash = Some(hash);
                    r.image_width = Some(1440);
                    r.image_height = Some(900);
                    shots.push((r.id.clone(), t));
                }
                "system" => r.detail = if i % 2 == 0 { "system did wake".into() } else { "system will sleep".into() },
                "gap" => r.detail = "recording paused".into(),
                "marker" => r.detail = "synthetic marker".into(),
                _ => {}
            }
            ledger_append(&db, &r).unwrap();
        }
    }
    for (id, t) in shots.iter().take((439.0 * scale) as usize) {
        ledger_save_recognition(&db, id, "ocr", "synthetic-ocr/1", "合成 OCR 文字 预算 review checklist", t + 5.0).unwrap();
    }
    db.execute("COMMIT", &[]).unwrap();
    db.execute("PRAGMA wal_checkpoint(TRUNCATE)", &[]).unwrap();
    println!("records {n} screenshots {}", shots.len());
}
