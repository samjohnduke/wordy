//! A Wordy project: one LoroDoc plus typed access to its containers.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use loro::{CommitOptions, LoroDoc, LoroMap, LoroTree, LoroValue, TreeID, TreeParentId, ValueOrContainer};

use crate::comments::Comments;

use crate::node::{Node, NodeKind, Space, Status};
use crate::schema::{self, meta};
use crate::storage;

/// Commit origin for everything that is not a body edit. See [`Project::commit_meta`].
pub const META_ORIGIN: &str = "meta";

pub struct Project {
    pub doc: LoroDoc,
    /// Folder on disk; `None` for an in-memory project (tests).
    pub dir: Option<PathBuf>,
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
        let project = Self { doc, dir: None };
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
    pub fn open(dir: &Path) -> Result<Self> {
        let bytes = std::fs::read(storage::snapshot_path(dir))
            .with_context(|| format!("reading {}", storage::snapshot_path(dir).display()))?;
        let doc = LoroDoc::new();
        doc.set_record_timestamp(true);
        schema::configure_text_styles(&doc);
        doc.import(&bytes).map_err(|e| anyhow!("import snapshot: {e}"))?;
        let p = Self { doc, dir: Some(dir.to_path_buf()) };
        p.ensure_roots()?;
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

    pub fn save(&self) -> Result<()> {
        let Some(dir) = &self.dir else { return Ok(()) };
        self.commit_meta();
        storage::save(&self.doc, dir)
    }

    /// Import another copy of this project (e.g. from the other machine). Loro merges.
    pub fn import_bytes(&self, bytes: &[u8]) -> Result<()> {
        self.doc.import(bytes).map_err(|e| anyhow!("import: {e}"))?;
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
            .filter(|id| self.node(*id).map(|n| n.kind() == kind && n.space() == space).unwrap_or(false))
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
        self.project_map().ensure_mergeable_map(schema::project::SETTINGS).expect("settings map")
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
        self.project_map().insert(schema::project::ID, ulid::Ulid::new().to_string())?;
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
        self.find_root(space, NodeKind::Root).expect("roots exist after ensure_roots")
    }

    pub fn trash(&self, space: Space) -> TreeID {
        self.find_root(space, NodeKind::Trash).expect("trash exists after ensure_roots")
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
        let parent = self.node(sibling)?.parent().ok_or_else(|| anyhow!("sibling has no parent"))?;
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
        let act = p.create_node(p.root(Space::Manuscript), NodeKind::Act, "Act I").unwrap();
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
        let q = Project { doc: q, dir: None };
        q.ensure_roots().unwrap();
        assert_eq!(q.name(), "Round");
        assert_eq!(q.tree().roots().len(), 6);
        assert_eq!(q.node(sc).unwrap().plain_text(), "hello");
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
        assert!(runs.iter().any(|r| r.marks.bold && r.marks.highlight));
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
        assert_eq!(n.relations(), vec![Relation { to: b, kind: "sibling".into(), note: "older".into() }]);
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
