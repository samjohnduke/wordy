//! A Wordy project: one LoroDoc plus typed access to its containers.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use loro::{LoroDoc, LoroMap, LoroTree, LoroValue, TreeID, TreeParentId, ValueOrContainer};

use crate::node::{Node, NodeKind, Space, Status};
use crate::schema::{self, meta};
use crate::storage;

pub struct Project {
    pub doc: LoroDoc,
    /// Folder on disk; `None` for an in-memory project (tests).
    pub dir: Option<PathBuf>,
}

fn now_ms() -> i64 {
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
        Ok(p)
    }

    /// Write the binary snapshot and the JSON mirror atomically.
    pub fn save(&self) -> Result<()> {
        let Some(dir) = &self.dir else { return Ok(()) };
        self.doc.commit();
        storage::save(&self.doc, dir)
    }

    /// Import another copy of this project (e.g. from the other machine). Loro merges.
    pub fn import_bytes(&self, bytes: &[u8]) -> Result<()> {
        self.doc.import(bytes).map_err(|e| anyhow!("import: {e}"))?;
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

    /// Make sure every space has a root and a trash. Idempotent.
    fn ensure_roots(&self) -> Result<()> {
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

    fn find_root(&self, space: Space, kind: NodeKind) -> Option<TreeID> {
        self.tree().roots().into_iter().find(|id| {
            self.node(*id)
                .map(|n| n.kind() == kind && n.space() == space)
                .unwrap_or(false)
        })
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
    pub fn comments(&self) -> LoroMap {
        self.doc.get_map(schema::COMMENTS)
    }
    pub fn versions(&self) -> LoroMap {
        self.doc.get_map(schema::VERSIONS)
    }
    pub fn sessions(&self) -> LoroMap {
        self.doc.get_map(schema::SESSIONS)
    }
    pub fn settings(&self) -> LoroMap {
        self.project_map()
            .ensure_mergeable_map(schema::project::SETTINGS)
            .expect("settings map")
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
        self.doc.commit();
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
                let mut keys: Vec<String> = attributes
                    .unwrap_or_default()
                    .keys()
                    .cloned()
                    .collect();
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
}
