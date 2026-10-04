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
/// `~/Wordy`, or `WORDY_PROJECTS_DIR` when set (a second instance for
/// testing keeps its projects apart that way).
pub fn projects_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("WORDY_PROJECTS_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("Wordy")
}

/// A folder name for a project called `name` under `root` that does not
/// exist yet: the name itself, then "name 2", "name 3", …
pub fn free_project_dir(root: &Path, name: &str) -> PathBuf {
    let base: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .to_string();
    let base = if base.is_empty() { "Project".to_string() } else { base };
    let first = root.join(&base);
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| root.join(format!("{base} {n}")))
        .find(|p| !p.exists())
        .expect("an unused folder name")
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

/// Write `bytes` to a sibling temp file, fsync it, then rename it into place,
/// so a crash mid-write leaves either the old file or the new one, never a
/// torn one.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write as _;
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        f.write_all(bytes)
            .with_context(|| format!("writing {}", tmp.display()))?;
        f.sync_all().with_context(|| format!("syncing {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
}

/// Save snapshot + JSON mirror.
pub fn save(doc: &LoroDoc, dir: &Path) -> Result<()> {
    let bytes = doc
        .export(loro::ExportMode::Snapshot)
        .map_err(|e| anyhow!("export snapshot: {e}"))?;
    write_project(doc, dir, &bytes)
}

/// Write an already exported snapshot plus the JSON mirror of `doc`.
pub fn write_project(doc: &LoroDoc, dir: &Path, snapshot: &[u8]) -> Result<()> {
    write_snapshot(dir, snapshot)?;
    write_json_mirror(doc, dir)
}

/// Write only `project.loro`, the file that matters.
pub fn write_snapshot(dir: &Path, snapshot: &[u8]) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    write_atomic(&snapshot_path(dir), snapshot)
}

/// Write `project.json`, a readable copy of the current state for grep and
/// debugging. It is derived, so callers may skip it on most autosaves.
pub fn write_json_mirror(doc: &LoroDoc, dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(&doc.get_deep_value())?;
    write_atomic(&json_path(dir), json.as_bytes())
}

/// The project id as the JSON mirror has it, without loading the project.
/// Ids never change, so the mirror (rewritten at most every 30 s) is good
/// enough; `None` for a folder without a mirror or one from before ids.
pub fn mirrored_id(dir: &Path) -> Option<String> {
    let bytes = std::fs::read(json_path(dir)).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let id = v.get("project")?.get("id")?.as_str()?;
    (!id.is_empty()).then(|| id.to_string())
}

/// The project name as the JSON mirror has it, without loading the project;
/// `None` for a folder without a mirror.
pub fn mirrored_name(dir: &Path) -> Option<String> {
    let bytes = std::fs::read(json_path(dir)).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let name = v.get("project")?.get("name")?.as_str()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// The project under `root` whose snapshot was written most recently: the
/// one that was open last, give or take a sync. `None` for an empty root.
pub fn most_recent_project(root: &Path) -> Option<PathBuf> {
    list_projects(root)
        .into_iter()
        .filter_map(|p| {
            let modified = std::fs::metadata(snapshot_path(&p)).ok()?.modified().ok()?;
            Some((modified, p))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, p)| p)
}

/// Rolling backups under `snapshots/`, newest first.
pub fn backups(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir.join(BACKUP_DIR))
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().map(|e| e == "loro").unwrap_or(false))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files.reverse();
    files
}

/// Move a damaged main file aside (`project.loro.corrupt-<stamp>`) so a
/// recovery save does not destroy the evidence.
pub fn quarantine(path: &Path) -> Result<PathBuf> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let name = format!(
        "{}.corrupt-{stamp}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("project.loro")
    );
    let dst = path.with_file_name(name);
    std::fs::rename(path, &dst).with_context(|| format!("moving {} aside", path.display()))?;
    Ok(dst)
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
        let keep = i < MAX_RECENT_BACKUPS || (keep_days.len() < 30 && !keep_days.contains(&day));
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn project_dir(root: &Path, name: &str, age: Duration) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let path = snapshot_path(&dir);
        std::fs::write(&path, b"x").unwrap();
        let f = std::fs::File::options().write(true).open(&path).unwrap();
        f.set_modified(SystemTime::now() - age).unwrap();
        dir
    }

    #[test]
    fn most_recent_project_goes_by_snapshot_time_not_name() {
        let root = tempfile::tempdir().unwrap();
        // "Zeta" sorts last by name but was written a day ago; "Alpha" is fresh.
        project_dir(root.path(), "Zeta", Duration::from_secs(86_400));
        let alpha = project_dir(root.path(), "Alpha", Duration::from_secs(60));
        std::fs::create_dir_all(root.path().join("not a project")).unwrap();
        assert_eq!(most_recent_project(root.path()), Some(alpha));
    }

    #[test]
    fn most_recent_project_is_none_for_an_empty_root() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(most_recent_project(root.path()), None);
        assert_eq!(most_recent_project(&root.path().join("missing")), None);
    }

    #[test]
    fn mirrored_name_reads_the_json_mirror() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(mirrored_name(root.path()), None);
        std::fs::write(json_path(root.path()), r#"{"project":{"name":"  Dune  "}}"#).unwrap();
        assert_eq!(mirrored_name(root.path()).as_deref(), Some("Dune"));
    }
}
