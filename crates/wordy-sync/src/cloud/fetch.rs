//! Get a local copy of a project that lives on the server: one another
//! machine of ours put there, or one somebody shared with us. The room is
//! joined with an empty document, the replay is imported, the folder is
//! written, and `cloud.json` records where the replay stopped so the copy
//! carries on syncing from there.

use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use loro::{ExportMode, LoroDoc};
use wordy_doc::{storage, Project};

use super::room::{RoomEvent, RoomHandle, RoomOptions};
use super::state::CloudState;

/// How long the whole replay may take before giving up.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(120);

/// Download project `project_id` into `dir`, which must not exist yet (or
/// be empty). On success the folder opens like any other project and its
/// cloud sync is already on.
pub fn fetch_project(server: &str, token: &str, project_id: &str, project_name: &str, dir: &Path) -> Result<()> {
    if dir.exists() && dir.read_dir()?.next().is_some() {
        bail!("{} already exists and is not empty", dir.display());
    }
    std::fs::create_dir_all(dir.join("assets"))?;
    std::fs::create_dir_all(dir.join("snapshots"))?;

    let (tx, rx) = mpsc::channel();
    let room = RoomHandle::start(
        RoomOptions {
            server: server.to_string(),
            token: token.to_string(),
            project_id: project_id.to_string(),
            project_name: project_name.to_string(),
            assets_dir: dir.join("assets"),
            since: 0,
            dictionary: Vec::new(),
        },
        move |ev| {
            let _ = tx.send(ev);
        },
    );

    let doc = LoroDoc::new();
    let deadline = Instant::now() + FETCH_TIMEOUT;
    let outcome = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let ev = match rx.recv_timeout(left) {
            Ok(ev) => ev,
            Err(_) => break Err(anyhow!("the server did not finish sending the project in time")),
        };
        match ev {
            RoomEvent::Updates(batch) => {
                for (seq, bytes) in batch {
                    doc.import(&bytes).map_err(|e| anyhow!("importing update {seq}: {e}"))?;
                }
            }
            RoomEvent::Synced { head, .. } => break Ok(head),
            RoomEvent::Disconnected { reason, fatal, .. } => {
                if fatal {
                    break Err(anyhow!("{reason}"));
                }
                tracing::warn!("fetching {project_id}: {reason}; retrying");
            }
            RoomEvent::Warning(w) => tracing::warn!("fetching {project_id}: {w}"),
            _ => {}
        }
    };
    // Waits for the asset reconcile that follows the replay.
    room.finish();
    let head = match outcome {
        Ok(head) => head,
        Err(e) => {
            let _ = std::fs::remove_dir_all(dir);
            return Err(e);
        }
    };
    if doc.get_map("project").is_empty() {
        let _ = std::fs::remove_dir_all(dir);
        bail!("the server has no content for this project yet; sync it from the machine that has it first");
    }

    let snapshot = doc
        .export(ExportMode::Snapshot)
        .map_err(|e| anyhow!("exporting the snapshot: {e}"))?;
    storage::write_snapshot(dir, &snapshot)?;
    let mut state = CloudState {
        enabled: true,
        last_seq: head,
        ..CloudState::default()
    };
    state.set_vv(&doc.oplog_vv());
    state.save(dir)?;
    // Open it once now: that checks the copy, writes the JSON mirror and
    // normalises anything an older writer left out.
    let project = Project::open(dir).context("the downloaded project does not open")?;
    project.save_and_mirror()?;
    Ok(())
}
