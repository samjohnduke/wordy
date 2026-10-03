//! A Wordy project: one LoroDoc plus typed access to its containers.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use loro::{
    CommitOptions, ExportMode, Frontiers, LoroDoc, LoroMap, LoroTree, LoroValue, TreeID, TreeParentId,
    ValueOrContainer, ID,
};

use crate::comments::Comments;

use crate::node::{Node, NodeKind, Space, Status};
use crate::paragraphs::Paragraphs;
use crate::schema::{self, meta};
use crate::storage;

/// Commit origin for everything that is not a body edit. See [`Project::commit_meta`].
pub const META_ORIGIN: &str = "meta";
/// Commit origin for project-wide body edits made outside any editor (rename
/// propagation). Editors exclude it from their undo stacks.
pub const BULK_ORIGIN: &str = "bulk";

pub struct Project {
    pub doc: LoroDoc,
    /// Folder on disk; `None` for an in-memory project (tests).
    pub dir: Option<PathBuf>,
    /// The backup this project was loaded from because `project.loro` was unreadable.
    recovered_from: Option<PathBuf>,
    /// Set by [`Project::compact_history`]: saves drop history before this
    /// version. The in-memory doc keeps its full history until the next launch.
    shallow_root: RefCell<Option<Frontiers>>,
    /// When `project.json` was last written; see [`MIRROR_INTERVAL`].
    last_mirror: Cell<Option<Instant>>,
}

/// `project.json` is rewritten at most this often by [`Project::save`].
/// [`Project::save_and_mirror`] (used on quit) always writes it.
pub const MIRROR_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// History older than this is discarded automatically at startup.
pub const PRUNE_AFTER_DAYS: i64 = 365;

/// What [`Project::prune_if_old`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PruneReport {
    /// Age of the oldest change that was discarded, in days.
    pub age_days: i64,
    pub before: HistoryStats,
    pub after: HistoryStats,
}

