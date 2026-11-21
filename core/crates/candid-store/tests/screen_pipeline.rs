mod common;
use common::{screen_text, Fx};
use candid_store::testing::RowExt;
use candid_store::screen_cleanup::*;
use candid_store::*;
use serde_json::json;
use std::collections::HashSet;
fn frame(id: &str, at: i64, body: &str) -> ScreenFrame {
    ScreenFrame {
        event_id: id.into(),
        source_id: "synthetic".into(),
        revision: "v1".into(),
        start_ms: at,
        end_ms: at,
        app: "Synthetic".into(),
        window: "doc".into(),
        text: body.into(),
        field: "body".into(),
    }
}
fn enable_screen(store: &TimelineStore) {
    let mut config = store.feature_settings().unwrap();
    config["items"]["screen-episodes"]["enabled"] = json!(true);
    store.workflow_command(&json!({"action":"features","config":config})).unwrap();
}
/// A chat message and an excerpt of app `notes` (the app of `screen_text` events), both read off `src`.
fn seed_curated(fx: &Fx, src: &str, at: i64, app: &str) {
    fx.store.raw_db().script(&format!(
        "INSERT OR IGNORE INTO conversations VALUES('chat:t','chat','{app}','项目群',{at},{at},'test',{at});
         INSERT INTO messages(message_id,conversation_id,sent_ms,actor,sender,text,producer,created_ms) VALUES('m-1','chat:t',{at},'other','张三','预算改成 200 元，周五前确认',  'test',{at});
         INSERT INTO messages(message_id,conversation_id,sent_ms,actor,sender,text,producer,created_ms) VALUES('m-2','chat:t',{at2},'self',NULL,'好的，我周五前回复',  'test',{at});
         INSERT INTO message_sources VALUES('m-1','{src}');
         INSERT INTO message_sources VALUES('m-2','{src}');
         INSERT INTO excerpts(excerpt_id,key,app,title,url,start_ms,end_ms,text,producer,created_ms) VALUES('ex:1','k','{app}','预算方案',NULL,{at},{at},'{body}','test',{at});
         INSERT INTO excerpt_sources VALUES('ex:1','{src}');",
        at2 = at + 1000,
        body = "预算正文".repeat(300),
    )).unwrap();
}
fn ep_id(fx: &Fx) -> String {
    fx.store.screen_command(&json!({"action":"status"})).unwrap()["episodes"][0]["id"].as_str().unwrap().to_string()
}
fn model_role(fx: &Fx) {
    let model = json!({"version":1,"providers":[{"provider_id":"s","name":"SYNTHETIC","api":"openai-chat","base_url":"http://127.0.0.1:1/v1","destination":"on_device","enabled":true}],"models":[{"provider_id":"s","model_id":"synthetic"}],"roles":[{"role":"episode","model":{"provider_id":"s","model_id":"synthetic"}}]});
    fx.store.model_command(&json!({"action":"save","config":model})).unwrap();
}
fn ingest_block(fx: &Fx, key: &str, at: i64) {
    let a = screen_text(&format!("{key}1"), at, "org.synthetic.notes", "Synthetic doc", "RAWONLY 屏幕原文第一行\n预算是 100 元", None, None);
    let b = screen_text(&format!("{key}2"), at + 30000, "org.synthetic.notes", "Synthetic doc", "RAWONLY 屏幕原文第二行\n预算是 200 元", None, None);
    fx.store.ingest(&IngestBatch::events(&fx.source, vec![a, b])).unwrap();
}
fn claim(fx: &Fx) -> serde_json::Value {
    fx.store.job_command(&json!({"action":"claim","kind":"screen.summarize","owner":"synthetic","lease_ms":60000})).unwrap()
}

