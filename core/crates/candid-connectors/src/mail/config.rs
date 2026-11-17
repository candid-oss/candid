//! Per-source mail configuration (`sources.config_json`) and the user-editable level rules (agent-platform-v1 §2.0.2).
//! The configuration never holds a secret; the IMAP password is reached through `sources.secret_ref` only.
use serde::{Deserialize, Serialize};

pub const CONFIG_VERSION: i64 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Not stored at all.
    Ignore,
    /// Sender, subject and time only; no body.
    Metadata,
    /// Body stored and searchable, no extraction.
    Normal,
    /// Candidate for extraction and summaries.
    Important,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Ignore => "ignore",
            Level::Metadata => "metadata",
            Level::Normal => "normal",
            Level::Important => "important",
        }
    }
}

/// All set fields must match (AND). Matching is case-insensitive; the message's own text never changes a level.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Condition {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Sender domain, including subdomains.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// Any To/Cc recipient.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_contains: Option<String>,
    /// Has `List-Unsubscribe` / `List-Id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list: Option<bool>,
    /// Mailbox name or role (`role:inbox`, `role:sent`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mailbox: Option<String>,
}

impl Condition {
    fn is_empty(&self) -> bool {
        *self == Condition::default()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub when: Condition,
    pub level: Level,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MailboxScope {
    /// Mailbox names or roles to import from. Default: inbox and sent.
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

impl Default for MailboxScope {
    fn default() -> Self {
        MailboxScope { include: vec!["role:inbox".into(), "role:sent".into()], exclude: vec![] }
    }
}

/// Optional "AI judges importance" (default off). Only a flag/hook: the connector marks undecided messages as
/// pending; no model is called here and the result may only move between normal and important.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AiJudge {
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ImapSettings {
    pub host: String,
    pub port: u16,
    /// `tls` (default), `starttls`; `none` is accepted only for a loopback host (tests, local bridges).
    pub tls: String,
}

impl Default for ImapSettings {
    fn default() -> Self {
        ImapSettings { host: String::new(), port: 993, tls: "tls".into() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MailConfig {
    pub version: i64,
    /// Mail.app account identifiers (the host part of its mailbox URLs) to read; empty means all accounts.
    pub accounts: Vec<String>,
    /// The user's own addresses, for direction; matched in memory, stored only as salted hashes.
    pub self_addresses: Vec<String>,
    pub mailboxes: MailboxScope,
    /// First import goes back this many days; `null` means everything the adapter holds.
    pub backfill_days: Option<i64>,
    /// New messages read per sync pass (the rest follows on the next pass).
    pub batch_limit: usize,
    pub rules: Vec<Rule>,
    /// Built-in rule "bulk": list/bulk, automated and no-reply senders → metadata only.
    pub builtin_rules: bool,
    /// Built-in rule "replied": threads you replied to and mail from people you have written to → normal, important candidate.
    pub builtin_replied: bool,
    /// Import runs only once the user confirmed the scope and preview. Sources stored before this field existed count as confirmed.
    pub import_confirmed: bool,
    pub ai_judge: AiJudge,
    /// Level of mail no rule decides: metadata only unless the user picks another (user decision 2026-11-13).
    pub default_level: Level,
    /// Mail.app data root override (default `~/Library/Mail`); tests point it at synthetic data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mail_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imap: Option<ImapSettings>,
}

impl Default for MailConfig {
    fn default() -> Self {
        MailConfig {
            version: CONFIG_VERSION,
            accounts: vec![],
            self_addresses: vec![],
            mailboxes: MailboxScope::default(),
            backfill_days: Some(90),
            batch_limit: 200,
            rules: vec![],
            builtin_rules: true,
            builtin_replied: true,
            import_confirmed: true,
            ai_judge: AiJudge::default(),
            default_level: Level::Metadata,
            mail_root: None,
            imap: None,
        }
    }
}

impl MailConfig {
    pub fn validate(&self, kind: &str) -> Result<(), String> {
        if self.version != CONFIG_VERSION {
            return Err("unsupported_config_version".into());
        }
        if self.rules.len() > 200 || self.rules.iter().any(|r| r.when.is_empty()) {
            return Err("invalid_rules".into());
        }
        if self.batch_limit == 0 || self.batch_limit > 2000 || self.backfill_days.is_some_and(|d| d < 1) {
            return Err("invalid_limits".into());
        }
        if self.accounts.len() > 64 || self.self_addresses.len() > 64 || self.mailboxes.include.len() + self.mailboxes.exclude.len() > 256 {
            return Err("invalid_scope".into());
        }
        match kind {
            "mail.imap" => {
                let s = self.imap.as_ref().ok_or("imap_settings_required")?;
                if s.host.trim().is_empty() || s.port == 0 {
                    return Err("imap_settings_required".into());
                }
                let loopback = ["127.0.0.1", "localhost", "::1"].contains(&s.host.as_str());
                match s.tls.as_str() {
                    "tls" | "starttls" => {}
                    "none" if loopback => {}
                    _ => return Err("insecure_transport_refused".into()),
                }
                if self.mail_root.is_some() {
                    return Err("invalid_scope".into());
                }
            }
            "mail.apple" => {
                if self.imap.is_some() {
                    return Err("invalid_scope".into());
                }
            }
            _ => return Err("unsupported_kind".into()),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Inbox,
    Sent,
    Drafts,
    Trash,
    Junk,
    Archive,
    Other,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Inbox => "inbox",
            Role::Sent => "sent",
            Role::Drafts => "drafts",
            Role::Trash => "trash",
            Role::Junk => "junk",
            Role::Archive => "archive",
            Role::Other => "other",
        }
    }
    /// Junk, trash and drafts are never read (§2.0.2); a message only left there counts as deleted.
    pub fn excluded(self) -> bool {
        matches!(self, Role::Trash | Role::Junk | Role::Drafts)
    }
}

/// Role from a mailbox name (last path component), covering common server and client spellings.
pub fn role_of(name: &str) -> Role {
    let last = name.rsplit(['/', '.']).next().unwrap_or(name).trim().to_lowercase();
    let full = name.trim().to_lowercase();
    match last.as_str() {
        "inbox" | "收件箱" => Role::Inbox,
        "sent" | "sent messages" | "sent mail" | "sent items" | "已发送" | "已发送邮件" | "寄件備份" => Role::Sent,
        "drafts" | "draft" | "草稿" | "草稿箱" => Role::Drafts,
        "trash" | "deleted messages" | "deleted items" | "bin" | "已删除" | "已删除邮件" | "废纸篓" | "垃圾桶" => Role::Trash,
        "junk" | "spam" | "junk e-mail" | "junk email" | "垃圾邮件" => Role::Junk,
        "archive" | "all mail" | "归档" | "存档" => Role::Archive,
        _ if full == "inbox" => Role::Inbox,
        _ => Role::Other,
    }
}

fn matches_mailbox(pattern: &str, mailbox: &str, role: Role) -> bool {
    match pattern.strip_prefix("role:") {
        Some(r) => r.eq_ignore_ascii_case(role.as_str()),
        None => pattern.eq_ignore_ascii_case(mailbox),
    }
}

impl MailboxScope {
    /// Whether new messages are imported from this mailbox.
    pub fn imports(&self, mailbox: &str, role: Role) -> bool {
        !role.excluded()
            && self.include.iter().any(|p| matches_mailbox(p, mailbox, role))
            && !self.exclude.iter().any(|p| matches_mailbox(p, mailbox, role))
    }
}

/// What a message offers to the level decision. Addresses are lower-case and only held in memory.
pub struct Facts<'a> {
    pub from: Option<&'a str>,
    pub recipients: Vec<&'a str>,
    pub subject: &'a str,
    pub list: bool,
    pub automated: bool,
    pub mailbox: &'a str,
    pub role: Role,
    /// A message of this thread was sent by the user, or the sender is someone the user has written to.
    pub replied: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Decision {
    pub level: Level,
    /// `rule:<n>`, `builtin:replied`, `builtin:list`, `builtin:automated`, `builtin:noreply` or `default`.
    pub basis: String,
    /// Listed as a candidate for the important level (the level itself stays normal).
    pub candidate: bool,
    /// Undecided by rules and the optional AI judge is on: a later step may move normal ↔ important.
    pub ai_pending: bool,
}

fn domain_match(addr: &str, domain: &str) -> bool {
    let d = domain.trim().trim_start_matches('@').to_lowercase();
    addr.rsplit_once('@').is_some_and(|(_, host)| host == d || host.ends_with(&format!(".{d}")))
}

impl Condition {
    fn matches(&self, f: &Facts) -> bool {
        self.from.as_ref().is_none_or(|v| f.from.is_some_and(|a| a.eq_ignore_ascii_case(v.trim())))
            && self.domain.as_ref().is_none_or(|d| f.from.is_some_and(|a| domain_match(a, d)))
            && self.to.as_ref().is_none_or(|v| f.recipients.iter().any(|r| r.eq_ignore_ascii_case(v.trim())))
            && self.subject_contains.as_ref().is_none_or(|v| f.subject.to_lowercase().contains(&v.to_lowercase()))
            && self.list.is_none_or(|v| v == f.list)
            && self.mailbox.as_ref().is_none_or(|v| matches_mailbox(v, f.mailbox, f.role))
    }
}

/// Order: user rules (first match wins, always first), built-in rules (replied, bulk), then the default level (`default_level`, metadata only unless changed).
pub fn decide(config: &MailConfig, f: &Facts) -> Decision {
    for (i, r) in config.rules.iter().enumerate() {
        if r.when.matches(f) {
            return Decision { level: r.level, basis: format!("rule:{i}"), candidate: false, ai_pending: false };
        }
    }
    if config.builtin_replied && f.replied {
        return Decision { level: Level::Normal, basis: "builtin:replied".into(), candidate: true, ai_pending: config.ai_judge.enabled };
    }
    if config.builtin_rules {
        let noreply = f.from.is_some_and(|a| {
            let local = a.split('@').next().unwrap_or("");
            ["noreply", "no-reply", "donotreply", "do-not-reply", "notifications", "notification", "mailer-daemon"].contains(&local)
        });
        let basis = if f.list {
            Some("builtin:list")
        } else if f.automated {
            Some("builtin:automated")
        } else if noreply {
            Some("builtin:noreply")
        } else {
            None
        };
        if let Some(b) = basis {
            return Decision { level: Level::Metadata, basis: b.into(), candidate: false, ai_pending: false };
        }
    }
    // No rule levels it: metadata only; the body is read live from the source when opened.
    Decision { level: config.default_level, basis: "default".into(), candidate: false, ai_pending: config.ai_judge.enabled }
}
