use std::fmt;

/// Errors of the timeline store. `code()` is stable and also written to `access_log.denied`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineError {
    Database(String),
    NewerSchema { found: i64, supported: i64 },
    InvalidStore(String),
    Locked,
    InvalidInput(String),
    UnknownSource(String),
    SourceNotWritable(String),
    UnknownClient(String),
    Denied(String),
    NotFound(String),
    InvalidCursor,
    BlobMismatch(String),
    MissingParent(String),
    Integrity(Vec<String>),
    SimulatedCrash,
    /// File-system failure outside SQLite (the Swift version surfaces these as Foundation errors).
    Io(String),
}

impl TimelineError {
    pub fn code(&self) -> String {
        match self {
            Self::Database(_) => "database".into(),
            Self::NewerSchema { .. } => "newer_schema".into(),
            Self::InvalidStore(_) => "invalid_store".into(),
            Self::Locked => "locked".into(),
            Self::InvalidInput(_) => "invalid_input".into(),
            Self::UnknownSource(_) => "unknown_source".into(),
            Self::SourceNotWritable(_) => "source_not_writable".into(),
            Self::UnknownClient(_) => "unknown_client".into(),
            Self::Denied(c) => c.clone(),
            Self::NotFound(_) => "not_found".into(),
            Self::InvalidCursor => "invalid_cursor".into(),
            Self::BlobMismatch(_) => "blob_mismatch".into(),
            Self::MissingParent(_) => "missing_parent".into(),
            Self::Integrity(_) => "integrity".into(),
            Self::SimulatedCrash => "simulated_crash".into(),
            Self::Io(_) => "io".into(),
        }
    }

    /// The user-facing message (same wording as the Swift `errorDescription`).
    pub fn message(&self) -> String {
        match self {
            Self::Database(d) => format!("时间线库操作失败：{d}"),
            Self::NewerSchema { found, supported } => format!("数据库版本 {found} 高于当前程序支持的 {supported}，拒绝写入。"),
            Self::InvalidStore(d) => format!("数据根目录无效：{d}"),
            Self::Locked => "数据根目录已被另一个服务占用。".into(),
            Self::InvalidInput(d) => format!("输入无效：{d}"),
            Self::UnknownSource(s) => format!("未知来源 {s}"),
            Self::SourceNotWritable(s) => format!("来源状态不允许写入：{s}"),
            Self::UnknownClient(c) => format!("未登记的访问方 {c}"),
            Self::Denied(c) => format!("访问被拒绝：{c}"),
            Self::NotFound(d) => format!("未找到 {d}"),
            Self::InvalidCursor => "游标无效或与请求不匹配。".into(),
            Self::BlobMismatch(d) => format!("原件校验不一致：{d}"),
            Self::MissingParent(d) => format!("父事件不存在：{d}"),
            Self::Integrity(issues) => format!("完整性检查失败：{}", issues.join("; ")),
            Self::SimulatedCrash => "simulated crash".into(),
            Self::Io(d) => format!("文件操作失败：{d}"),
        }
    }
}

impl fmt::Display for TimelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for TimelineError {}

impl From<rusqlite::Error> for TimelineError {
    fn from(e: rusqlite::Error) -> Self {
        match e {
            rusqlite::Error::SqliteFailure(_, Some(msg)) => Self::Database(msg),
            other => Self::Database(other.to_string()),
        }
    }
}

impl From<std::io::Error> for TimelineError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

impl From<serde_json::Error> for TimelineError {
    fn from(e: serde_json::Error) -> Self {
        Self::Database(format!("JSON: {e}"))
    }
}
