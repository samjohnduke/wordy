//! Per-machine sync settings: this machine's name and the pairing code.
//! Stored in `~/.config/wordy/sync.json` (override the folder with
//! `WORDY_CONFIG_DIR`, handy for running two copies on one machine).

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SyncConfig {
    /// Shown to the other side; defaults to the hostname.
    pub peer_name: String,
    /// Pre-shared pairing code, typed once on each machine.
    pub pairing_code: String,
    /// Listening port; 0 picks a free one each launch.
    pub port: u16,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            peer_name: default_peer_name(),
            pairing_code: String::new(),
            port: 0,
        }
    }
}

pub fn default_peer_name() -> String {
    gethostname::gethostname()
        .to_string_lossy()
        .trim_end_matches(".local")
        .to_string()
}

pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("WORDY_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("wordy")
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
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let json = serde_json::to_vec_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn ready(&self) -> bool {
        !self.pairing_code.trim().is_empty() && !self.peer_name.trim().is_empty()
    }
}
