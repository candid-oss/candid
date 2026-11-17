//! Adapter B: the user's own IMAP login through himalaya's library (email-lib / imap-client, pimalaya). Read-only
//! by construction: mailboxes are opened with EXAMINE, bodies fetched with BODY.PEEK[], and only LIST, SEARCH and
//! FETCH are issued — no STORE/COPY/MOVE/EXPUNGE/APPEND. The password comes from the caller (process memory, fed by
//! the App from the Keychain entry named by `secret_ref`) and is never written anywhere.
use super::config::{role_of, MailConfig, Role};
use super::{mid_key, mime, Known, Location, RawMail, Scan};
use email::account::config::{passwd::PasswordConfig, AccountConfig};
use email::backend::context::BackendContextBuilder;
use email::folder::FolderKind;
use email::imap::config::{ImapAuthConfig, ImapConfig};
use email::imap::ImapContextBuilder;
use email::tls::{Encryption, Tls};
use imap_client::imap_next::imap_types::{core::AString, datetime::NaiveDate, search::SearchKey, sequence::SequenceSet};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::sync::Arc;

pub const FORMAT: &str = "imap/email-lib-0.27";
/// Vanished messages looked up in other folders per pass; beyond this, deletions wait (presence unknown).
const MAX_LOOKUPS: usize = 100;
const FETCH_CHUNK: usize = 25;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct BoxState {
    uidvalidity: u32,
    /// uid → source key, for every message of this mailbox the source has seen.
    keys: BTreeMap<u32, String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct Cursor {
    v: i64,
    mailboxes: BTreeMap<String, BoxState>,
}

fn connect_error(e: impl std::fmt::Debug) -> String {
    let d = format!("{e:?}").to_lowercase();
    if d.contains("login") || d.contains("authenticat") || d.contains("credential") || d.contains("passw") {
        "imap_auth_failed".into()
    } else if d.contains("tls") || d.contains("certificate") {
        "imap_tls_failed".into()
    } else {
        "imap_unreachable".into()
    }
}

fn folder_role(kind: Option<&FolderKind>, name: &str, desc: &str) -> Role {
    let d = desc.to_lowercase();
    match kind {
        Some(FolderKind::Inbox) => Role::Inbox,
        Some(FolderKind::Sent) => Role::Sent,
        Some(FolderKind::Drafts) => Role::Drafts,
        Some(FolderKind::Trash) => Role::Trash,
        _ if d.contains("\\junk") || d.contains("\\spam") => Role::Junk,
        _ if d.contains("\\all") || d.contains("\\archive") => Role::Archive,
        _ => role_of(name),
    }
}

pub struct Login<'a> {
    pub login: &'a str,
    pub password: &'a str,
}

fn imap_config(config: &MailConfig, login: &Login) -> Result<ImapConfig, String> {
    let s = config.imap.as_ref().ok_or("imap_settings_required")?;
    let encryption = match s.tls.as_str() {
        "tls" => Encryption::Tls(Tls { provider: None, cert: None }),
        "starttls" => Encryption::StartTls(Tls { provider: None, cert: None }),
        "none" if ["127.0.0.1", "localhost", "::1"].contains(&s.host.as_str()) => Encryption::None,
        _ => return Err("insecure_transport_refused".into()),
    };
    Ok(ImapConfig {
        host: s.host.clone(),
        port: s.port,
        encryption: Some(encryption),
        login: login.login.to_string(),
        auth: ImapAuthConfig::Password(PasswordConfig(secret::Secret::new_raw(login.password))),
        extensions: None,
        watch: None,
        clients_pool_size: Some(1),
    })
}

fn set(uids: &[u32]) -> Option<SequenceSet> {
    let v: Vec<NonZeroU32> = uids.iter().filter_map(|u| NonZeroU32::new(*u)).collect();
    SequenceSet::try_from(v).ok()
}

/// Connects, authenticates and lists folders only (authorize / status probe). Nothing is read.
pub fn probe(config: &MailConfig, login: &Login) -> Result<Vec<(String, String)>, String> {
    let cfg = imap_config(config, login)?;
    runtime()?.block_on(async move {
        let account = Arc::new(AccountConfig::default());
        let ctx = BackendContextBuilder::build(ImapContextBuilder::new(account.clone(), Arc::new(cfg)).with_pool_size(1)).await.map_err(connect_error)?;
        let mut client = ctx.client().await;
        let folders = client.list_all_mailboxes(&account).await.map_err(|_| "imap_list_failed")?;
        Ok(folders.iter().map(|f| (f.name.clone(), folder_role(f.kind.as_ref(), &f.name, &f.desc).as_str().to_string())).collect())
    })
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|_| "runtime".to_string())
}