/// Size of the edit history, for the dashboard.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryStats {
    pub changes: usize,
    pub ops: usize,
    /// History before some version has been discarded.
    pub shallow: bool,
    /// Size of `project.loro` on disk.
    pub file_bytes: u64,
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Project {
    /// Create a new empty project in memory with the three space roots.
    pub fn new_in_memory(name: &str) -> Result<Self> {
        let doc = LoroDoc::new();
        doc.set_record_timestamp(true);
        schema::configure_text_styles(&doc);
        let project = Self {
            doc,
            dir: None,
            recovered_from: None,
            shallow_root: RefCell::new(None),
            last_mirror: Cell::new(None),
        };
        project.init_schema(name)?;
        project.ensure_id()?;
        Ok(project)
    }

    /// Create a new project folder at `dir` and write the first snapshot.
    pub fn create(dir: &Path, name: &str) -> Result<Self> {
        if dir.exists() && dir.read_dir()?.next().is_some() {
            bail!("{} already exists and is not empty", dir.display());
        }
        std::fs::create_dir_all(dir.join("assets"))?;
        std::fs::create_dir_all(dir.join("snapshots"))?;
        let mut p = Self::new_in_memory(name)?;
        p.dir = Some(dir.to_path_buf());
        p.save()?;
        Ok(p)
    }

    /// Open an existing project folder.
    ///
    /// If `project.loro` is missing or will not load (a crash mid-write, a
    /// bad disk), the newest readable backup in `snapshots/` is used instead,
    /// the damaged file is moved aside, and [`Project::recovered_from`] says so.
    pub fn open(dir: &Path) -> Result<Self> {
        let main = storage::snapshot_path(dir);
        let mut recovered_from = None;
        let doc = match Self::load_doc(&main) {
            Ok(doc) => doc,
            Err(main_err) => {
                let mut found = None;
                for backup in storage::backups(dir) {
                    match Self::load_doc(&backup) {
                        Ok(doc) => {
                            found = Some((doc, backup));
                            break;
                        }
                        Err(e) => tracing::warn!("backup {} unusable: {e:#}", backup.display()),
                    }
                }
                let Some((doc, backup)) = found else {
                    return Err(main_err.context("and no readable backup in snapshots/"));
                };
                tracing::error!("{main_err:#}; recovering from {}", backup.display());
                if main.exists() {
                    match storage::quarantine(&main) {
                        Ok(aside) => tracing::info!("damaged file kept as {}", aside.display()),
                        Err(e) => tracing::error!("{e:#}"),
                    }
                }
                recovered_from = Some(backup);
                doc
            }
        };
        let p = Self {
            doc,
            dir: Some(dir.to_path_buf()),
            recovered_from,
            shallow_root: RefCell::new(None),
            last_mirror: Cell::new(None),
        };
        p.ensure_roots()?;
        if p.recovered_from.is_some() {
            // Put a good main file back right away.
            p.save()?;
        }
        if p.ensure_id()? {
            // A project from before ids: write it down now so a copy made
            // before the next edit carries the same id.
            p.save()?;
        }
        Ok(p)
    }

    /// Write the binary snapshot and the JSON mirror atomically.
    /// Commit pending non-body edits (tree, metadata, comments) under the
    /// `meta` origin. Editors exclude that origin from their undo stacks, so a
    /// rename or a status change never gets undone by Ctrl-Z in a scene.
    pub fn commit_meta(&self) {
        self.doc.commit_with(CommitOptions::default().origin(META_ORIGIN));
    }

    fn load_doc(path: &Path) -> Result<LoroDoc> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let doc = LoroDoc::new();
        doc.set_record_timestamp(true);
        schema::configure_text_styles(&doc);
        doc.import(&bytes)
            .map_err(|e| anyhow!("import {}: {e}", path.display()))?;
        if doc.get_tree(schema::NODES).is_empty() && doc.get_map(schema::PROJECT).is_empty() {
            bail!("{} holds no project data", path.display());
        }
        Ok(doc)
    }

    /// The backup this project was recovered from at open, if any.
    pub fn recovered_from(&self) -> Option<&Path> {
        self.recovered_from.as_deref()
    }

    /// Write the snapshot. The JSON mirror is refreshed only if it is older
    /// than [`MIRROR_INTERVAL`] (or was never written); it is a convenience
    /// copy, and serializing the whole project on every autosave is wasted work.
    pub fn save(&self) -> Result<()> {
        let due = self
            .last_mirror
            .get()
            .map(|t| t.elapsed() >= MIRROR_INTERVAL)
            .unwrap_or(true);
        self.save_inner(due)
    }

    /// Write the snapshot and the JSON mirror unconditionally. Use on quit
    /// and for explicit saves, so the mirror never lags behind for long.
    pub fn save_and_mirror(&self) -> Result<()> {
        self.save_inner(true)
    }

    fn save_inner(&self, mirror: bool) -> Result<()> {
        let Some(dir) = &self.dir else { return Ok(()) };
        self.commit_meta();
        let bytes = match &*self.shallow_root.borrow() {
            Some(f) => self
                .doc
                .export(ExportMode::shallow_snapshot(f))
                .map_err(|e| anyhow!("export: {e}"))?,
            None => self
                .doc
                .export(ExportMode::Snapshot)
                .map_err(|e| anyhow!("export: {e}"))?,
        };
        storage::write_snapshot(dir, &bytes)?;
        if mirror {
            storage::write_json_mirror(&self.doc, dir)?;
            self.last_mirror.set(Some(Instant::now()));
        }
        Ok(())
    }

    /// Unix time (seconds) of the oldest change still in the op log, if any.
    /// Loro records a timestamp on every commit (see `set_record_timestamp`).
    pub fn oldest_change_timestamp(&self) -> Option<i64> {
        let vv = self.doc.oplog_vv();
        let since = self.doc.shallow_since_vv();
        let mut oldest: Option<i64> = None;
        for (peer, end) in vv.iter() {
            let start = since.get(peer).copied().unwrap_or(0);
            if start >= *end {
                continue;
            }
            if let Some(meta) = self.doc.get_change(ID::new(*peer, start)) {
                // Loro stores seconds; be tolerant of a millisecond value.
                let mut ts = meta.timestamp;
                if ts > 100_000_000_000 {
                    ts /= 1000;
                }
                if ts > 0 {
                    oldest = Some(oldest.map_or(ts, |o| o.min(ts)));
                }
            }
        }
        oldest
    }

    /// Days between the oldest change in the op log and now.
    pub fn history_age_days(&self) -> Option<i64> {
        let oldest = self.oldest_change_timestamp()?;
        let now = chrono::Utc::now().timestamp();
        Some((now - oldest).max(0) / 86_400)
    }

    /// Startup housekeeping: if the op log reaches back more than
    /// [`PRUNE_AFTER_DAYS`], back the full file up and compact the history,
    /// so a years-old project never needs the user to find the Compact button.
    /// Returns what happened, or `None` if nothing needed doing.
    pub fn prune_if_old(&self) -> Result<Option<PruneReport>> {
        let Some(age_days) = self.history_age_days() else {
            return Ok(None);
        };
        // Fewer than two changes means there is nothing behind the head to drop.
        if age_days <= PRUNE_AFTER_DAYS || self.doc.len_changes() < 2 {
            return Ok(None);
        }
        let before = self.history_stats();
        let after = self.compact_history()?;
        tracing::info!(
            "pruned history older than {age_days} days: project.loro {} -> {} bytes (in-memory history is dropped on relaunch)",
            before.file_bytes,
            after.file_bytes
        );
        Ok(Some(PruneReport {
            age_days,
            before,
            after,
        }))
    }

    pub fn history_stats(&self) -> HistoryStats {
        let file_bytes = self
            .dir
            .as_deref()
            .and_then(|d| std::fs::metadata(storage::snapshot_path(d)).ok())
            .map(|m| m.len())
            .unwrap_or(0);
        HistoryStats {
            changes: self.doc.len_changes(),
            ops: self.doc.len_ops(),
            shallow: self.doc.is_shallow() || self.shallow_root.borrow().is_some(),
            file_bytes,
        }
    }

    /// Discard edit history before the current version. The full file is
    /// backed up to `snapshots/` first. Saved versions and comments are
    /// ordinary data and are kept; only the CRDT op log shrinks.
    ///
    /// Sync note: another machine can still exchange changes after this as
    /// long as it already has everything this copy has now. Compact right
    /// after a sync.
    pub fn compact_history(&self) -> Result<HistoryStats> {
        if let Some(dir) = &self.dir {
            storage::backup(dir)?;
        }
        self.commit_meta();
        *self.shallow_root.borrow_mut() = Some(self.doc.oplog_frontiers());
        self.save()?;
        Ok(self.history_stats())
    }

    /// Import another copy of this project (e.g. from the other machine). Loro merges.
    pub fn import_bytes(&self, bytes: &[u8]) -> Result<()> {
        self.doc.import(bytes).map_err(|e| anyhow!("import: {e}"))?;
        self.ensure_roots()?;
        self.commit_meta();
        Ok(())
    }

    /// Import several updates in one go (a cloud replay).
    pub fn import_many(&self, updates: &[Vec<u8>]) -> Result<()> {
        self.doc.import_batch(updates).map_err(|e| anyhow!("import: {e}"))?;
        self.ensure_roots()?;
        self.commit_meta();
        Ok(())
    }

    fn init_schema(&self, name: &str) -> Result<()> {
        let project = self.project_map();
        project.insert(schema::project::NAME, name)?;
        project.insert(schema::project::CREATED, now_ms())?;
        project.insert(schema::project::SCHEMA, schema::SCHEMA_VERSION)?;
        project.ensure_mergeable_map(schema::project::SETTINGS)?;
        self.ensure_roots()?;
        self.doc.commit();
        Ok(())
    }

    /// Make sure every space has exactly one root and one trash. Idempotent.
    ///
    /// Two copies of a project that each created their own roots (or a fresh
    /// doc that imported another) end up with duplicates; those are merged
    /// into the one with the smallest id so every peer converges on the same.
    fn ensure_roots(&self) -> Result<()> {
        self.dedupe_roots()?;
        for space in [Space::Manuscript, Space::World, Space::Notes] {
            if self.find_root(space, NodeKind::Root).is_none() {
                let id = self.tree().create(TreeParentId::Root)?;
                self.write_meta(id, NodeKind::Root, space, space.label())?;
            }
            if self.find_root(space, NodeKind::Trash).is_none() {
                let id = self.tree().create(TreeParentId::Root)?;
                self.write_meta(id, NodeKind::Trash, space, "Trash")?;
            }
        }
        Ok(())
    }

    fn dedupe_roots(&self) -> Result<()> {
        let tree = self.tree();
        for space in [Space::Manuscript, Space::World, Space::Notes] {
            for kind in [NodeKind::Root, NodeKind::Trash] {
                let roots = self.roots_matching(space, kind);
                let Some((keep, extras)) = roots.split_first() else {
                    continue;
                };
                for extra in extras {
                    for child in tree.children(*extra).unwrap_or_default() {
                        tree.mov(child, *keep)?;
                    }
                    tree.delete(*extra)?;
                }
            }
        }
        Ok(())
    }

    /// Top-level nodes of the given space and kind, smallest id first.
    fn roots_matching(&self, space: Space, kind: NodeKind) -> Vec<TreeID> {
        let mut roots: Vec<TreeID> = self
            .tree()
            .roots()
            .into_iter()
            .filter(|id| {
                self.node(*id)
                    .map(|n| n.kind() == kind && n.space() == space)
                    .unwrap_or(false)
            })
            .collect();
        roots.sort_by_key(|id| (id.peer, id.counter));
        roots
    }

    fn find_root(&self, space: Space, kind: NodeKind) -> Option<TreeID> {
        self.roots_matching(space, kind).into_iter().next()
    }

    fn write_meta(&self, id: TreeID, kind: NodeKind, space: Space, title: &str) -> Result<()> {
        let meta = self.tree().get_meta(id)?;
        meta.insert(meta::KIND, kind.as_str())?;
        meta.insert(meta::SPACE, space.as_str())?;
        meta.insert(meta::TITLE, title)?;
        meta.insert(meta::CREATED, now_ms())?;
        Ok(())
    }

    // ---- containers -------------------------------------------------------

    pub fn project_map(&self) -> LoroMap {
        self.doc.get_map(schema::PROJECT)
    }
    pub fn tree(&self) -> LoroTree {
        self.doc.get_tree(schema::NODES)
    }
    pub fn tasks(&self) -> LoroMap {
        self.doc.get_map(schema::TASKS)
    }
    pub fn comments(&self) -> Comments {
        Comments::new(self.doc.get_map(schema::COMMENTS))
    }
    pub fn versions_map(&self) -> LoroMap {
        self.doc.get_map(schema::VERSIONS)
    }
    pub fn sessions_map(&self) -> LoroMap {
        self.doc.get_map(schema::SESSIONS)
    }
    pub fn settings_map(&self) -> LoroMap {
        self.project_map()
            .ensure_mergeable_map(schema::project::SETTINGS)
            .expect("settings map")
    }

    /// The project's stable id (a ulid minted when it was first opened by a
    /// version that knows about ids). Copies made by zipping or syncing share it.
    pub fn id(&self) -> String {
        match self.project_map().get(schema::project::ID) {
            Some(ValueOrContainer::Value(LoroValue::String(s))) => s.to_string(),
            _ => String::new(),
        }
    }

    /// Mint an id if this project predates ids. Commits under `meta`.
    /// Returns whether one was minted.
    fn ensure_id(&self) -> Result<bool> {
        if !self.id().is_empty() {
            return Ok(false);
        }
        self.project_map()
            .insert(schema::project::ID, ulid::Ulid::new().to_string())?;
        self.commit_meta();
        Ok(true)
    }

    pub fn name(&self) -> String {
        match self.project_map().get(schema::project::NAME) {
            Some(ValueOrContainer::Value(LoroValue::String(s))) => s.to_string(),
            _ => "Untitled".into(),
        }
    }

    pub fn set_name(&self, name: &str) -> Result<()> {
        self.project_map().insert(schema::project::NAME, name)?;
        Ok(())
    }

    // ---- nodes -------------------------------------------------------------

    pub fn node(&self, id: TreeID) -> Result<Node> {
        Node::new(self.tree(), id)
    }

    pub fn root(&self, space: Space) -> TreeID {
        self.find_root(space, NodeKind::Root)
            .expect("roots exist after ensure_roots")
    }

    pub fn trash(&self, space: Space) -> TreeID {
        self.find_root(space, NodeKind::Trash)
            .expect("trash exists after ensure_roots")
    }

    /// Children in order.
    pub fn children(&self, parent: TreeID) -> Vec<TreeID> {
        self.tree().children(parent).unwrap_or_default()
    }

    /// Create a node as the last child of `parent`.
    pub fn create_node(&self, parent: TreeID, kind: NodeKind, title: &str) -> Result<TreeID> {
        let space = self.node(parent)?.space();
        let id = self.tree().create(parent)?;
        self.write_meta(id, kind, space, title)?;
        if kind == NodeKind::Scene {
            let m = self.tree().get_meta(id)?;
            m.insert(meta::STATUS, Status::Draft.as_str())?;
            m.insert(meta::INCLUDE_IN_COMPILE, true)?;
        }
        if kind.has_body() {
            let body = self.tree().get_meta(id)?.ensure_mergeable_text(meta::BODY)?;
            if body.is_empty() {
                // Quill convention: a body always ends with a newline.
                body.insert(0, "\n")?;
            }
        }
        Ok(id)
    }

    /// Create a node right after `sibling`, in the same parent.
    pub fn create_node_after(&self, sibling: TreeID, kind: NodeKind, title: &str) -> Result<TreeID> {
        let parent = self
            .node(sibling)?
            .parent()
            .ok_or_else(|| anyhow!("sibling has no parent"))?;
        let id = self.create_node(parent, kind, title)?;
        self.tree().mov_after(id, sibling)?;
        Ok(id)
    }

    pub fn move_node(&self, id: TreeID, new_parent: TreeID, index: usize) -> Result<()> {
        self.tree().mov_to(id, new_parent, index)?;
        Ok(())
    }

    /// Move `id` directly before `sibling` (same parent as `sibling`).
    pub fn move_before(&self, id: TreeID, sibling: TreeID) -> Result<()> {
        self.tree().mov_before(id, sibling)?;
        Ok(())
    }

    /// Move `id` directly after `sibling` (same parent as `sibling`).
    pub fn move_after(&self, id: TreeID, sibling: TreeID) -> Result<()> {
        self.tree().mov_after(id, sibling)?;
        Ok(())
    }

    /// Move `id` to be the last child of `parent`.
    pub fn move_into(&self, id: TreeID, parent: TreeID) -> Result<()> {
        self.tree().mov(id, parent)?;
        Ok(())
    }

    /// Whether `id` is `ancestor` or sits somewhere below it.
    pub fn is_descendant(&self, id: TreeID, ancestor: TreeID) -> bool {
        let tree = self.tree();
        let mut cur = id;
        loop {
            if cur == ancestor {
                return true;
            }
            match tree.parent(cur) {
                Some(loro::TreeParentId::Node(p)) => cur = p,
                _ => return false,
            }
        }
    }

    /// Deep copy of `id`, placed right after it: metadata, body with marks,
    /// fields, tags, aliases, relations, attachments and every descendant.
    pub fn duplicate_node(&self, id: TreeID) -> Result<TreeID> {
        let src = self.node(id)?;
        let parent = src.parent().ok_or_else(|| anyhow!("cannot duplicate a root"))?;
        let title = format!("{} copy", src.title());
        let new_id = self.copy_subtree(id, parent, Some(&title))?;
        self.tree().mov_after(new_id, id)?;
        Ok(new_id)
    }

    fn copy_subtree(&self, src_id: TreeID, parent: TreeID, title: Option<&str>) -> Result<TreeID> {
        let src = self.node(src_id)?;
        let title = title.map(str::to_string).unwrap_or_else(|| src.title());
        let new_id = self.create_node(parent, src.kind(), &title)?;
        self.node(new_id)?.copy_from(&src)?;
        for child in src.children() {
            self.copy_subtree(child, new_id, None)?;
        }
        Ok(new_id)
    }

    /// Bring a trashed node back to the end of its space's root.
    pub fn restore_node(&self, id: TreeID) -> Result<()> {
        let space = self.node(id)?.space();
        self.tree().mov(id, self.root(space))?;
        Ok(())
    }

    /// Soft delete: move to the space's trash.
    pub fn trash_node(&self, id: TreeID) -> Result<()> {
        let space = self.node(id)?.space();
        self.tree().mov(id, self.trash(space))?;
        Ok(())
    }

    /// Hard delete from the tree (history still keeps it).
    pub fn delete_node(&self, id: TreeID) -> Result<()> {
        self.tree().delete(id)?;
        Ok(())
    }

    /// All live nodes of a kind, any space, in tree order.
    pub fn nodes_of_kind(&self, kind: NodeKind) -> Vec<TreeID> {
        let mut out = Vec::new();
        for space in [Space::Manuscript, Space::World, Space::Notes] {
            self.walk(self.root(space), &mut |id, n| {
                if n.kind() == kind {
                    out.push(id)
                }
            });
        }
        out
    }

    /// Every live entity, in World-tree order.
    pub fn entities(&self) -> Vec<TreeID> {
        self.nodes_of_kind(NodeKind::Entity)
    }

    /// Names for the mention matcher: each entity's title plus aliases.
    pub fn entity_names(&self) -> Vec<crate::mentions::EntityNames> {
        self.entities()
            .into_iter()
            .filter_map(|id| self.node(id).ok())
            .map(|n| {
                let mut names = vec![n.title()];
                names.extend(n.aliases());
                crate::mentions::EntityNames { id: n.id, names }
            })
            .collect()
    }

    /// Explicit links to `entity` whose text reads `text` (case-insensitive),
    /// per live node: what a rename offers to update.
    pub fn linked_mentions(&self, entity: TreeID, text: &str) -> Vec<(TreeID, usize)> {
        let id = entity.to_string();
        let want = text.trim().to_lowercase();
        if want.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for node in self.all_nodes() {
            let Some(body) = self.node(node).ok().and_then(|n| n.body_if_exists()) else {
                continue;
            };
            let paras = Paragraphs::from_text(&body);
            let n = paras
                .link_spans()
                .into_iter()
                .filter(|(r, l)| *l == id && paras.slice_cp(r.clone()).trim().to_lowercase() == want)
                .count();
            if n > 0 {
                out.push((node, n));
            }
        }
        out
    }

    /// Replace the text under every explicit link to `entity` in `node`'s body
    /// that reads `from` with `to`, keeping the link, as one commit under
    /// `origin`. Returns how many spans changed.
    pub fn replace_linked_mentions(
        &self,
        node: TreeID,
        entity: TreeID,
        from: &str,
        to: &str,
        origin: &str,
    ) -> Result<usize> {
        let Some(body) = self.node(node)?.body_if_exists() else {
            return Ok(0);
        };
        let id = entity.to_string();
        let want = from.trim().to_lowercase();
        let paras = Paragraphs::from_text(&body);
        let spans: Vec<Range<usize>> = paras
            .link_spans()
            .into_iter()
            .filter(|(r, l)| *l == id && paras.slice_cp(r.clone()).trim().to_lowercase() == want)
            .map(|(r, _)| r)
            .collect();
        let n = spans.len();
        for r in spans.into_iter().rev() {
            body.delete(r.start, r.len())?;
            body.insert(r.start, to)?;
            body.mark(r.start..r.start + to.chars().count(), "link", id.as_str())?;
        }
        if n > 0 {
            self.doc.commit_with(CommitOptions::default().origin(origin));
        }
        Ok(n)
    }

    /// Every live node in every space, depth first.
    pub fn all_nodes(&self) -> Vec<TreeID> {
        let mut out = Vec::new();
        for space in [Space::Manuscript, Space::World, Space::Notes] {
            self.walk(self.root(space), &mut |id, _| out.push(id));
        }
        out
    }

    /// Whether `id` is live (under a space root rather than a trash root or gone).
    pub fn is_live(&self, id: TreeID) -> bool {
        let tree = self.tree();
        let mut cur = id;
        loop {
            match tree.parent(cur) {
                Some(loro::TreeParentId::Node(p)) => cur = p,
                Some(loro::TreeParentId::Root) => {
                    return self.node(cur).map(|n| n.kind() == NodeKind::Root).unwrap_or(false)
                }
                _ => return false,
            }
        }
    }

    /// Depth-first walk of live nodes under `parent` (excluding `parent`).
    pub fn walk(&self, parent: TreeID, f: &mut dyn FnMut(TreeID, &Node)) {
        for child in self.children(parent) {
            if let Ok(n) = self.node(child) {
                f(child, &n);
                self.walk(child, f);
            }
        }
    }

    /// Scenes in manuscript order.
    pub fn manuscript_scenes(&self) -> Vec<TreeID> {
        let mut out = Vec::new();
        self.walk(self.root(Space::Manuscript), &mut |id, n| {
            if n.kind() == NodeKind::Scene {
                out.push(id)
            }
        });
        out
    }

    pub fn export_snapshot(&self) -> Result<Vec<u8>> {
        self.commit_meta();
        Ok(self.doc.export(loro::ExportMode::Snapshot)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_copies_body_marks_meta_and_children() {
        let p = Project::new_in_memory("Dup").unwrap();
        let root = p.root(Space::Manuscript);
        let ch = p.create_node(root, NodeKind::Chapter, "One").unwrap();
        let after = p.create_node(root, NodeKind::Chapter, "Two").unwrap();
        let sc = p.create_node(ch, NodeKind::Scene, "Opening").unwrap();
        let scene = p.node(sc).unwrap();
        let body = scene.body().unwrap();
        body.insert(0, "Hello world").unwrap();
        body.mark(0..5, "bold", true).unwrap();
        scene.add_tag("pov").unwrap();
        scene.set_status(Status::Revised).unwrap();
        scene.set_word_goal(Some(1200)).unwrap();

        let dup = p.duplicate_node(ch).unwrap();
        let order = p.children(root);
        assert_eq!(order, vec![ch, dup, after], "copy sits right after the original");
        let d = p.node(dup).unwrap();
        assert_eq!(d.title(), "One copy");
        assert_eq!(d.kind(), NodeKind::Chapter);
        let kids = p.children(dup);
        assert_eq!(kids.len(), 1);
        let dsc = p.node(kids[0]).unwrap();
        assert_eq!(dsc.title(), "Opening");
        assert_eq!(dsc.tags(), vec!["pov".to_string()]);
        assert_eq!(dsc.status(), Status::Revised);
        assert_eq!(dsc.word_goal(), Some(1200));
        assert_eq!(dsc.plain_text(), scene.plain_text());
        let (a, b) = (dsc.body().unwrap().to_delta(), body.to_delta());
        assert_eq!(format!("{a:?}"), format!("{b:?}"), "marks survive the copy");
        // Independent bodies: editing the copy leaves the original alone.
        dsc.body().unwrap().insert(0, "X").unwrap();
        assert_ne!(dsc.plain_text(), scene.plain_text());
    }

    #[test]
    fn descendant_and_move_into() {
        let p = Project::new_in_memory("Mv").unwrap();
        let root = p.root(Space::Manuscript);
        let a = p.create_node(root, NodeKind::Chapter, "A").unwrap();
        let b = p.create_node(root, NodeKind::Chapter, "B").unwrap();
        let s = p.create_node(a, NodeKind::Scene, "S").unwrap();
        assert!(p.is_descendant(s, a));
        assert!(p.is_descendant(a, a));
        assert!(!p.is_descendant(a, s));
        p.move_into(s, b).unwrap();
        assert_eq!(p.children(b), vec![s]);
        assert!(p.children(a).is_empty());
    }

    #[test]
    fn new_project_has_roots_and_trash() {
        let p = Project::new_in_memory("Test").unwrap();
        assert_eq!(p.name(), "Test");
        for s in [Space::Manuscript, Space::World, Space::Notes] {
            let r = p.node(p.root(s)).unwrap();
            assert_eq!(r.kind(), NodeKind::Root);
            assert_eq!(r.space(), s);
            assert_eq!(p.node(p.trash(s)).unwrap().kind(), NodeKind::Trash);
        }
        assert_eq!(p.tree().roots().len(), 6);
    }

    #[test]
    fn create_hierarchy_and_body() {
        let p = Project::new_in_memory("T").unwrap();
        let act = p
            .create_node(p.root(Space::Manuscript), NodeKind::Act, "Act I")
            .unwrap();
        let ch = p.create_node(act, NodeKind::Chapter, "Ch 1").unwrap();
        let sc = p.create_node(ch, NodeKind::Scene, "Opening").unwrap();
        let scene = p.node(sc).unwrap();
        assert_eq!(scene.kind(), NodeKind::Scene);
        assert_eq!(scene.space(), Space::Manuscript);
        assert_eq!(scene.status(), Status::Draft);
        assert!(scene.include_in_compile());
        scene.body().unwrap().insert(0, "It was a dark night.").unwrap();
        assert_eq!(scene.plain_text(), "It was a dark night.");
        assert_eq!(scene.word_count(), 5);
        assert_eq!(p.manuscript_scenes(), vec![sc]);
        assert!(p.node(ch).unwrap().body_if_exists().is_none());
    }

    #[test]
    fn create_after_and_trash() {
        let p = Project::new_in_memory("T").unwrap();
        let root = p.root(Space::Manuscript);
        let a = p.create_node(root, NodeKind::Scene, "A").unwrap();
        let c = p.create_node(root, NodeKind::Scene, "C").unwrap();
        let b = p.create_node_after(a, NodeKind::Scene, "B").unwrap();
        assert_eq!(p.children(root), vec![a, b, c]);
        p.trash_node(b).unwrap();
        assert_eq!(p.children(root), vec![a, c]);
        assert_eq!(p.children(p.trash(Space::Manuscript)), vec![b]);
    }

    #[test]
    fn roundtrip_through_snapshot() {
        let p = Project::new_in_memory("Round").unwrap();
        let sc = p.create_node(p.root(Space::Manuscript), NodeKind::Scene, "S").unwrap();
        p.node(sc).unwrap().body().unwrap().insert(0, "hello").unwrap();
        let bytes = p.export_snapshot().unwrap();

        let q = LoroDoc::new();
        schema::configure_text_styles(&q);
        q.import(&bytes).unwrap();
        let q = Project {
            doc: q,
            dir: None,
            recovered_from: None,
            shallow_root: RefCell::new(None),
            last_mirror: Cell::new(None),
        };
        q.ensure_roots().unwrap();
        assert_eq!(q.name(), "Round");
        assert_eq!(q.tree().roots().len(), 6);
        assert_eq!(q.node(sc).unwrap().plain_text(), "hello");
    }

    #[test]
    fn corrupt_main_file_recovers_from_newest_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Novel");
        let p = Project::create(&dir, "Novel").unwrap();
        let sc = p.create_node(p.root(Space::Notes), NodeKind::Note, "Idea").unwrap();
        p.node(sc).unwrap().body().unwrap().insert(0, "kept").unwrap();
        p.save().unwrap();
        storage::backup(&dir).unwrap();
        // Edits after the backup are lost by design; the file is then torn.
        p.node(sc).unwrap().body().unwrap().insert(4, " lost").unwrap();
        p.save().unwrap();
        let main = storage::snapshot_path(&dir);
        let bytes = std::fs::read(&main).unwrap();
        std::fs::write(&main, &bytes[..bytes.len() / 2]).unwrap();

        let q = Project::open(&dir).unwrap();
        assert!(q.recovered_from().is_some(), "should report the backup used");
        assert_eq!(q.node(sc).unwrap().plain_text(), "kept");
        // The main file is good again and the damaged one was kept aside.
        assert!(Project::open(&dir).unwrap().recovered_from().is_none());
        let aside = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("project.loro.corrupt-"));
        assert!(aside);

        // No backup at all: a clear error, not a silent empty project.
        let dir2 = tmp.path().join("Other");
        Project::create(&dir2, "Other").unwrap();
        std::fs::write(storage::snapshot_path(&dir2), b"garbage").unwrap();
        assert!(Project::open(&dir2).is_err());
    }

    #[test]
    fn json_mirror_is_throttled_and_forced_on_quit() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Mirror");
        let p = Project::create(&dir, "Mirror").unwrap();
        let json = storage::json_path(&dir);
        let first = std::fs::read_to_string(&json).unwrap();
        assert!(first.contains("Mirror"), "create writes the mirror once");

        p.set_name("Renamed").unwrap();
        p.save().unwrap();
        // The snapshot is current, the mirror is not: it was written seconds ago.
        let reopened = Project::open(&dir).unwrap();
        assert_eq!(reopened.name(), "Renamed");
        assert_eq!(std::fs::read_to_string(&json).unwrap(), first);

        p.save_and_mirror().unwrap();
        let forced = std::fs::read_to_string(&json).unwrap();
        assert!(forced.contains("Renamed") && forced != first);

        // Once the interval has passed, a plain save refreshes it again.
        p.last_mirror.set(Some(
            Instant::now() - MIRROR_INTERVAL - std::time::Duration::from_secs(1),
        ));
        p.set_name("Again").unwrap();
        p.save().unwrap();
        assert!(std::fs::read_to_string(&json).unwrap().contains("Again"));
    }

    #[test]
    fn startup_pruning_compacts_year_old_history_once() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Old");
        std::fs::create_dir_all(dir.join("snapshots")).unwrap();
        // Loro never lets a commit's timestamp go below its predecessors',
        // so the old change has to be the very first one in the document.
        let doc = LoroDoc::new();
        doc.set_record_timestamp(true);
        schema::configure_text_styles(&doc);
        let long_ago = chrono::Utc::now().timestamp() - 400 * 86_400;
        doc.set_next_commit_timestamp(long_ago);
        doc.get_text("scratch").insert(0, "first draft").unwrap();
        doc.commit();
        let p = Project {
            doc,
            dir: Some(dir.clone()),
            recovered_from: None,
            shallow_root: RefCell::new(None),
            last_mirror: Cell::new(None),
        };
        p.init_schema("Old").unwrap();
        p.ensure_id().unwrap();
        let sc = p
            .create_node(p.root(Space::Manuscript), NodeKind::Scene, "One")
            .unwrap();
        let body = p.node(sc).unwrap().body().unwrap();
        for i in 0..20 {
            body.insert(body.len_unicode(), &format!("w{i} ")).unwrap();
            p.doc.commit();
        }
        body.insert(body.len_unicode(), "end").unwrap();
        p.doc.commit();
        p.save().unwrap();
        assert!(p.history_age_days().unwrap() >= 399, "{:?}", p.history_age_days());

        let report = p.prune_if_old().unwrap().expect("old history gets pruned");
        assert!(report.age_days >= 399);
        // The in-memory doc keeps its history until relaunch; the file shrinks now.
        assert!(
            report.after.shallow && report.after.file_bytes < report.before.file_bytes,
            "{report:?}"
        );
        assert_eq!(storage::backups(&dir).len(), 1, "the full file is backed up first");

        // Reopened: content intact, shallow, and nothing left to prune.
        let p2 = Project::open(&dir).unwrap();
        assert!(p2.doc.is_shallow());
        assert!(
            p2.node(sc).unwrap().plain_text().contains("w0 w1") && p2.node(sc).unwrap().plain_text().contains("end")
        );
        assert_eq!(p2.prune_if_old().unwrap(), None);
        assert_eq!(storage::backups(&dir).len(), 1);

        // A fresh project is left alone.
        let fresh = Project::create(&tmp.path().join("Fresh"), "Fresh").unwrap();
        assert_eq!(fresh.prune_if_old().unwrap(), None);
    }

    #[test]
    fn compact_history_keeps_content_and_stays_syncable() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Novel");
        let a = Project::create(&dir, "Novel").unwrap();
        let sc = a
            .create_node(a.root(Space::Manuscript), NodeKind::Scene, "One")
            .unwrap();
        let body = a.node(sc).unwrap().body().unwrap();
        for i in 0..50 {
            body.insert(body.len_unicode(), &format!("w{i} ")).unwrap();
            a.doc.commit();
        }
        a.save().unwrap();
        let before = a.history_stats();
        assert!(before.ops >= 50 && !before.shallow, "{before:?}");

        // The other machine is a copy of this one, in sync at the moment of compaction.
        let dir_b = tmp.path().join("Novel-B");
        std::fs::create_dir_all(&dir_b).unwrap();
        std::fs::copy(storage::snapshot_path(&dir), storage::snapshot_path(&dir_b)).unwrap();
        let b = Project::open(&dir_b).unwrap();

        let after = a.compact_history().unwrap();
        assert!(after.shallow);
        assert!(after.file_bytes < before.file_bytes, "{after:?} vs {before:?}");
        assert!(storage::backups(&dir).len() == 1, "full history backed up first");

        // Reopen: content intact, history gone, and further saves stay shallow.
        let a2 = Project::open(&dir).unwrap();
        assert!(a2.doc.is_shallow());
        assert!(
            a2.history_stats().ops < before.ops,
            "{:?} vs {before:?}",
            a2.history_stats()
        );
        assert!(a2.node(sc).unwrap().plain_text().contains("w0 w1 w2"));
        a2.node(sc).unwrap().body().unwrap().insert(0, "A: ").unwrap();
        a2.save().unwrap();
        let a3 = Project::open(&dir).unwrap();
        assert!(a3.doc.is_shallow());
        assert!(a3.node(sc).unwrap().plain_text().starts_with("A: "));

        // Concurrent edit on B, then exchange updates both ways.
        let bb = b.node(sc).unwrap().body().unwrap();
        bb.insert(bb.len_unicode(), "B end").unwrap();
        b.doc.commit();
        let to_b = a3.doc.export(ExportMode::updates(&b.doc.oplog_vv())).unwrap();
        let to_a = b.doc.export(ExportMode::updates(&a3.doc.oplog_vv())).unwrap();
        let st = b.doc.import(&to_b).unwrap();
        assert!(st.pending.is_none(), "B must be able to apply A's post-compaction ops");
        a3.import_bytes(&to_a).unwrap();
        assert_eq!(a3.node(sc).unwrap().plain_text(), b.node(sc).unwrap().plain_text());
        assert!(a3.node(sc).unwrap().plain_text().starts_with("A: "));
        assert!(a3.node(sc).unwrap().plain_text().ends_with("B end"));
    }

    #[test]
    fn save_and_open_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Novel");
        let p = Project::create(&dir, "Novel").unwrap();
        let sc = p.create_node(p.root(Space::Notes), NodeKind::Note, "Idea").unwrap();
        p.node(sc).unwrap().body().unwrap().insert(0, "note text").unwrap();
        p.save().unwrap();
        assert!(dir.join("project.loro").exists());
        assert!(dir.join("project.json").exists());

        let q = Project::open(&dir).unwrap();
        assert_eq!(q.name(), "Novel");
        assert_eq!(q.node(sc).unwrap().plain_text(), "note text");
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("project.json")).unwrap()).unwrap();
        assert_eq!(json["project"]["name"], "Novel");
    }

    #[test]
    fn marks_expand_as_configured() {
        let p = Project::new_in_memory("M").unwrap();
        let sc = p.create_node(p.root(Space::Manuscript), NodeKind::Scene, "S").unwrap();
        let t = p.node(sc).unwrap().body().unwrap();
        t.insert(0, "abc").unwrap();
        t.mark(0..3, "bold", true).unwrap();
        t.insert(3, "d").unwrap(); // bold expands after
        t.mark(0..2, "link", "x").unwrap();
        t.insert(2, "Z").unwrap(); // link does not expand
        let delta = t.to_delta();
        let mut runs = Vec::new();
        for d in delta {
            if let loro::TextDelta::Insert { insert, attributes } = d {
                let mut keys: Vec<String> = attributes.unwrap_or_default().keys().cloned().collect();
                keys.sort();
                runs.push((insert, keys));
            }
        }
        assert_eq!(
            runs,
            vec![
                ("ab".to_string(), vec!["bold".to_string(), "link".to_string()]),
                ("Zcd".to_string(), vec!["bold".to_string()]),
                ("\n".to_string(), vec![]),
            ]
        );
    }

    #[test]
    fn formatted_scene_round_trips_through_save_and_load() {
        use crate::Paragraphs;
        let dir = std::env::temp_dir().join(format!("wordy-rt-{}", ulid::Ulid::new()));
        let p = Project::create(&dir, "RT").unwrap();
        let scene = p.create_node(p.root(Space::Manuscript), NodeKind::Scene, "S").unwrap();
        let body = p.node(scene).unwrap().body().unwrap();
        body.insert(0, "Title\nSome bold and italic text.\n* * *\n").unwrap();
        body.mark(5..6, "block", "h1").unwrap();
        body.mark(11..15, "bold", true).unwrap();
        body.mark(20..26, "italic", true).unwrap();
        body.mark(11..26, "highlight", true).unwrap();
        body.mark(16..19, "comment", "01ABC").unwrap();
        body.mark(38..39, "block", "break").unwrap();
        let before = Paragraphs::from_text(&body);
        p.save().unwrap();

        let q = Project::open(&dir).unwrap();
        let body2 = q.node(scene).unwrap().body().unwrap();
        let after = Paragraphs::from_text(&body2);
        assert_eq!(before, after);
        assert_eq!(after.get(0).unwrap().block, crate::Block::H1);
        assert_eq!(after.get(2).unwrap().block, crate::Block::Break);
        let runs = &after.get(1).unwrap().runs;
        assert!(runs.iter().any(|r| r.marks.bold && r.marks.highlight.is_some()));
        assert!(runs.iter().any(|r| r.marks.comment.as_deref() == Some("01ABC")));
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod entity_tests {
    use super::*;
    use crate::node::Relation;

    #[test]
    fn fields_relations_attachments_round_trip() {
        let p = Project::new_in_memory("t").unwrap();
        let world = p.root(Space::World);
        let a = p.create_node(world, NodeKind::Entity, "Anna").unwrap();
        let b = p.create_node(world, NodeKind::Entity, "Bram").unwrap();
        let na = p.node(a).unwrap();
        na.set_field("role", "Protagonist").unwrap();
        na.add_alias("Annie").unwrap();
        na.add_relation(b, "sibling", "older").unwrap();
        na.add_attachment("face.png", "abc.png", "image/png").unwrap();
        p.commit_meta();
        let bytes = p.export_snapshot().unwrap();
        let q = Project::new_in_memory("t2").unwrap();
        q.import_bytes(&bytes).unwrap();
        let n = q.node(a).unwrap();
        assert_eq!(n.field("role"), "Protagonist");
        assert_eq!(n.fields(), vec![("role".to_string(), "Protagonist".to_string())]);
        assert_eq!(
            n.relations(),
            vec![Relation {
                to: b,
                kind: "sibling".into(),
                note: "older".into()
            }]
        );
        assert_eq!(n.attachments()[0].path, "abc.png");
        n.set_field("role", "").unwrap();
        assert!(n.fields().is_empty());
        n.remove_relation(0).unwrap();
        assert!(n.relations().is_empty());
        let names = q.entity_names();
        assert_eq!(names.len(), 2);
        assert_eq!(names[0].names, vec!["Anna".to_string(), "Annie".to_string()]);
        assert!(q.is_live(a));
        q.trash_node(a).unwrap();
        assert!(!q.is_live(a));
        assert_eq!(q.entities(), vec![b]);
    }
}
