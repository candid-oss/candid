//! App-only proxy. Audio is resolved by media identity, never a caller-supplied path.
use crate::router::{lock, SharedStore};
use serde_json::{json, Value};
use std::time::Duration;
fn remote(p: &Value, action: &str, v: &Value, audio: Option<Vec<u8>>) -> Result<Value, String> {
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        // One recording window is several minutes of audio in one model call.
        .timeout(Duration::from_secs(if action == "window" { 600 } else { 120 }))
        .build()
        .map_err(|_| "无法连接本机模型")?;
    let base = p["base_url"]
        .as_str()
        .ok_or("缺少模型地址")?
        .trim_end_matches('/');
    let url = format!(
        "{base}/voiceprints{}",
        match action { "analyze" => "/analyze", "reconcile_recording" => "/reconcile", "window" => "/window", "stitch" => "/stitch", _ => "" }
    );
    let mut req = if action == "list" {
        client.get(url)
    } else {
        client.post(url)
    };
    if let Some(reference) = p["secret_ref"].as_str() {
        req = req.bearer_auth(crate::workflows::secret(reference).ok_or("模型凭据尚未授权")?);
    }
    if let Some(bytes) = audio {
        let mut form = reqwest::blocking::multipart::Form::new().part(
            "file",
            reqwest::blocking::multipart::Part::bytes(bytes).file_name("selection.wav"),
        );
        if action == "window" {
            form = form.text("meta", v.to_string());
        }
        req = req.multipart(form);
    } else if action != "list" {
        req = req.json(v);
    }
    let response = req.send().map_err(|_| "本机模型离线或超时，请重试")?;
    if !response.status().is_success() {
        return Err(match response.status().as_u16() {
            429 => "模型正在处理其他请求，请稍后重试",
            422 if action == "window" => "window_too_dense",
            409 => "名称已存在，请改名或选择合并",
            400 => "分析已失效或输入无效，请重新核对",
            _ => "声纹服务未完成操作，请重试",
        }
        .into());
    }
    use std::io::Read;
    let mut bytes = Vec::new();
    response
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "读取模型结果失败")?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("模型结果过大".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "模型结果格式无效".into())
}
pub fn command(store: &SharedStore, v: &Value) -> Result<Value, String> {
    let action = v["action"].as_str().unwrap_or("list");
    let p = lock(store).voiceprint_provider().map_err(|e| e.message())?;
    if action == "list" {
        reconcile(store);
        let mut out = remote(&p, action, v, None)?;
        let pending = lock(store)
            .voiceprint_cleanup_candidates()
            .map_err(|e| e.message())?
            .len();
        out["cleanup_pending"] = json!(pending);
        if pending > 0 {
            out["cleanup_message"] = json!(
                "有来源清理待完成；请保持原模型服务在线。旧服务或缺登记回执时需升级支持精准撤回。"
            );
        }
        return Ok(out);
    }
    if action == "clear_cache" {
        if v["confirmed"] != true {
            return Err("请确认清理分析缓存".into());
        }
        let entries = lock(store).voiceprint_analyses().map_err(|e| e.message())?;
        for entry in entries {
            if entry["provider"] != p {
                return Err("部分分析属于其他模型服务，请启用原服务后清理".into());
            }
            remote(
                &p,
                "forget",
                &json!({"action":"forget","analysis_id":entry["remote_id"],"confirmed":true}),
                None,
            )?;
            lock(store)
                .voiceprint_forget(
                    entry["result"]["analysis"]["analysis_id"]
                        .as_str()
                        .unwrap_or(""),
                )
                .map_err(|e| e.message())?;
        }
        return remote(&p, "list", &json!({}), None);
    }
    if action == "analyze" {
        let source = lock(store)
            .voiceprint_source(v["id"].as_str().unwrap_or(""))
            .map_err(|e| e.message())?;
        let plan = crate::audio_chunks::AudioChunks::open(std::path::Path::new(
            source["path"].as_str().unwrap(),
        ))?;
        let start = v["start_seconds"].as_f64().ok_or("请选择分析起点")?;
        let end = v["end_seconds"].as_f64().ok_or("请选择分析终点")?;
        if !start.is_finite()
            || !end.is_finite()
            || start < 0.
            || end <= start
            || end - start > 240.
            || end > plan.frames as f64 / plan.rate as f64
        {
            return Err("请选择录音内不超过240秒的区间".into());
        }
        let a = (start * plan.rate as f64).floor() as u64;
        let b = (end * plan.rate as f64).floor() as u64;
        if b <= a || (b - a) * plan.align as u64 > 32 * 1024 * 1024 {
            return Err("分析区间过大或过短".into());
        }
        let audio = plan.bytes(a, b - a)?;
        let digest = candid_store::sha256_hex(&audio);
        let mut result = remote(&p, action, v, Some(audio))?;
        let remote_id = result["analysis"]["analysis_id"]
            .as_str()
            .ok_or("缺少分析身份")?
            .to_owned();
        let analysis_id = candid_store::uuid_v4();
        let checked = (|| {
            let s = lock(store);
            s.voiceprint_check(&source).map_err(|e| e.message())?;
            if s.voiceprint_provider().map_err(|e| e.message())? != p {
                return Err("模型配置已改变".into());
            }
            if result["format"] != "candid-voiceprint-analysis/1"
                || result["analysis"]["upload_hash"] != digest
            {
                return Err("模型返回来源不匹配".into());
            }
            let offset = a as f64 / plan.rate as f64;
            result["source"] = json!({"id":source["id"],"digest":source["digest"],"start_seconds":offset,"end_seconds":b as f64/plan.rate as f64,"sample_rate":plan.rate,"start_sample":a,"end_sample":b});
            // Preserve model-relative coordinates; expose original coordinates separately for playback.
            for seg in result["analysis"]["segments"]
                .as_array_mut()
                .ok_or("缺少分析片段")?
            {
                let x = seg["start_s"].as_f64().ok_or("无效片段")?;
                let y = seg["end_s"].as_f64().ok_or("无效片段")?;
                if x < 0. || y <= x || y > (b - a) as f64 / plan.rate as f64 + 0.001 {
                    return Err("模型片段超出分析区间".into());
                }
                seg["original_start_seconds"] = json!(x + offset);
                seg["original_end_seconds"] = json!(y + offset);
            }
            result["analysis"]["analysis_id"] = json!(analysis_id);
            s.voiceprint_save(&analysis_id,&json!({"remote_id":remote_id,"format":"voiceprint-provenance/1","source":source,"provider":p,"result":result,"enrollments":[]})).map_err(|e|e.message())?;
            Ok(result.clone())
        })();
        if checked.is_err() {
            let _ = remote(
                &p,
                "forget",
                &json!({"action":"forget","analysis_id":remote_id,"confirmed":true}),
                None,
            );
        }
        return checked;
    }
    if action == "label" {
        return label(store, v);
    }
    if !["enroll", "rename", "merge", "delete", "forget"].contains(&action) {
        return Err("未知声纹操作".into());
    }
    if action != "rename" && v["confirmed"] != true {
        return Err("请先核对原音、身份与授权，并明确确认本次操作".into());
    }
    // Snapshot authority before HTTP; recheck afterward without blocking media operations on an offline model.
    let s = lock(store);
    let mut provenance = None;
    let mut outgoing = v.clone();
    if action == "enroll" || action == "forget" {
        let cached = match s.voiceprint_load(v["analysis_id"].as_str().unwrap_or("")) {
            Ok(v) => v,
            Err(_) if action == "forget" => {
                drop(s);
                return remote(&p, "list", &json!({}), None);
            }
            Err(e) => return Err(e.message()),
        };
        if action == "enroll" {
            s.voiceprint_check(&cached["source"])
                .map_err(|e| e.message())?;
        }
        if cached["provider"] != p {
            return Err("模型配置已改变，请重新分析".into());
        }
        outgoing["analysis_id"] = cached["remote_id"].clone();
        if action == "enroll" {
            provenance = Some(cached);
        }
    }
    drop(s);
    if action == "enroll" {
        require_withdrawal(&remote(&p, "list", &json!({}), None)?)?;
    }
    let out = remote(&p, action, &outgoing, None)?;
    let s = lock(store);
    if let Some(cached) = &provenance {
        if s.voiceprint_check(&cached["source"]).is_err()
            || s.voiceprint_provider().ok().as_ref() != Some(&p)
        {
            let mut audit = cached.clone();
            audit["cleanup_required"] = json!(true);
            audit["enrollments"].as_array_mut().unwrap().push(
                json!({"person":out["person"],"receipt":out["receipt"],"speaker_id":v["speaker_id"],"confirmed":true}),
            );
            s.voiceprint_enrollment(&audit).map_err(|e| e.message())?;
            drop(s);
            let _ = remote(
                &p,
                "withdraw",
                &json!({"action":"withdraw","receipt":out["receipt"],"confirmed":true}),
                None,
            );
            return Err("来源或权限已改变，登记撤回已安排；请保持模型服务在线".into());
        }
    }
    if let Some(mut cached) = provenance {
        if !out["receipt"].is_object() {
            cached["cleanup_required"] = json!(true);
        }
        cached["enrollments"].as_array_mut().unwrap().push(json!({"person":out["person"],"receipt":out["receipt"],"speaker_id":v["speaker_id"],"confirmed":true,"at":chrono::Utc::now().to_rfc3339()}));
        s.voiceprint_enrollment(&cached).map_err(|e| e.message())?;
        s.voiceprint_save(v["analysis_id"].as_str().unwrap_or(""), &cached)
            .map_err(|e| e.message())?;
    }
    if action == "enroll" && !out["receipt"].is_object() {
        return Err("模型服务未提供登记回执，请升级服务；来源清理待办已保留".into());
    }
    if action == "merge" {
        s.voiceprint_merge_provenance(
            v["person_id"].as_str().unwrap_or(""),
            v["target_id"].as_str().unwrap_or(""),
        )
        .map_err(|e| e.message())?;
    }
    if action == "forget" {
        s.voiceprint_forget(v["analysis_id"].as_str().unwrap_or(""))
            .map_err(|e| e.message())?;
    }
    if ["rename", "merge", "delete"].contains(&action) {
        // Recordings show people through entities; a failure here only delays the new name, never the library.
        let mut change = v.clone();
        change["target_name"] = out["person"]["name"].clone();
        let _ = s.voice_library_changed(action, v["person_id"].as_str().unwrap_or(""), &change);
        drop(s);
        let _ = rematch(store);
    }
    Ok(out)
}