pub fn scan(config: &MailConfig, login: &Login, cursor: Option<&str>, known: &Known, limit: usize, now_ms: i64) -> Result<Scan, String> {
    let cfg = imap_config(config, login)?;
    let mut cur: Cursor = cursor.and_then(|c| serde_json::from_str(c).ok()).unwrap_or(Cursor { v: 1, ..Default::default() });
    let cutoff = config.backfill_days.map(|d| now_ms - d * 86_400_000);
    runtime()?.block_on(async move {
        let account = Arc::new(AccountConfig::default());
        let ctx = BackendContextBuilder::build(ImapContextBuilder::new(account.clone(), Arc::new(cfg)).with_pool_size(1)).await.map_err(connect_error)?;
        let mut client = ctx.client().await;
        let folders = client.list_all_mailboxes(&account).await.map_err(|_| "imap_list_failed")?;
        let mut scan = Scan { format: FORMAT.into(), ..Default::default() };
        let mut present: Vec<Location> = Vec::new();
        let mut remaining = limit;
        let mut seen_new: BTreeSet<String> = BTreeSet::new();
        let mut next = Cursor { v: 1, mailboxes: BTreeMap::new() };
        let mut others: Vec<String> = Vec::new();
        for f in folders.iter() {
            let role = folder_role(f.kind.as_ref(), &f.name, &f.desc);
            if !config.mailboxes.imports(&f.name, role) {
                if !role.excluded() {
                    others.push(f.name.clone());
                }
                continue;
            }
            let encoded = utf7_imap::encode_utf7_imap(f.name.clone());
            let data = client.examine_mailbox(&encoded).await.map_err(|_| "imap_examine_failed")?;
            let validity = data.uid_validity.map(|v| v.get()).unwrap_or(0);
            let mut state = cur.mailboxes.remove(&f.name).unwrap_or_default();
            if state.uidvalidity != validity {
                state = BoxState { uidvalidity: validity, keys: BTreeMap::new() };
            }
            let all: BTreeSet<u32> = client.search_uids([SearchKey::All]).await.map_err(|_| "imap_search_failed")?.iter().map(|u| u.get()).collect();
            let recent: BTreeSet<u32> = match cutoff {
                Some(c) => {
                    let day = chrono::DateTime::from_timestamp_millis(c).map(|d| d.date_naive()).ok_or("imap_search_failed")?;
                    client.search_uids([SearchKey::Since(NaiveDate::unvalidated(day))]).await.map_err(|_| "imap_search_failed")?.iter().map(|u| u.get()).collect()
                }
                None => all.clone(),
            };
            state.keys.retain(|uid, _| all.contains(uid));
            for key in state.keys.values() {
                present.push(Location { key: key.clone(), mailbox: f.name.clone() });
            }
            let mut wanted: Vec<u32> = recent.iter().filter(|u| all.contains(u) && !state.keys.contains_key(u)).cloned().collect();
            if wanted.len() > remaining {
                wanted.truncate(remaining);
                scan.more = true;
            }
            for chunk in wanted.chunks(FETCH_CHUNK) {
                let mut fetched: Vec<(u32, Vec<u8>)> = Vec::new();
                let msgs = client.peek_messages(set(chunk).ok_or("imap_fetch_failed")?).await.map_err(|_| "imap_fetch_failed")?;
                let list = msgs.to_vec();
                if list.len() == chunk.len() {
                    for (uid, m) in chunk.iter().zip(list) {
                        fetched.push((*uid, m.raw().map(|r| r.to_vec()).unwrap_or_default()));
                    }
                } else {
                    // A message vanished meanwhile: fetch one by one so uids and bodies stay aligned.
                    for uid in chunk {
                        let one = client.peek_messages(set(&[*uid]).ok_or("imap_fetch_failed")?).await.map_err(|_| "imap_fetch_failed")?;
                        if let Some(m) = one.first() {
                            fetched.push((*uid, m.raw().map(|r| r.to_vec()).unwrap_or_default()));
                        }
                    }
                }
                for (uid, bytes) in fetched {
                    let key = mime::message_id(&bytes).map(|id| mid_key(&id)).unwrap_or_else(|| format!("imap:{}:{}:{uid}", f.name, validity));
                    state.keys.insert(uid, key.clone());
                    present.push(Location { key: key.clone(), mailbox: f.name.clone() });
                    remaining = remaining.saturating_sub(1);
                    if known.contains_key(&key) {
                        continue;
                    }
                    let first = seen_new.insert(key.clone());
                    scan.new.push(RawMail { key, account: None, mailbox: f.name.clone(), bytes: if first { Some(bytes) } else { None }, partial: false, received_ms: None, index: Default::default() });
                }
            }
            next.mailboxes.insert(f.name.clone(), state);
        }
        // Stored messages no longer in the imported mailboxes may have been archived: look them up by Message-ID.
        let here: BTreeSet<&String> = present.iter().map(|l| &l.key).collect();
        let vanished: Vec<String> = known.keys().filter(|k| !here.contains(k)).cloned().collect();
        let mut complete = true;
        if !vanished.is_empty() {
            let searchable: Vec<&String> = vanished.iter().filter(|k| k.starts_with("mid:")).collect();
            if searchable.len() > MAX_LOOKUPS {
                complete = false;
                scan.issues.push("presence_limited".into());
            } else if !searchable.is_empty() {
                let mut extra = Vec::new();
                for name in &others {
                    client.examine_mailbox(utf7_imap::encode_utf7_imap(name.clone())).await.map_err(|_| "imap_examine_failed")?;
                    for key in &searchable {
                        let id = format!("<{}>", &key[4..]);
                        let (Ok(h), Ok(v)) = (AString::try_from("Message-ID"), AString::try_from(id)) else { continue };
                        let hits = client.search_uids([SearchKey::Header(h, v)]).await.map_err(|_| "imap_search_failed")?;
                        if !hits.is_empty() {
                            extra.push(Location { key: (*key).clone(), mailbox: name.clone() });
                        }
                    }
                }
                present.extend(extra);
            }
        }
        // Mailboxes that disappeared drop out of the cursor; their messages count as gone unless found above.
        scan.present = if complete { Some(present) } else { None };
        scan.cursor = serde_json::to_string(&next).unwrap();
        Ok(scan)
    })
}
