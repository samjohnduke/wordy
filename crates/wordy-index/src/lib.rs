//! wordy-index: a derived SQLite index over a project. Safe to delete; rebuilt
//! from the Loro document. Holds per-node word counts, backlinks (explicit
//! link marks and auto-detected mentions), tags, and an FTS5 body index.

use std::path::Path;

use anyhow::Result;
use rusqlite::{params, Connection};
use wordy_doc::{Matcher, Node, Paragraphs, Project, TreeID};

pub const FILE_NAME: &str = "index.sqlite";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    Explicit,
    Auto,
}

impl LinkKind {
    fn as_str(self) -> &'static str {
        match self {
            LinkKind::Explicit => "explicit",
            LinkKind::Auto => "auto",
        }
    }
    fn parse(s: &str) -> LinkKind {
        if s == "explicit" {
            LinkKind::Explicit
        } else {
            LinkKind::Auto
        }
    }
}

/// One node that references an entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backlink {
    pub node: TreeID,
    pub title: String,
    pub space: String,
    pub kind: LinkKind,
    pub count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub node: TreeID,
    pub title: String,
    pub space: String,
    pub snippet: String,
}

pub struct Index {
    conn: Connection,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS nodes (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    space TEXT NOT NULL,
    title TEXT NOT NULL,
    status TEXT NOT NULL,
    words INTEGER NOT NULL,
    updated INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS backlinks (
    entity_id TEXT NOT NULL,
    node_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY (entity_id, node_id, kind)
);
CREATE INDEX IF NOT EXISTS backlinks_node ON backlinks(node_id);
CREATE TABLE IF NOT EXISTS tags (
    node_id TEXT NOT NULL,
    tag TEXT NOT NULL,
    PRIMARY KEY (node_id, tag)
);
CREATE VIRTUAL TABLE IF NOT EXISTS fts_body USING fts5(id UNINDEXED, title, body, tokenize='unicode61');
CREATE TABLE IF NOT EXISTS daily_words (
    date TEXT PRIMARY KEY,
    words INTEGER NOT NULL
);
";

impl Index {
    pub fn open(dir: &Path) -> Result<Self> {
        let conn = Connection::open(dir.join(FILE_NAME))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        Self::with_conn(conn)
    }

    pub fn in_memory() -> Result<Self> {
        Self::with_conn(Connection::open_in_memory()?)
    }

    fn with_conn(conn: Connection) -> Result<Self> {
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// Drop everything derived and re-index every live node.
    pub fn rebuild(&mut self, project: &Project, matcher: &Matcher) -> Result<()> {
        let started = std::time::Instant::now();
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            "DELETE FROM nodes; DELETE FROM backlinks; DELETE FROM tags; DELETE FROM fts_body;",
        )?;
        let mut n = 0usize;
        for id in project.all_nodes() {
            if let Ok(node) = project.node(id) {
                index_node(&tx, &node, matcher)?;
                n += 1;
            }
        }
        tx.commit()?;
        tracing::debug!("index rebuilt: {n} nodes in {:?}", started.elapsed());
        Ok(())
    }

    /// Re-index one node (after its body or metadata changed).
    pub fn update_node(&mut self, project: &Project, id: TreeID, matcher: &Matcher) -> Result<()> {
        let tx = self.conn.transaction()?;
        remove_node(&tx, id)?;
        if project.is_live(id) {
            if let Ok(node) = project.node(id) {
                index_node(&tx, &node, matcher)?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn remove_node(&mut self, id: TreeID) -> Result<()> {
        let tx = self.conn.transaction()?;
        remove_node(&tx, id)?;
        tx.commit()?;
        Ok(())
    }

    /// Nodes referencing `entity`, explicit links first, then by title.
    pub fn appears_in(&self, entity: TreeID) -> Result<Vec<Backlink>> {
        let mut stmt = self.conn.prepare(
            "SELECT b.node_id, n.title, n.space, b.kind, b.count FROM backlinks b
             JOIN nodes n ON n.id = b.node_id
             WHERE b.entity_id = ?1
             ORDER BY CASE b.kind WHEN 'explicit' THEN 0 ELSE 1 END, n.space, n.title",
        )?;
        let rows = stmt.query_map(params![entity.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, title, space, kind, count) = row?;
            if let Ok(node) = TreeID::try_from(id.as_str()) {
                out.push(Backlink {
                    node,
                    title,
                    space,
                    kind: LinkKind::parse(&kind),
                    count,
                });
            }
        }
        // Merge explicit+auto rows for the same node into one entry.
        let mut merged: Vec<Backlink> = Vec::new();
        for b in out {
            if let Some(m) = merged.iter_mut().find(|m| m.node == b.node) {
                m.count += b.count;
                if b.kind == LinkKind::Explicit {
                    m.kind = LinkKind::Explicit;
                }
            } else {
                merged.push(b);
            }
        }
        Ok(merged)
    }

    /// Entities a node references (for "mentions" filters).
    pub fn entities_in(&self, node: TreeID) -> Result<Vec<TreeID>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT entity_id FROM backlinks WHERE node_id = ?1")?;
        let rows = stmt.query_map(params![node.to_string()], |r| r.get::<_, String>(0))?;
        Ok(rows
            .flatten()
            .filter_map(|s| TreeID::try_from(s.as_str()).ok())
            .collect())
    }

    /// Full-text search over titles and bodies. Each term matches as a prefix.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(|t| format!("\"{}\"*", t.replace('"', "\"\"")))
            .collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let q = terms.join(" ");
        let mut stmt = self.conn.prepare(
            "SELECT f.id, n.title, n.space, snippet(fts_body, 2, '\u{1}', '\u{2}', '…', 14)
             FROM fts_body f JOIN nodes n ON n.id = f.id
             WHERE fts_body MATCH ?1
             ORDER BY bm25(fts_body) LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![q, limit as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, title, space, snippet) = row?;
            if let Ok(node) = TreeID::try_from(id.as_str()) {
                out.push(SearchHit {
                    node,
                    title,
                    space,
                    snippet,
                });
            }
        }
        Ok(out)
    }

    pub fn word_count(&self, id: TreeID) -> Result<Option<i64>> {
        let mut stmt = self.conn.prepare("SELECT words FROM nodes WHERE id = ?1")?;
        let mut rows = stmt.query(params![id.to_string()])?;
        Ok(match rows.next()? {
            Some(r) => Some(r.get(0)?),
            None => None,
        })
    }

    pub fn nodes_with_tag(&self, tag: &str) -> Result<Vec<TreeID>> {
        let mut stmt = self
            .conn
            .prepare("SELECT node_id FROM tags WHERE tag = ?1")?;
        let rows = stmt.query_map(params![tag], |r| r.get::<_, String>(0))?;
        Ok(rows
            .flatten()
            .filter_map(|s| TreeID::try_from(s.as_str()).ok())
            .collect())
    }

    pub fn set_daily_words(&self, date: &str, words: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO daily_words(date, words) VALUES (?1, ?2)
             ON CONFLICT(date) DO UPDATE SET words = excluded.words",
            params![date, words],
        )?;
        Ok(())
    }

    pub fn daily_words(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT date, words FROM daily_words ORDER BY date")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        Ok(rows.flatten().collect())
    }
}

fn remove_node(tx: &rusqlite::Transaction<'_>, id: TreeID) -> Result<()> {
    let s = id.to_string();
    tx.execute("DELETE FROM nodes WHERE id = ?1", params![s])?;
    tx.execute("DELETE FROM backlinks WHERE node_id = ?1", params![s])?;
    tx.execute("DELETE FROM tags WHERE node_id = ?1", params![s])?;
    tx.execute("DELETE FROM fts_body WHERE id = ?1", params![s])?;
    Ok(())
}

fn index_node(tx: &rusqlite::Transaction<'_>, node: &Node, matcher: &Matcher) -> Result<()> {
    let id = node.id.to_string();
    let plain = node.plain_text();
    let words = wordy_doc::count_words(&plain) as i64;
    tx.execute(
        "INSERT INTO nodes(id, kind, space, title, status, words, updated) VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![
            id,
            node.kind().as_str(),
            node.space().as_str(),
            node.title(),
            node.status().as_str(),
            words,
            node.created().unwrap_or(0),
        ],
    )?;
    for tag in node.tags() {
        tx.execute(
            "INSERT OR IGNORE INTO tags(node_id, tag) VALUES (?1, ?2)",
            params![id, tag],
        )?;
    }
    if node.kind().has_body() {
        tx.execute(
            "INSERT INTO fts_body(id, title, body) VALUES (?1, ?2, ?3)",
            params![id, node.title(), plain],
        )?;
    }

    // Backlinks.
    let mut explicit: std::collections::HashMap<String, i64> = Default::default();
    if let Some(body) = node.body_if_exists() {
        let paras = Paragraphs::from_text(&body);
        for p in paras.iter() {
            for r in &p.runs {
                if let Some(l) = &r.marks.link {
                    *explicit.entry(l.clone()).or_default() += 1;
                }
            }
        }
    }
    for (entity, count) in &explicit {
        tx.execute(
            "INSERT INTO backlinks(entity_id, node_id, kind, count) VALUES (?1,?2,?3,?4)",
            params![entity, id, LinkKind::Explicit.as_str(), count],
        )?;
    }
    let mut auto: std::collections::HashMap<TreeID, i64> = Default::default();
    for m in matcher.scan_excluding(&plain, Some(node.id)) {
        if !m.is_ambiguous() {
            *auto.entry(m.candidates[0]).or_default() += 1;
        }
    }
    for (entity, count) in auto {
        let e = entity.to_string();
        if explicit.contains_key(&e) {
            continue;
        }
        tx.execute(
            "INSERT INTO backlinks(entity_id, node_id, kind, count) VALUES (?1,?2,?3,?4)",
            params![e, id, LinkKind::Auto.as_str(), count],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wordy_doc::{NodeKind, Space};

    #[test]
    fn backlinks_and_search() {
        let p = Project::new_in_memory("t").unwrap();
        let world = p.root(Space::World);
        let anna = p.create_node(world, NodeKind::Entity, "Anna").unwrap();
        let bram = p.create_node(world, NodeKind::Entity, "Bram").unwrap();
        let ms = p.root(Space::Manuscript);
        let scene = p.create_node(ms, NodeKind::Scene, "Opening").unwrap();
        let body = p.node(scene).unwrap().body().unwrap();
        body.insert(0, "Anna met Bram. Then anna left.\n").unwrap();
        // Explicit link on "Bram".
        body.mark(9..13, "link", bram.to_string()).unwrap();
        p.commit_meta();

        let matcher = Matcher::new(&p.entity_names());
        let mut ix = Index::in_memory().unwrap();
        ix.rebuild(&p, &matcher).unwrap();

        let a = ix.appears_in(anna).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].node, scene);
        assert_eq!(a[0].kind, LinkKind::Auto);
        assert_eq!(a[0].count, 2);
        let b = ix.appears_in(bram).unwrap();
        assert_eq!(b[0].kind, LinkKind::Explicit);
        assert_eq!(ix.word_count(scene).unwrap(), Some(6));

        let hits = ix.search("bra", 10).unwrap();
        assert_eq!(hits.len(), 2, "scene body and Bram's title");
        let hits = ix.search("left", 10).unwrap();
        assert_eq!(hits[0].node, scene);
        assert!(hits[0].snippet.contains("\u{1}left\u{2}"));

        // Trash the scene: incremental update removes it.
        p.trash_node(scene).unwrap();
        ix.update_node(&p, scene, &matcher).unwrap();
        assert!(ix.appears_in(anna).unwrap().is_empty());
        assert!(ix.search("left", 10).unwrap().is_empty());
    }
}
