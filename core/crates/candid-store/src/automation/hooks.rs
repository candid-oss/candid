use super::*;

impl TimelineStore {
    pub fn automation_scan(&self) -> Result<()> {
        // Look up active runs by job ID; old completed runs must not be decoded on every scheduler pass.
        let rows = self.db.query("SELECT m.value FROM jobs j INDEXED BY jobs_pending JOIN store_meta m ON m.key='automation.run/1:'||j.job_id WHERE j.kind='agent.skill' AND j.state IN ('queued','running') ORDER BY j.created_ms,j.job_id", &[], |r| r.text(0))?;
        for raw in rows {
            let r: Value = serde_json::from_str(&raw)?;
            let lease = self
                .db
                .first(
                    "SELECT lease_until_ms FROM jobs WHERE job_id=?",
                    &[text(r["id"].as_str().unwrap())],
                    |r| r.opt_int(0),
                )?
                .flatten();
            let reason = if self.automation_validate(&r).is_err() {
                Some("source_or_configuration_changed")
            } else if r["state"] == "running" && lease.is_some_and(|n| n < self.clock.now_ms()) {
                Some("interrupted")
            } else {
                None
            };
            if let Some(reason) = reason {
                self.db.transaction(|| {
                    self.automation_end_inner(
                        &r,
                        if reason == "interrupted" {
                            "failed"
                        } else {
                            "cancelled"
                        },
                        Some(reason),
                        None,
                    )
                })?;
            }
        }
        let c = self.automation_config()?;
        if self.automation_quality()?["passed"] != true {
            return Ok(());
        }
        for (id, p) in c.skills.iter().filter(|(_, p)| p.enabled && p.automatic) {
            if p.trigger == "daily" {
                let zone = crate::Zone::named(&p.timezone).unwrap();
                let local = zone.local(self.clock.now_ms());
                let at = chrono::NaiveTime::parse_from_str(&p.time, "%H:%M").unwrap();
                if local.time() >= at {
                    let _ = self.automation_enqueue(
                        id,
                        &format!("daily:{id}:{}", local.date()),
                        "daily",
                        &[],
                        "",
                        "background",
                    );
                }
            } else if ["recording_sealed", "episode_ended"].contains(&p.trigger.as_str()) {
                let events=self.db.query("SELECT event_id,source_id,event_id FROM events WHERE kind='audio.recording' AND COALESCE(json_extract(payload_json,'$.recording_state'),'stopped')<>'recording' AND end_ms IS NOT NULL AND end_ms<=? AND start_ms>=? ORDER BY start_ms LIMIT 200",&[int(self.clock.now_ms()),int(self.clock.now_ms()-86400000)],|r|json!({"event_id":r.text(0),"source_id":r.text(1)}))?;
                if p.trigger == "recording_sealed" {
                    for e in events {
                        let event_id = e["event_id"].as_str().unwrap();
                        let payload = self
                            .db
                            .first(
                                "SELECT payload_json FROM events WHERE event_id=?",
                                &[text(event_id)],
                                |r| r.text(0),
                            )?
                            .unwrap_or_default();
                        let payload: Value = serde_json::from_str(&payload)?;
                        let ready = if let Some(media) = payload["hub_media_id"].as_str() {
                            self.media_item(media).is_ok_and(|m| {
                                m["processing_status"] == "ready"
                                    && m["recording_state"] != "recording"
                            })
                        } else {
                            self.db.scalar("SELECT COUNT(*) FROM event_text WHERE event_id=?",&[text(event_id)])?>0||self.db.scalar("SELECT COUNT(*) FROM events WHERE kind='speech.segment' AND parent_event_id=?",&[text(event_id)])?>0
                        };
                        if !ready {
                            continue;
                        }
                        let _ = self.automation_enqueue(
                            id,
                            &format!("recording:{id}:{}", e["event_id"].as_str().unwrap()),
                            "recording_sealed",
                            &[e],
                            "",
                            "background",
                        );
                    }
                }
                if p.trigger == "episode_ended" {
                    let eps=self.db.query("SELECT p.episode_id,e.event_id,v.source_id,v.event_id FROM episodes p JOIN episode_evidence e USING(episode_id,rev) JOIN events v USING(event_id) WHERE p.status='draft' AND p.end_ms<=? AND p.end_ms>=? GROUP BY p.episode_id ORDER BY p.end_ms LIMIT 200",&[int(self.clock.now_ms()-600000),int(self.clock.now_ms()-86400000)],|r|(r.text(0),json!({"event_id":r.text(1),"source_id":r.text(2)})))?;
                    for (ep, _) in eps {
                        let refs=self.db.query("SELECT v.event_id,v.source_id,v.event_id FROM episode_evidence e JOIN events v USING(event_id) WHERE e.episode_id=? AND e.rev=(SELECT MAX(rev) FROM episodes WHERE episode_id=?)",&[text(&ep),text(&ep)],|r|json!({"event_id":r.text(0),"source_id":r.text(1)}))?;
                        let _ = self.automation_enqueue(
                            id,
                            &format!("episode:{id}:{ep}"),
                            "episode_ended",
                            &refs,
                            "",
                            "background",
                        );
                    }
                }
            }
        }
        Ok(())
    }
}
