//! Immutable, source-attached presentation corrections. No evidence is copied or rewritten.
use crate::query::core::ClientContext;
use crate::{
    db::{int, text, RowExt},
    Result, TimelineError, TimelineStore,
};
use serde_json::{json, Value};

impl TimelineStore {
    fn annotation_refs_allowed(&self, refs: &Value, ctx: &ClientContext) -> Result<bool> {
        let Some(refs) = refs.as_array() else {
            return Ok(false);
        };
        if refs.is_empty() || refs.len() > 1000 || !ctx.scope.text || !ctx.scope.derived {
            return Ok(false);
        }
        let f = self.scope_filter(ctx, "e");
        let params = [vec![text(serde_json::to_string(refs)?)], f.params.clone()].concat();
        let count = self.db.scalar(&format!(
            "SELECT COUNT(*) FROM json_each(?) ref JOIN events e ON e.event_id=json_extract(ref.value,'$.event_id') AND e.source_id=json_extract(ref.value,'$.source_id') JOIN sources s USING(source_id)
             WHERE e.sensitivity<>'protected' AND s.status<>'revoked' AND json_extract(s.policy_json,'$.store_text')=1
             AND NOT EXISTS (SELECT 1 FROM json_each(s.policy_json,'$.exclude.apps') x WHERE x.value=e.app_bundle_id)
             AND NOT EXISTS (SELECT 1 FROM json_each(s.policy_json,'$.exclude.threads') x WHERE x.value=e.thread_id)
             AND NOT EXISTS (SELECT 1 FROM json_each(s.policy_json,'$.exclude.patterns')) AND ({})", f.sql()), &params)?;
        Ok(count == refs.len() as i64)
    }
    pub(crate) fn visible_annotation(
        &self,
        event: &str,
        ctx: &ClientContext,
    ) -> Result<Option<Value>> {
        let row=self.db.first("SELECT value_json FROM corrections WHERE target_type='event_label' AND target_id=? AND json_extract(value_json,'$.format')='candid-annotation/1' ORDER BY rowid DESC LIMIT 1",&[text(event)],|r|r.text(0))?;
        let Some(row) = row else { return Ok(None) };
        let v: Value = serde_json::from_str(&row)?;
        Ok(self.annotation_refs_allowed(&v["refs"], ctx)?.then_some(v))
    }
    /// The fragments (screen kinds only) behind `ids`, sorted and deduplicated, if every one is in `ctx`'s scope.
    pub(crate) fn annotation_targets(&self, ids: Vec<String>, ctx: &ClientContext) -> Result<(Vec<String>, Vec<Value>)> {
        if ids.is_empty() || ids.len() > 1000 {
            return Err(TimelineError::InvalidInput("请选择有效片段".into()));
        }
        let mut ids = ids;
        ids.sort();
        ids.dedup();
        let mut refs = vec![];
        for id in &ids {
            let source=self.db.first("SELECT source_id FROM events WHERE event_id=? AND kind IN ('app.focus','window.focus','screen.text','screen.shot')",&[text(id)],|r|r.text(0))?.ok_or_else(||TimelineError::NotFound("片段".into()))?;
            refs.push(json!({"event_id":id,"source_id":source}));
        }
        if !self.annotation_refs_allowed(&json!(refs), ctx)? {
            return Err(TimelineError::Denied(
                "annotation_source_unavailable".into(),
            ));
        }
        Ok((ids, refs))
    }
    /// Appends one annotation revision (a `relabel` correction per fragment). `by` marks who wrote it when that is
    /// not the user at the App (`{"client","run_id"}` for the attached agent); the App's own saves stay unmarked.
    pub(crate) fn annotation_write(&self, ids: &[String], refs: &[Value], current: Option<&Value>, title: &Value, notes: &Value, request: &str, by: Option<&Value>) -> Result<Value> {
        for field in [title, notes] {
            if !field.is_null() && field.as_str().is_none_or(|s| s.len() > 16000) {
                return Err(TimelineError::InvalidInput("标题或备注过长".into()));
            }
        }
        let mut value = json!({"format":"candid-annotation/1","request_id":request,"refs":refs,"title":title,"notes":notes,"previous":current.map(|c|c["request_id"].clone()),"created_ms":self.clock.now_ms()});
        if let Some(by) = by {
            value["by"] = by.clone();
        }
        self.db.transaction(|| {
            for id in ids {
                let key = format!("annotation:{request}:{id}");
                if let Some(old) = self.db.first(
                    "SELECT value_json FROM corrections WHERE correction_id=?",
                    &[text(&key)],
                    |r| r.text(0),
                )? {
                    let old: Value = serde_json::from_str(&old)?;
                    if old["refs"] != value["refs"]
                        || old["title"] != value["title"]
                        || old["notes"] != value["notes"]
                    {
                        return Err(TimelineError::InvalidInput("保存身份冲突".into()));
                    }
                } else {
                    self.db.execute(
                        "INSERT INTO corrections VALUES(?,'event_label',?,'relabel',?,?)",
                        &[
                            text(key),
                            text(id),
                            text(value.to_string()),
                            int(self.clock.now_ms()),
                        ],
                    )?;
                }
            }
            Ok(json!({"saved":true,"annotation":value}))
        })
    }
    pub fn annotation_command(&self, v: &Value, client: &str) -> Result<Value> {
        let (ctx, revoked) = self
            .load_client(client)?
            .ok_or_else(|| TimelineError::UnknownClient(client.into()))?;
        if revoked {
            return Err(TimelineError::Denied("revoked".into()));
        }
        let action = v["action"].as_str().unwrap_or("");
        let ids: Vec<String> = serde_json::from_value(v["event_ids"].clone())?;
        let (ids, refs) = self.annotation_targets(ids, &ctx)?;
        let current = self.visible_annotation(&ids[0], &ctx)?;
        if action == "annotation_get" {
            return Ok(json!({"annotation":current,"event_ids":ids}));
        }
        if self.db.scalar(
            "SELECT COUNT(*) FROM clients WHERE client_id=? AND transport='internal'",
            &[text(client)],
        )? != 1
        {
            return Err(TimelineError::Denied("internal_only".into()));
        }
        if action != "annotation_save" {
            return Err(TimelineError::InvalidInput("未知编辑操作".into()));
        }
        let request = v["request_id"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() < 128)
            .ok_or_else(|| TimelineError::InvalidInput("缺少保存身份".into()))?;
        self.annotation_write(&ids, &refs, current.as_ref(), &v["title"], &v["notes"], request, None)
    }
}
