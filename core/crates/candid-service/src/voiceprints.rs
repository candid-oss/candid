//! App-only proxy. Audio is resolved by media identity, never a caller-supplied path.
use crate::router::{lock, SharedStore};
use serde_json::{json, Value};
use std::time::Duration;
fn remote(p: &Value, action: &str, v: &Value, audio: Option<Vec<u8>>) -> Result<Value, String> {
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|_| "无法连接本机模型")?;
    let base = p["base_url"]
        .as_str()
        .ok_or("缺少模型地址")?
        .trim_end_matches('/');
    let url = format!(
        "{base}/voiceprints{}",
        if action == "analyze" { "/analyze" } else if action == "reconcile_recording" {"/reconcile"} else { "" }
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
        req = req.multipart(reqwest::blocking::multipart::Form::new().part(
            "file",
            reqwest::blocking::multipart::Part::bytes(bytes).file_name("selection.wav"),
        ));
    } else if action != "list" {
        req = req.json(v);
    }
    let response = req.send().map_err(|_| "本机模型离线或超时，请重试")?;
    if !response.status().is_success() {
        return Err(match response.status().as_u16() {
            429 => "模型正在处理其他请求，请稍后重试",
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


/// The model service sees bounded clean snippets and opaque source groups, never paths or transcript text.
pub fn reconcile_recording(r:&Value)->Result<Value,String> {
    use base64::Engine;
    let mut input=r["input"].clone();
    let files=input["files"].as_array().ok_or("missing_source")?;
    let mut plans=std::collections::HashMap::new();
    for file in files {
        let plan=crate::audio_chunks::AudioChunks::for_voice(std::path::Path::new(file["path"].as_str().ok_or("missing_source")?))?;
        plans.insert(file["id"].as_str().unwrap_or("").to_string(),(plan,file["offset"].as_f64().unwrap_or(0.)));
    }
    let entries=input["entries"].as_array().ok_or("invalid_output")?.clone();
    let mut selected:Vec<usize>=(0..entries.len()).collect();
    selected.sort_by(|a,b| {let duration=|i:usize|entries[i]["interval"][1].as_f64().unwrap_or(0.)-entries[i]["interval"][0].as_f64().unwrap_or(0.);duration(*b).total_cmp(&duration(*a))});
    let mut budget=0usize;
    for i in selected.into_iter().take(512) {
        let e=&entries[i];
        let a=e["interval"][0].as_f64().ok_or("invalid_output")?;
        let b=e["interval"][1].as_f64().ok_or("invalid_output")?;
        let mut pieces=vec![(a,b)];
        for other in &entries {
            if other["source_group"]==e["source_group"] {continue;}
            let x=other["interval"][0].as_f64().unwrap_or(0.);let y=other["interval"][1].as_f64().unwrap_or(0.);
            pieces=pieces.into_iter().flat_map(|(a,b)|if y<=a||x>=b{vec![(a,b)]}else{let mut v=Vec::new();if a<x{v.push((a,x));}if y<b{v.push((y,b));}v}).collect();
        }
        let Some((a,b))=pieces.into_iter().max_by(|a,b|(a.1-a.0).total_cmp(&(b.1-b.0))) else{continue};
        if b-a<1.5 {continue;}
        let file=e["source_segment_id"].as_str().unwrap_or(input["scope"].as_str().unwrap_or(""));
        let (plan,offset)=plans.get(file).ok_or("missing_source")?;
        let start=((a-offset).max(0.)*plan.rate as f64).round() as u64;
        let count=((b-a).min(6.)*plan.rate as f64).round() as u64;
        if start>=plan.frames {continue;}
        let bytes=plan.bytes(start,count.min(plan.frames-start))?;
        budget+=bytes.len();if budget>24*1024*1024 {break;}
        input["entries"][i]["sample"]=json!(base64::engine::general_purpose::STANDARD.encode(bytes));
    }
    let mut pooled=serde_json::Map::new();
    let groups:std::collections::BTreeSet<&str>=entries.iter().filter_map(|e|e["source_group"].as_str()).collect();
    for group in groups {
        if input["entries"].as_array().unwrap().iter().any(|e|e["source_group"]==group&&e["sample"].is_string()) {continue;}
        let mut pcm=Vec::new();
        for e in &entries {
            if e["source_group"]!=group {continue;}
            let a=e["interval"][0].as_f64().unwrap_or(0.);let b=e["interval"][1].as_f64().unwrap_or(0.);
            if entries.iter().any(|other|other["source_group"]!=group && other["interval"][0].as_f64().unwrap_or(0.)<b && other["interval"][1].as_f64().unwrap_or(0.)>a) {continue;}
            let file=e["source_segment_id"].as_str().unwrap_or(input["scope"].as_str().unwrap_or(""));
            let (plan,offset)=plans.get(file).ok_or("missing_source")?;
            let start=((a-offset).max(0.)*16000.).round() as u64;
            let count=(((b-a)*16000.).round() as u64).min(128000u64.saturating_sub(pcm.len() as u64/2));
            if start>=plan.frames || count==0 {continue;}
            let count=count.min(plan.frames-start);let bytes=plan.bytes(start,count)?;
            pcm.extend_from_slice(&bytes[bytes.len()-count as usize*2..]);
            if pcm.len()>=256000 {break;}
        }
        if pcm.len()<48000 {continue;}
        budget+=pcm.len();if budget>24*1024*1024 {break;}
        let mut wav=b"RIFF".to_vec();wav.extend((36u32+pcm.len() as u32).to_le_bytes());wav.extend(b"WAVEfmt ");wav.extend(16u32.to_le_bytes());wav.extend([1,0,1,0]);wav.extend(16000u32.to_le_bytes());wav.extend(32000u32.to_le_bytes());wav.extend([2,0,16,0]);wav.extend(b"data");wav.extend((pcm.len() as u32).to_le_bytes());wav.extend(pcm);
        pooled.insert(group.to_string(),json!(base64::engine::general_purpose::STANDARD.encode(wav)));
    }
    input["group_samples"]=Value::Object(pooled);
    input.as_object_mut().unwrap().remove("files");
    remote(&r["provider"],"reconcile_recording",&input,None)
}
