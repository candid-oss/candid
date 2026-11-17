//! Where a running timeline service can be reached (timeline-v1 §5.4). `endpoint.json` holds no secrets; the
//! command-line token lives in `cli.token`. Both are written 0600 in the per-user run directory.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceEndpoint {
    pub url: String,
    pub pid: i32,
    pub store_id: String,
    pub started_at: String,
}

pub struct RunDirectory;

impl RunDirectory {
    /// `CANDID_RUN_DIR` when set; otherwise `~/Library/Application Support/Candid/run` on macOS and
    /// `$XDG_DATA_HOME/Candid/run` (default `~/.local/share/Candid/run`) elsewhere.
    pub fn path(env: &HashMap<String, String>) -> PathBuf {
        if let Some(custom) = env.get("CANDID_RUN_DIR").filter(|v| !v.is_empty()) {
            return PathBuf::from(custom);
        }
        let home = env.get("HOME").cloned().unwrap_or_else(|| "/".into());
        if cfg!(target_os = "macos") {
            return Path::new(&home).join("Library/Application Support/Candid/run");
        }
        let data = env.get("XDG_DATA_HOME").filter(|v| !v.is_empty()).cloned().unwrap_or_else(|| format!("{home}/.local/share"));
        Path::new(&data).join("Candid/run")
    }

    pub fn from_process_env() -> PathBuf {
        Self::path(&std::env::vars().collect())
    }

    pub fn endpoint_file(dir: &Path) -> PathBuf {
        dir.join("endpoint.json")
    }

    /// Token of the built-in 「本机命令行」 client; only the current user can read it.
    pub fn cli_token_file(dir: &Path) -> PathBuf {
        dir.join("cli.token")
    }

    pub fn read_endpoint(dir: &Path) -> std::io::Result<ServiceEndpoint> {
        let data = fs::read(Self::endpoint_file(dir))?;
        serde_json::from_slice(&data).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Writes a file only the current user can read, replacing any previous one atomically.
    pub fn write_private(data: &[u8], path: &Path) -> std::io::Result<()> {
        let dir = path.parent().unwrap_or(Path::new("."));
        if !dir.exists() {
            fs::create_dir_all(dir)?;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let temp = dir.join(format!(".{name}.{}", candid_store::uuid_v4().to_uppercase()));
        let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&temp, path)
    }
}