/// Matches every stored speaker reference against the current library and keeps the results in the core, so
/// recordings recognise people named later without being transcribed again. Returns how many were matched.
pub fn rematch(store: &SharedStore) -> Result<usize, String> {
    let p = lock(store).voiceprint_provider().map_err(|e| e.message())?;
    let refs = lock(store).voice_refs().map_err(|e| e.message())?;
    let mut matched = 0;
    for chunk in refs.chunks(2000) {
        let out = remote(&p, "rematch", &json!({"action":"rematch","refs":chunk}), None)?;
        let Some(matches) = out["matches"].as_object() else { return Err("声纹服务不支持重新匹配，请更新语音服务".into()) };
        matched += matches.values().filter(|m| m["status"] == "matched").count();
        lock(store).voice_matches_save(matches).map_err(|e| e.message())?;
    }
    Ok(matched)
}

/// 「这是谁」 on a speaker in a recording. A named answer also saves the speaker's clearest voice as library
/// samples: the existing analyze → enroll path runs on a short window around that speaker's longest line.
/// When the voice service is offline the label is still saved, with `samples_saved: false`.
fn label(store: &SharedStore, v: &Value) -> Result<Value, String> {
    let mut answer = v.clone();
    answer["samples_saved"] = json!(false);
    if v["anonymous"] != true {
        match enroll_from_recording(store, v) {
            Ok(voice) => {
                answer["voice_id"] = json!(voice);
                answer["samples_saved"] = json!(true);
            }
            Err(e) => answer["samples_error"] = json!(e),
        }
    }
    lock(store).label_speaker(&answer).map_err(|e| e.message())?;
    // A new name or new samples can make other recordings recognise this person too.
    if answer["samples_saved"] == true {
        let _ = rematch(store);
    }
    let mut item = lock(store).media_item(v["id"].as_str().unwrap_or("")).map_err(|e| e.message())?;
    if let Some(e) = answer.get("samples_error") {
        item["samples_error"] = e.clone();
    }
    Ok(item)
}