#[test]
fn activity_input_is_the_curated_layer_only_and_blocks_without_it_never_call_the_model() {
    let fx = Fx::make("activity-input");
    enable_screen(&fx.store);
    model_role(&fx);
    let at = fixtures::FIXED_NOW_MS - 3600000;
    ingest_block(&fx, "s", at);
    // No messages or excerpts yet: a rule title and summary from the statistics, nothing queued, a run refused.
    fx.store.screen_scan().unwrap();
    let id = ep_id(&fx);
    let st = fx.store.screen_command(&json!({"action":"status"})).unwrap();
    assert_eq!(st["episodes"][0]["title"], "notes · Synthetic doc");
    assert!(st["episodes"][0]["summary"].as_str().unwrap().contains("分钟"));
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM jobs WHERE kind='screen.summarize'"), 0);
    fx.store.screen_command(&json!({"action":"run","id":&id})).unwrap();
    let job = claim(&fx);
    assert!(fx.store.screen_prepare(&job).unwrap_err().message().contains("no_curated_content"));
    // With curated rows the model reads them and nothing from the raw screen text.
    let src = fx.texts("SELECT event_id FROM events WHERE kind='screen.text' ORDER BY start_ms")[0].clone();
    seed_curated(&fx, &src, at + 10000, "notes");
    // Another app's chat in the same minutes is not this block's.
    fx.store.raw_db().script(&format!("INSERT INTO conversations VALUES('chat:o','chat','other','别的群',{at},{at},'test',{at}); INSERT INTO messages(message_id,conversation_id,sent_ms,actor,sender,text,producer,created_ms) VALUES('m-x','chat:o',{at},'other','路人','OTHERAPP 无关消息','test',{at}); INSERT INTO message_sources VALUES('m-x','{src}');")).unwrap();
    fx.store.screen_command(&json!({"action":"run","id":&id})).unwrap();
    let job = claim(&fx);
    let r = fx.store.screen_prepare(&job).unwrap();
    let content = r["content"].as_str().unwrap();
    assert!(content.contains("「项目群」张三：预算改成 200 元"), "{content}");
    assert!(content.contains("我：好的"), "{content}");
    assert!(content.contains("《预算方案》"), "{content}");
    assert!(content.chars().count() < 1200, "excerpt body is truncated: {}", content.chars().count());
    assert!(!content.contains("RAWONLY") && !content.contains("OTHERAPP"), "{content}");
    assert_eq!(r["refs"]["m1"], "m-1");
    assert_eq!(r["refs"]["x1"], "ex:1");
    assert!(r["prompt"].as_str().unwrap().contains("citations"));
    // Citations must be the input's own references.
    let bad = json!({"title":"t","summary":"s","citations":["m9"]}).to_string();
    assert!(fx.store.screen_complete(&r, &json!(bad)).is_err());
    let good = json!({"title":"和张三确认预算","summary":"预算改成 200 元，周五前确认。","citations":["m1","x1"]}).to_string();
    let done = fx.store.screen_complete(&r, &json!(good)).unwrap();
    assert_eq!(done["citations"], json!(["m-1","ex:1"]));
    let st = fx.store.screen_command(&json!({"action":"status"})).unwrap();
    assert_eq!(st["episodes"][0]["title"], "和张三确认预算");
}

#[test]
fn activity_summary_is_cached_until_the_input_changes_and_edits_survive() {
    let fx = Fx::make("activity-cache");
    enable_screen(&fx.store);
    model_role(&fx);
    let at = fixtures::FIXED_NOW_MS - 3600000;
    ingest_block(&fx, "s", at);
    let src = fx.texts("SELECT event_id FROM events WHERE kind='screen.text' ORDER BY start_ms")[0].clone();
    seed_curated(&fx, &src, at + 10000, "notes");
    // Automatic work starts from the opt-in; let this block count.
    fx.store.raw_db().script(&format!("INSERT OR REPLACE INTO store_meta VALUES('screen.automatic-since/1','{at}');")).unwrap();
    fx.store.screen_scan().unwrap();
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM jobs WHERE kind='screen.summarize' AND state='queued'"), 1);
    let id = ep_id(&fx);
    let job = claim(&fx);
    let r = fx.store.screen_prepare(&job).unwrap();
    let first_job = job.clone();
    let out = json!({"title":"标题一","summary":"概括一","citations":["m1"]}).to_string();
    fx.store.screen_complete(&r, &json!(out)).unwrap();
    let revs = fx.scalar("SELECT COUNT(*) FROM episodes");
    // Same input, prompt and model: a rebuild neither queues a job nor loses the model text.
    fx.store.screen_command(&json!({"action":"rebuild"})).unwrap();
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM jobs WHERE kind='screen.summarize' AND state='queued'"), 0);
    let st = fx.store.screen_command(&json!({"action":"status"})).unwrap();
    assert_eq!(st["episodes"][0]["title"], "标题一");
    assert!(fx.scalar("SELECT COUNT(*) FROM episodes") >= revs);
    assert!(fx.store.screen_prepare(&first_job).unwrap_err().message().contains("cache_hit"));
    // A new message changes the input: a new job and a new revision.
    fx.store.raw_db().script(&format!("INSERT INTO messages(message_id,conversation_id,sent_ms,actor,sender,text,producer,created_ms) VALUES('m-3','chat:t',{},'other','张三','补充：改到周四','test',{at}); INSERT INTO message_sources VALUES('m-3','{src}');", at + 20000)).unwrap();
    fx.store.screen_scan().unwrap();
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM jobs WHERE kind='screen.summarize' AND state='queued'"), 1);
    let job = claim(&fx);
    let r = fx.store.screen_prepare(&job).unwrap();
    assert!(r["content"].as_str().unwrap().contains("改到周四"));
    // A user edit wins over every later model result and rebuild.
    fx.store.screen_command(&json!({"action":"edit","id":&id,"title":"我改的","summary":"我的概括"})).unwrap();
    assert!(fx.store.screen_complete(&r, &json!(json!({"title":"新","summary":"新","citations":[]}).to_string())).is_err());
    fx.store.screen_command(&json!({"action":"rebuild"})).unwrap();
    let st = fx.store.screen_command(&json!({"action":"status"})).unwrap();
    assert_eq!(st["episodes"][0]["title"], "我改的");
}

