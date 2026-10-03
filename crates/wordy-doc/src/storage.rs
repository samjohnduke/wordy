//! On-disk layout and atomic writes.
//!
//! ```text
//! <project>/
//!   project.loro      binary snapshot — the truth
//!   project.json      JSON mirror of the current state, for grep/debugging
//!   assets/           attachments by content hash
//!   snapshots/        rolling backups
//!   index.sqlite      derived, safe to delete
//! ```

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use loro::LoroDoc;

pub const SNAPSHOT_FILE: &str = "project.loro";
pub const JSON_FILE: &str = "project.json";
pub const BACKUP_DIR: &str = "snapshots";
pub const ASSETS_DIR: &str = "assets";
pub const MAX_RECENT_BACKUPS: usize = 10;

pub fn snapshot_path(dir: &Path) -> PathBuf {
    dir.join(SNAPSHOT_FILE)
}
pub fn json_path(dir: &Path) -> PathBuf {
    dir.join(JSON_FILE)
}

/// Default projects folder: `~/Wordy`.
pub fn projects_root() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("Wordy")
}

/// List project folders under the root (anything containing project.loro).
pub fn list_projects(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && snapshot_path(&p).exists() {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
}

/// Save snapshot + JSON mirror.
pub fn save(doc: &LoroDoc, dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let bytes = doc
        .export(loro::ExportMode::Snapshot)
        .map_err(|e| anyhow!("export snapshot: {e}"))?;
    write_atomic(&snapshot_path(dir), &bytes)?;
    let json = serde_json::to_string_pretty(&doc.get_deep_value())?;
    write_atomic(&json_path(dir), json.as_bytes())?;
    Ok(())
}

/// Copy the current snapshot into `snapshots/` with a timestamp and prune old ones.
pub fn backup(dir: &Path) -> Result<Option<PathBuf>> {
    let src = snapshot_path(dir);
    if !src.exists() {
        return Ok(None);
    }
    let bdir = dir.join(BACKUP_DIR);
    std::fs::create_dir_all(&bdir)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let dst = bdir.join(format!("project-{stamp}.loro"));
    std::fs::copy(&src, &dst)?;
    prune_backups(&bdir)?;
    Ok(Some(dst))
}

/// Keep the newest `MAX_RECENT_BACKUPS`, plus the first backup of each of the last 30 days.
fn prune_backups(bdir: &Path) -> Result<()> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(bdir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "loro").unwrap_or(false))
        .collect();
    files.sort();
    files.reverse(); // newest first
    let mut keep_days: Vec<String> = Vec::new();
    for (i, f) in files.iter().enumerate() {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        let day = name.get(8..16).unwrap_or("").to_string();
        let keep = i < MAX_RECENT_BACKUPS
            || (keep_days.len() < 30 && !keep_days.contains(&day));
        if keep {
            if !keep_days.contains(&day) {
                keep_days.push(day);
            }
        } else {
            let _ = std::fs::remove_file(f);
        }
    }
    Ok(())
}