fn enroll_from_recording(store: &SharedStore, v: &Value) -> Result<String, String> {
    let id = v["id"].as_str().ok_or("缺少录音")?;
    let key = v["speaker"].as_str().ok_or("缺少说话人")?;
    let item = lock(store).media_item(id).map_err(|e| e.message())?;
    let line = item["segments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["speaker"] == key)
        .max_by(|a, b| {
            let len = |s: &Value| s["end_seconds"].as_f64().unwrap_or(0.) - s["start_seconds"].as_f64().unwrap_or(0.);
            len(a).total_cmp(&len(b))
        })
        .ok_or("录音里没有这个说话人")?;
    // Logical recordings span several physical parts; analysis runs on the part that holds the line.
    let physical = line["source_segment_id"].as_str().unwrap_or(id).to_string();
    let offset = item["recording_parts"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["id"] == physical.as_str())
        .map(|p| p["sample_start"].as_f64().unwrap_or(0.) / p["sample_rate"].as_f64().unwrap_or(1.))
        .unwrap_or(0.);
    let (a, b) = (line["start_seconds"].as_f64().unwrap_or(0.) - offset, line["end_seconds"].as_f64().unwrap_or(0.) - offset);
    let source = lock(store).voiceprint_source(&physical).map_err(|e| e.message())?;
    let plan = crate::audio_chunks::AudioChunks::open(std::path::Path::new(source["path"].as_str().unwrap_or("")))?;
    let duration = plan.frames as f64 / plan.rate as f64;
    let start = (a - 2.).max(0.);
    let end = (b + 2.).max(start + 8.).min(start + 60.).min(duration);
    let analysis = command(store, &json!({"action":"analyze","id":physical,"start_seconds":start,"end_seconds":end}))?;
    let analysis_id = analysis["analysis"]["analysis_id"].as_str().unwrap_or("").to_string();
    let forget = || {
        let _ = command(store, &json!({"action":"forget","analysis_id":analysis_id,"confirmed":true}));
    };
    let mut overlap: Vec<(String, f64)> = vec![];
    for seg in analysis["analysis"]["segments"].as_array().into_iter().flatten() {
        let x = seg["original_start_seconds"].as_f64().unwrap_or(0.).max(a);
        let y = seg["original_end_seconds"].as_f64().unwrap_or(0.).min(b);
        let speaker = seg["speaker_id"].as_str().unwrap_or("").to_string();
        if y > x {
            match overlap.iter_mut().find(|(s, _)| *s == speaker) {
                Some(entry) => entry.1 += y - x,
                None => overlap.push((speaker, y - x)),
            }
        }
    }
    let Some((speaker, _)) = overlap.into_iter().max_by(|x, y| x.1.total_cmp(&y.1)) else {
        forget();
        return Err("这一段太短或有重叠，没能取到清楚的样本".into());
    };
    let mut enroll = json!({"action":"enroll","analysis_id":analysis_id,"speaker_id":speaker,"confirmed":true});
    match v["voice_id"].as_str() {
        Some(voice) => enroll["person_id"] = json!(voice),
        None => enroll["name"] = v["name"].clone(),
    }
    let out = command(store, &enroll);
    forget();
    out?["person"]["id"].as_str().map(str::to_string).ok_or_else(|| "声纹服务没有返回人物".into())
}

/// Retry cleanup on the existing worker; independent model outages never block media reads.
pub fn reconcile(store: &SharedStore) {
    // Speaker vectors of deleted recordings leave the voice service too.
    let provider = lock(store).voiceprint_provider();
    let pending = lock(store).voice_refs_to_forget(None);
    if let (Ok(p), Ok(pending)) = (provider, pending) {
        if !pending.is_empty() && remote(&p, "forget_refs", &json!({"action":"forget_refs","refs":pending}), None).is_ok() {
            let _ = lock(store).voice_refs_to_forget(Some(&pending));
        }
    }
    let (provider, pending) = {
        let s = lock(store);
        let Ok(p) = s.voiceprint_provider() else {
            return;
        };
        let Ok(v) = s.voiceprint_cleanup_candidates() else {
            return;
        };
        (p, v)
    };
    for v in pending {
        if v["provider"] != provider {
            continue;
        }
        let mut ok = true;
        if v["format"] == "voiceprint-provenance/1" {
            ok = remote(
                &provider,
                "forget",
                &json!({"action":"forget","analysis_id":v["remote_id"],"confirmed":true}),
                None,
            )
            .is_ok();
        } else {
            let capable = remote(&provider, "list", &json!({}), None)
                .and_then(|v| require_withdrawal(&v))
                .is_ok();
            if !capable {
                continue;
            }
            for e in v["enrollments"].as_array().into_iter().flatten() {
                if !e["receipt"].is_object() {
                    ok = false;
                    break;
                }
                if remote(
                    &provider,
                    "withdraw",
                    &json!({"action":"withdraw","receipt":e["receipt"],"confirmed":true}),
                    None,
                )
                .is_err()
                {
                    ok = false;
                    break;
                }
            }
        }

        if ok {
            let _ = lock(store).voiceprint_cleanup_done(&v);
        }
    }
}

fn require_withdrawal(v: &Value) -> Result<(), String> {
    if v["capabilities"]
        .as_array()
        .is_some_and(|a| a.iter().any(|x| x == "template_withdrawal"))
    {
        Ok(())
    } else {
        Err("请升级本机声纹服务以支持精准撤回模板；清理待办将保留，不删除整个人物".into())
    }
}


/// A recording's one job: the whole recording in long windows, then stitched into one transcript. The voice
/// service must offer recording windows; earlier services without them are reported, not worked around.
pub fn recording_job(store:&SharedStore,r:&Value)->Result<Value,String> {
    let windows=remote(&r["provider"],"list",&Value::Null,None)?["capabilities"].as_array()
        .is_some_and(|c|c.iter().any(|x|x=="recording_windows"));
    if !windows {return Err("voice_service_outdated".into());}
    recording_windows(store,r)
}

pub const WINDOW_SECONDS: f64 = 600.0;
pub const WINDOW_OVERLAP_SECONDS: f64 = 60.0;
/// A short tail joins the window before it instead of becoming a window of its own (at most 12 minutes).
const WINDOW_TAIL_SECONDS: f64 = 120.0;
const WINDOW_MIN_SECONDS: f64 = 120.0;

/// The next window of a recording `total` seconds long, given where the previous one ended.
pub fn next_window(previous_end: Option<f64>, total: f64, size: f64) -> Option<(f64, f64)> {
    let start = match previous_end {
        None => 0.0,
        Some(end) if end >= total - 1e-6 => return None,
        Some(end) => (end - WINDOW_OVERLAP_SECONDS).max(0.0),
    };
    let end = if total - start <= size + WINDOW_TAIL_SECONDS.min(size / 2.0) { total } else { start + size };
    Some((start, end))
}

/// The recording's audio between two times as one 16 kHz mono WAV; gaps between files are silence.
fn window_audio(files: &[(crate::audio_chunks::AudioChunks, f64)], start: f64, end: f64) -> Result<Vec<u8>, String> {
    let frames = ((end - start) * 16000.0).round() as usize;
    let mut pcm = vec![0u8; frames * 2];
    for (plan, offset) in files {
        let first = ((start - offset) * 16000.0).round().max(0.0) as u64;
        let last = (((end - offset) * 16000.0).round().max(0.0) as u64).min(plan.frames);
        if first >= last { continue; }
        let bytes = plan.bytes(first, last - first)?;
        let data = &bytes[bytes.len() - (last - first) as usize * 2..];
        let at = (((offset + first as f64 / 16000.0) - start) * 16000.0).round().max(0.0) as usize * 2;
        let n = data.len().min(pcm.len().saturating_sub(at));
        pcm[at..at + n].copy_from_slice(&data[..n]);
    }
    let mut wav = b"RIFF".to_vec();
    wav.extend((36u32 + pcm.len() as u32).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend([1, 0, 1, 0]);
    wav.extend(16000u32.to_le_bytes());
    wav.extend(32000u32.to_le_bytes());
    wav.extend([2, 0, 16, 0]);
    wav.extend(b"data");
    wav.extend((pcm.len() as u32).to_le_bytes());
    wav.extend(pcm);
    Ok(wav)
}

/// The whole recording in windows of several minutes: transcript, speakers and voiceprints from one model call per
/// window (`mode=conversation`), or text only on the small model (`mode=quick`). Each finished window is saved in
/// the job's checkpoint, so stopping the app or cancelling loses at most the window in progress, and a window the
/// model cannot fit is retried at half the length. Each window and the stitch is one entry in the call ledger, and
/// the service's `cpu_seconds` for all of them is summed into the result.
fn recording_windows(store:&SharedStore,r:&Value)->Result<Value,String> {
    let input=&r["input"];
    let job=&r["job"];
    let mut files=Vec::new();
    for file in input["files"].as_array().ok_or("missing_source")? {
        let plan=crate::audio_chunks::AudioChunks::for_voice(std::path::Path::new(file["path"].as_str().ok_or("missing_source")?))?;
        files.push((plan,file["offset"].as_f64().unwrap_or(0.)));
    }
    let total=files.iter().map(|(p,o)|o+p.frames as f64/16000.).fold(0.,f64::max);
    if total<=0. {return Err("missing_source".into());}
    let mode=r["mode"].as_str().unwrap_or("conversation");
    let identity=json!({"format":"speaker-windows/1","mode":mode,"revision":input["revision"],"config_revision":r["config_revision"],"total":total});
    let mut state=job["checkpoint"]["speaker_windows"].clone();
    if state["identity"]!=identity {state=json!({"identity":identity,"size":WINDOW_SECONDS,"windows":[]});}
    loop {
        let previous=state["windows"].as_array().and_then(|w|w.last()).and_then(|w|w["end"].as_f64());
        let size=state["size"].as_f64().unwrap_or(WINDOW_SECONDS);
        let Some((start,end))=next_window(previous,total,size) else {break};
        {
            let s=lock(store);
            s.workflow_reply_allowed(r).map_err(|e|e.message())?;
            s.job_command(&json!({"action":"heartbeat","id":job["id"],"owner":job["owner"],"attempt":job["attempt"],"lease_ms":600000})).map_err(|e|e.message())?;
            let _=s.media_job_progress(job,&json!({"stage":"transcribing","completed_seconds":start,"total_seconds":total,"cancellation_supported":true}));
        }
        let index=state["windows"].as_array().map_or(0,|w|w.len());
        let meta=json!({"scope":input["scope"],"index":index,"mode":mode});
        let audio=window_audio(&files,start,end)?;
        match ledgered(store,r,|| remote(&r["provider"],"window",&meta,Some(audio))) {
            Ok(result) => {
                state["windows"].as_array_mut().ok_or("invalid_checkpoint")?.push(json!({"start":start,"end":end,"result":result}));
            }
            Err(e) if e=="window_too_dense" && size>WINDOW_MIN_SECONDS => {state["size"]=json!((size/2.).max(WINDOW_MIN_SECONDS));}
            Err(e) => return Err(e),
        }
        let s=lock(store);
        let mut checkpoint=s.job_command(&json!({"action":"get","id":job["id"]})).map_err(|e|e.message())?["checkpoint"].clone();
        if !checkpoint.is_object() {checkpoint=json!({});}
        checkpoint["speaker_windows"]=state.clone();
        s.job_command(&json!({"action":"checkpoint","id":job["id"],"owner":job["owner"],"attempt":job["attempt"],"checkpoint":checkpoint})).map_err(|e|e.message())?;
    }
    lock(store).workflow_reply_allowed(r).map_err(|e|e.message())?;
    let entries:Vec<Value>=input["entries"].as_array().into_iter().flatten().map(|e|json!({"interval":e["interval"],"correction_key":e["correction_key"],"protected":e["protected"]})).collect();
    let _=lock(store).media_job_progress(job,&json!({"stage":"finalizing","completed_seconds":total,"total_seconds":total,"cancellation_supported":true}));
    let mut out=ledgered(store,r,|| remote(&r["provider"],"stitch",&json!({"scope":input["scope"],"revision":input["revision"],"mode":mode,"windows":state["windows"],"entries":entries}),None))?;
    let cpu:f64=state["windows"].as_array().into_iter().flatten().filter_map(|w|w["result"]["cpu_seconds"].as_f64()).sum::<f64>()+out["cpu_seconds"].as_f64().unwrap_or(0.);
    out["cpu_seconds"]=json!((cpu*1000.).round()/1000.);
    out["policy"]=input["policy"].clone();
    Ok(out)
}

/// One model call in the shared call ledger (`ai_calls`), so daily limits and usage count windows like any call.
fn ledgered(store:&SharedStore,r:&Value,call:impl FnOnce()->Result<Value,String>)->Result<Value,String> {
    let id=lock(store).workflow_call_start(r).map_err(|e|e.message())?;
    let out=call();
    let _=lock(store).workflow_call_finish(&id,if out.is_ok(){"ok"}else{"error"},out.as_ref().err().map(String::as_str),&Value::Null);
    out
}