#[test]
fn day_lists_blocks_in_time_order_with_related_ids_and_the_daily_summary() {
    let fx = Fx::make("activity-day");
    enable_screen(&fx.store);
    let at = fixtures::FIXED_NOW_MS - 3600000;
    ingest_block(&fx, "s", at);
    let src = fx.texts("SELECT event_id FROM events WHERE kind='screen.text' ORDER BY start_ms")[0].clone();
    seed_curated(&fx, &src, at + 10000, "notes");
    fx.store.screen_scan().unwrap();
    fx.store.raw_db().script(&format!("INSERT INTO digests VALUES('day:2000-01-01',1,'day',946684800000,946771200000,'{{\"text\":\"当天回顾\"}}','draft','workflow.daily',NULL,NULL,NULL,{at});")).unwrap();
    let day = fx.store.call_tool("day", br#"{"start":"-2h"}"#, &fx.client).unwrap();
    let blocks = day["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 1, "{day}");
    assert_eq!(blocks[0]["app"], "notes");
    assert_eq!(blocks[0]["by"], "rule");
    assert_eq!(blocks[0]["conversations"], json!(["chat:t"]));
    assert_eq!(blocks[0]["excerpts"], json!(["ex:1"]));
    assert!(blocks[0]["summary"].as_str().unwrap().contains("分钟"));
    let all = fx.store.call_tool("day", br#"{"start":"1999-12-31","end":"2100-01-01"}"#, &fx.client).unwrap();
    assert_eq!(all["daily"][0]["text"], "当天回顾");
    assert!(candid_store::TOOL_NAMES.contains(&"day"));
}

#[test]
fn cleanup_keeps_numeric_changes_and_context_and_all_original_selectors() {
    let source = vec![
        frame("a", 0, "聊天甲\n好\n报价 100 元\n文档正文很长的一段文字"),
        frame(
            "b",
            10000,
            "聊天乙\n好\n报价 200 元\n文档正文很长的一段文字",
        ),
    ];
    let eps = clean(source, &HashSet::new());
    let lines = &eps[0].lines;
    assert!(lines.iter().any(|l| l.text == "报价 100 元" && l.is_new));
    assert!(lines.iter().any(|l| l.text == "报价 200 元" && l.is_new));
    assert_eq!(
        lines.iter().filter(|l| l.text == "好" && l.is_new).count(),
        2
    );
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.text == "文档正文很长的一段文字" && l.is_new)
            .count(),
        1
    );
    for f in &eps[0].frames {
        for l in lines.iter().filter(|l| l.event_id == f.event_id) {
            assert_eq!(
                f.text
                    .chars()
                    .skip(l.start)
                    .take(l.end - l.start)
                    .collect::<String>(),
                l.text
            );
        }
    }
}
#[test]
fn recurrence_and_chrome_downweight_keep_raw_and_allow_user_override() {
    let frames = (0..24)
        .map(|i| {
            frame(
                &format!("f{i}"),
                i * 10000,
                &format!("固定导航菜单\n不同内容第 {i} 条"),
            )
        })
        .collect::<Vec<_>>();
    let eps = clean(frames.clone(), &HashSet::new());
    let line = eps[0]
        .lines
        .iter()
        .find(|l| l.text == "固定导航菜单")
        .unwrap();
    assert!(line.weight < 0.5);
    let mut important = HashSet::new();
    important.insert(line.family.clone());
    let eps = clean(frames, &important);
    assert_eq!(eps[0].lines[0].weight, 1.);
    let eps = clean(
        vec![
            frame("first", 0, "重复内容很长的一段文字"),
            frame("later", 31 * 60000, "重复内容很长的一段文字"),
        ],
        &HashSet::new(),
    );
    assert_eq!(eps.len(), 2);
    assert!(eps[1].lines[0].is_new);
}
#[test]
fn ended_projection_idempotency_model_citations_and_revocation() {
    let fx = Fx::make("screen-pipeline");
    enable_screen(&fx.store);
    let at = fixtures::FIXED_NOW_MS - 3600000;
    let a = screen_text(
        "s1",
        at,
        "org.synthetic.notes",
        "Synthetic doc",
        "预算是 100 元\n今天讨论项目方案",
        None,
        None,
    );
    let b = screen_text(
        "s2",
        at + 30000,
        "org.synthetic.notes",
        "Synthetic doc",
        "预算是 200 元\n今天讨论项目方案",
        None,
        None,
    );
    fx.store
        .ingest(&IngestBatch::events(&fx.source, vec![a, b]))
        .unwrap();
    let src = fx.texts("SELECT event_id FROM events WHERE kind='screen.text' ORDER BY start_ms")[0].clone();
    seed_curated(&fx, &src, at + 10000, "notes");
    let report = fx.store.screen_scan().unwrap();
    assert_eq!(report["ended"], 1);
    let first = fx
        .store
        .screen_command(&json!({"action":"status"}))
        .unwrap();
    let id = first["episodes"][0]["id"].as_str().unwrap();
    fx.store.screen_scan().unwrap();
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM episodes"), 1);
    let model = json!({"version":1,"providers":[{"provider_id":"s","name":"SYNTHETIC","api":"openai-chat","base_url":"http://127.0.0.1:1/v1","destination":"on_device","enabled":true}],"models":[{"provider_id":"s","model_id":"synthetic"}],"roles":[{"role":"episode","model":{"provider_id":"s","model_id":"synthetic"}}]});
    fx.store
        .model_command(&json!({"action":"save","config":model}))
        .unwrap();
    fx.store.screen_scan().unwrap();
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM jobs WHERE kind='screen.summarize'"), 0, "opting in does not backfill historical summaries");
    fx.store.screen_command(&json!({"action":"run","id":id})).unwrap();
    let job=fx.store.job_command(&json!({"action":"claim","kind":"screen.summarize","owner":"synthetic","lease_ms":60000})).unwrap();
    // Claim may pick the pre-model configuration's job; obsolete jobs must not be usable.
    let prepared = fx.store.screen_prepare(&job);
    let job = if prepared.is_err() {
        fx.store
            .job_command(&json!({"action":"cancel","id":job["id"]}))
            .unwrap();
        fx.store.job_command(&json!({"action":"claim","kind":"screen.summarize","owner":"synthetic","lease_ms":60000})).unwrap()
    } else {
        job
    };
    let r = fx.store.screen_prepare(&job).unwrap();
    assert_eq!(r["response_schema"]["required"], json!(["title","summary","citations"]));
    fx.store.screen_request_rejected(&r, "http_400").unwrap();
    assert!(fx.store.screen_prepare(&job).unwrap_err().message().contains("waiting_model_request_rejected"));
    // An explicit retry clears the stop; automatic scans cannot clear it.
    fx.store.screen_command(&json!({"action":"run","id":id})).unwrap();
    assert!(fx.store.screen_prepare(&job).is_ok());
    assert!(r["content"].as_str().unwrap().contains("200 元"));
    assert!(fx
        .store
        .screen_complete(
            &r,
            &json!(
                json!({"title":"合成标题","summary":"合成摘要","citations":["m99"]})
                    .to_string()
            )
        )
        .is_err());
    let detail = fx
        .store
        .screen_command(&json!({"action":"detail","id":id}))
        .unwrap();
    fx.store
        .screen_complete(
            &r,
            &json!(
                json!({"title":"合成标题","summary":"预算从100改到200","citations":["m1"]})
                    .to_string()
            ),
        )
        .unwrap();
    assert_eq!(
        fx.store
            .screen_command(&json!({"action":"status"}))
            .unwrap()["episodes"][0]["summary"],
        "预算从100改到200"
    );
    if let Ok(dir)=std::env::var("CANDID_PROCESSING_FIXTURES") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(std::path::Path::new(&dir).join("screen-input.v1.json"),serde_json::to_vec_pretty(&detail).unwrap()).unwrap();
        let summary=fx.store.raw_db().query("SELECT value FROM store_meta WHERE key LIKE 'screen.summary/1:%'",&[],|r|r.text(0)).unwrap();
        std::fs::write(std::path::Path::new(&dir).join("screen-summary.v1.json"),&summary[0]).unwrap();
    }
    let mut p = SourcePolicy::default();
    p.agent_visible = false;
    fx.store.update_policy(&fx.source, &p).unwrap();
    assert!(fx
        .store
        .screen_command(&json!({"action":"detail","id":id}))
        .is_err());
    assert!(fx
        .store
        .screen_command(&json!({"action":"status"}))
        .unwrap()["episodes"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn thirty_query_regression_preserves_retrieval_and_projected_evidence() {
    let fx = Fx::make("screen-thirty-queries");
    enable_screen(&fx.store);
    let events = (0..30)
        .map(|i| {
            screen_text(
                &format!("case-{i}"),
                fixtures::FIXED_NOW_MS - 3600000 + i * 10000,
                "org.synthetic.notes",
                "SYNTHETIC corpus",
                &format!(
                    "固定菜单与侧栏导航\nSYNTHETIC topic{i:02} 明确记录预算为 {} 元",
                    100 + i
                ),
                None,
                None,
            )
        })
        .collect();
    fx.store
        .ingest(&IngestBatch::events(&fx.source, events))
        .unwrap();
    fx.store.screen_scan().unwrap();
    let status = fx
        .store
        .screen_command(&json!({"action":"status"}))
        .unwrap();
    let id = status["episodes"][0]["id"].as_str().unwrap();
    let detail = fx
        .store
        .screen_command(&json!({"action":"detail","id":id}))
        .unwrap();
    let lines = detail["episode"]["lines"].as_array().unwrap();
    for i in 0..30 {
        let req: SearchRequest =
            serde_json::from_value(json!({"q":format!("topic{i:02}"),"limit":10})).unwrap();
        let hits = fx.store.search(&req, &fx.client).unwrap();
        assert!(!hits.hits.is_empty());
        assert!(lines.iter().any(|l| l["text"]
            .as_str()
            .unwrap()
            .contains(&format!("topic{i:02}"))
            && l["is_new"] == true
            && l["weight"].as_f64().unwrap() >= 0.5));
    }
    assert!(lines
        .iter()
        .filter(|l| l["text"] == "固定菜单与侧栏导航")
        .all(|l| l["weight"].as_f64().unwrap() < 0.5));
    // This is a fixed synthetic regression corpus, not a release evaluation of a user's sources or a model.
    assert_eq!(fx.store.automation_quality().unwrap()["passed"], false);
    fx.store
        .screen_command(
            &json!({"action":"edit","id":id,"title":"用户改名","summary":"保留用户的摘要"}),
        )
        .unwrap();
    fx.store
        .screen_command(&json!({"action":"rebuild"}))
        .unwrap();
    let status = fx
        .store
        .screen_command(&json!({"action":"status"}))
        .unwrap();
    assert_eq!(status["episodes"][0]["title"], "用户改名");
    assert_eq!(status["episodes"][0]["summary"], "保留用户的摘要");
}

#[test]
fn code_spacing_case_and_negation_are_changes_not_ocr_duplicates() {
    let eps = clean(
        vec![
            frame(
                "a",
                0,
                "    Value = 1\n我们确认这个项目需要经过负责人批准才能开始执行",
            ),
            frame(
                "b",
                1000,
                "Value = 1\n    value = 1\n我们确认这个项目不需要经过负责人批准才能开始执行",
            ),
        ],
        &HashSet::new(),
    );
    assert!(eps[0]
        .lines
        .iter()
        .filter(|l| l.event_id == "b")
        .all(|l| l.is_new));
}

#[test]
fn screen_processing_starts_disabled_and_pausing_preserves_existing_drafts() {
    let fx = Fx::make("screen-off");
    assert_eq!(fx.store.feature_settings().unwrap()["items"]["screen-episodes"]["enabled"], false);
    fx.store.ingest(&IngestBatch::events(&fx.source, vec![screen_text("old", fixtures::FIXED_NOW_MS - 3600000, "org.synthetic.notes", "SYNTHETIC", "合成正文", None, None)])).unwrap();
    assert_eq!(fx.store.screen_scan().unwrap()["disabled"], true);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM episodes"), 0);
    enable_screen(&fx.store);
    fx.store.screen_scan().unwrap();
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM episodes WHERE status='draft'"), 1);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM jobs WHERE kind='screen.summarize'"), 0);
    let mut config = fx.store.feature_settings().unwrap();config["items"]["screen-episodes"]["enabled"] = json!(false);
    fx.store.workflow_command(&json!({"action":"features","config":config})).unwrap();
    assert_eq!(fx.store.screen_scan().unwrap()["disabled"], true);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM episodes WHERE status='draft'"), 1);
}
