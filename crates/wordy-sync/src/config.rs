//! Per-machine sync settings: this machine's name, the server and the
//! linked account. Stored in `~/.config/wordy/sync.json` (override the
//! folder with `WORDY_CONFIG_DIR`, handy for running two copies on one
//! machine). The file holds a bearer token once linked, so it is written
//! readable by the owner only.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::cloud::CloudAccount;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SyncConfig {
    /// How this machine is listed on the account; defaults to the hostname.
    /// (`peer_name` is the field's old name, from the LAN sync days.)
    #[serde(alias = "peer_name")]
    pub device_name: String,
    /// Which Wordy server the cloud account lives on.
    pub cloud_server: String,
    /// The linked cloud account, if this machine has been linked.
    pub cloud: Option<CloudAccount>,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            device_name: default_device_name(),
            cloud_server: crate::cloud::DEFAULT_SERVER.to_string(),
            cloud: None,
        }
    }
}

pub fn default_device_name() -> String {
    gethostname::gethostname()
        .to_string_lossy()
        .trim_end_matches(".local")
        .to_string()
}

pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("WORDY_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("wordy")
}

pub fn config_path() -> PathBuf {
    config_dir().join("sync.json")
}

impl SyncConfig {
    /// Load the config, or defaults when there is none yet.
    pub fn load() -> Self {
        let path = config_path();
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!("{}: {e}; using defaults", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let json = serde_json::to_vec_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        write_private(&tmp, &json)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// The server URL without a trailing slash (falls back to the default
    /// when the field was emptied by hand).
    pub fn cloud_server(&self) -> String {
        let s = self.cloud_server.trim().trim_end_matches('/');
        if s.is_empty() {
            crate::cloud::DEFAULT_SERVER.to_string()
        } else {
            s.to_string()
        }
    }
}

/// Write `bytes` to `path`, creating it with mode 0600 on Unix.
fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write as _;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    #[cfg(unix)]
    {
        // The mode above only applies when the file is created.
        use std::os::unix::fs::PermissionsExt as _;
        let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    f.write_all(bytes)?;
    Ok(())
}
