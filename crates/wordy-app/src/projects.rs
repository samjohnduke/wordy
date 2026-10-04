//! The projects this machine knows about: the recent list from prefs plus
//! every folder under the projects root. Switching, creating and opening
//! live on the workspace; this is the list and the folder rules.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use gpui_kit::App;
use wordy_doc::{storage, Project};

use crate::prefs::Prefs;

/// A project folder on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownProject {
    pub dir: PathBuf,
    pub name: String,
}

/// The display name of the project in `dir` without opening it: the
/// mirror's name, else the folder name.
pub fn project_name(dir: &Path) -> String {
    storage::mirrored_name(dir).unwrap_or_else(|| {
        dir.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".to_string())
    })
}

/// Every project this machine can open: recent ones first (most recent
/// first), then the rest of the projects root by name. Folders that no
/// longer hold a project are left out.
pub fn known_projects(cx: &App) -> Vec<KnownProject> {
    let mut dirs: Vec<PathBuf> = Prefs::global(cx)
        .recent_projects
        .iter()
        .filter(|d| storage::snapshot_path(d).exists())
        .cloned()
        .collect();
    for dir in storage::list_projects(&storage::projects_root()) {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs.into_iter()
        .map(|dir| KnownProject {
            name: project_name(&dir),
            dir,
        })
        .collect()
}

/// Create a project called `name` in a fresh folder under the projects root.
pub fn create_project(name: &str) -> Result<Project> {
    let name = name.trim();
    if name.is_empty() {
        bail!("give the project a name");
    }
    let root = storage::projects_root();
    std::fs::create_dir_all(&root)?;
    let dir = storage::free_project_dir(&root, name);
    tracing::info!("creating {}", dir.display());
    Project::create(&dir, name)
}

/// Open the project in `dir`, or turn an empty folder into a new project
/// named after it, so "Open a folder" also works for keeping a project
/// outside the projects root.
pub fn open_or_create_in(dir: &Path) -> Result<Project> {
    if storage::snapshot_path(dir).exists() {
        return Project::open(dir);
    }
    let empty = dir.read_dir().map(|mut rd| rd.next().is_none()).unwrap_or(true);
    if !empty {
        bail!(
            "{} is not a Wordy project: it has no {} and is not empty",
            dir.display(),
            storage::SNAPSHOT_FILE
        );
    }
    let name = project_name(dir);
    Project::create(dir, &name)
}
