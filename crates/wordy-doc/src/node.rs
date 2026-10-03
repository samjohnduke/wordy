//! Typed view over one tree node's meta map.

use anyhow::{anyhow, Result};
use std::collections::HashMap;

use loro::{LoroList, LoroMap, LoroText, LoroTree, LoroValue, TreeID, ValueOrContainer};
use serde::{Deserialize, Serialize};

use crate::schema::meta;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Space {
    Manuscript,
    World,
    Notes,
}

impl Space {
    pub fn as_str(self) -> &'static str {
        match self {
            Space::Manuscript => "manuscript",
            Space::World => "world",
            Space::Notes => "notes",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "manuscript" => Some(Space::Manuscript),
            "world" => Some(Space::World),
            "notes" => Some(Space::Notes),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Space::Manuscript => "Manuscript",
            Space::World => "World",
            Space::Notes => "Notes",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    /// Root of a space. Never shown as an item; never deleted.
    Root,
    Act,
    Chapter,
    Scene,
    Folder,
    Note,
    Entity,
    /// Trash root under a space. Deleted items are moved here.
    Trash,
}

impl NodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeKind::Root => "root",
            NodeKind::Act => "act",
            NodeKind::Chapter => "chapter",
            NodeKind::Scene => "scene",
            NodeKind::Folder => "folder",
            NodeKind::Note => "note",
            NodeKind::Entity => "entity",
            NodeKind::Trash => "trash",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "root" => NodeKind::Root,
            "act" => NodeKind::Act,
            "chapter" => NodeKind::Chapter,
            "scene" => NodeKind::Scene,
            "folder" => NodeKind::Folder,
            "note" => NodeKind::Note,
            "entity" => NodeKind::Entity,
            "trash" => NodeKind::Trash,
            _ => return None,
        })
    }
    /// Whether this node carries a rich-text body.
    pub fn has_body(self) -> bool {
        matches!(self, NodeKind::Scene | NodeKind::Note | NodeKind::Entity)
    }
    /// Whether children may be added under this node.
    pub fn is_container(self) -> bool {
        matches!(
            self,
            NodeKind::Root | NodeKind::Act | NodeKind::Chapter | NodeKind::Folder | NodeKind::Trash
        )
    }
    pub fn default_title(self) -> &'static str {
        match self {
            NodeKind::Act => "New Act",
            NodeKind::Chapter => "New Chapter",
            NodeKind::Scene => "New Scene",
            NodeKind::Folder => "New Folder",
            NodeKind::Note => "New Note",
            NodeKind::Entity => "New Entity",
            NodeKind::Root => "Root",
            NodeKind::Trash => "Trash",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Idea,
    #[default]
    Draft,
    Revised,
    Final,
}

impl Status {
    pub const ALL: [Status; 4] = [Status::Idea, Status::Draft, Status::Revised, Status::Final];
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Idea => "idea",
            Status::Draft => "draft",
            Status::Revised => "revised",
            Status::Final => "final",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "idea" => Status::Idea,
            "draft" => Status::Draft,
            "revised" => Status::Revised,
            "final" => Status::Final,
            _ => return None,
        })
    }
    pub fn label(self) -> &'static str {
        match self {
            Status::Idea => "Idea",
            Status::Draft => "Draft",
            Status::Revised => "Revised",
            Status::Final => "Final",
        }
    }
}

/// A typed relation from one entity to another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relation {
    pub to: TreeID,
    pub kind: String,
    pub note: String,
}

/// A file stored under the project's `assets/` directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    /// Path relative to `assets/`.
    pub path: String,
    pub mime: String,
}

/// A handle to one node. Cheap to clone; reads go straight to Loro.
#[derive(Clone)]
pub struct Node {
    pub id: TreeID,
    tree: LoroTree,
    meta: LoroMap,
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node")
            .field("id", &self.id.to_string())
            .field("kind", &self.kind())
            .field("title", &self.title())
            .finish()
    }
}

fn value_str(v: Option<ValueOrContainer>) -> Option<String> {
    match v {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => Some(s.to_string()),
        _ => None,
    }
}
fn value_i64(v: Option<ValueOrContainer>) -> Option<i64> {
    match v {
        Some(ValueOrContainer::Value(LoroValue::I64(i))) => Some(i),
        Some(ValueOrContainer::Value(LoroValue::Double(d))) => Some(d as i64),
        _ => None,
    }
}
fn value_bool(v: Option<ValueOrContainer>) -> Option<bool> {
    match v {
        Some(ValueOrContainer::Value(LoroValue::Bool(b))) => Some(b),
        _ => None,
    }
}

impl Node {
    pub(crate) fn new(tree: LoroTree, id: TreeID) -> Result<Self> {
        let meta = tree.get_meta(id).map_err(|e| anyhow!("node {id} has no meta: {e}"))?;
        Ok(Self { id, tree, meta })
    }

    pub fn meta(&self) -> &LoroMap {
        &self.meta
    }

    pub fn kind(&self) -> NodeKind {
        value_str(self.meta.get(meta::KIND))
            .and_then(|s| NodeKind::parse(&s))
            .unwrap_or(NodeKind::Note)
    }

    pub fn space(&self) -> Space {
        value_str(self.meta.get(meta::SPACE))
            .and_then(|s| Space::parse(&s))
            .unwrap_or(Space::Notes)
    }

    pub fn title(&self) -> String {
        value_str(self.meta.get(meta::TITLE)).unwrap_or_default()
    }

    pub fn set_title(&self, title: &str) -> Result<()> {
        self.meta.insert(meta::TITLE, title)?;
        Ok(())
    }

    pub fn status(&self) -> Status {
        value_str(self.meta.get(meta::STATUS))
            .and_then(|s| Status::parse(&s))
            .unwrap_or_default()
    }

    pub fn set_status(&self, status: Status) -> Result<()> {
        self.meta.insert(meta::STATUS, status.as_str())?;
        Ok(())
    }

    pub fn word_goal(&self) -> Option<i64> {
        value_i64(self.meta.get(meta::WORD_GOAL))
    }

    pub fn set_word_goal(&self, goal: Option<i64>) -> Result<()> {
        match goal {
            Some(g) => self.meta.insert(meta::WORD_GOAL, g)?,
            None => self.meta.delete(meta::WORD_GOAL)?,
        }
        Ok(())
    }

    pub fn include_in_compile(&self) -> bool {
        value_bool(self.meta.get(meta::INCLUDE_IN_COMPILE)).unwrap_or(true)
    }

    pub fn set_include_in_compile(&self, include: bool) -> Result<()> {
        self.meta.insert(meta::INCLUDE_IN_COMPILE, include)?;
        Ok(())
    }

    pub fn created(&self) -> Option<i64> {
        value_i64(self.meta.get(meta::CREATED))
    }

    /// The rich-text body. Created lazily so folders never carry one.
    pub fn body(&self) -> Result<LoroText> {
        Ok(self.meta.ensure_mergeable_text(meta::BODY)?)
    }

    /// Body if it exists; `None` for nodes that never had text.
    pub fn body_if_exists(&self) -> Option<LoroText> {
        match self.meta.get(meta::BODY) {
            Some(ValueOrContainer::Container(c)) => c.into_text().ok(),
            _ => None,
        }
    }

    /// Plain text of the body without the terminating newline.
    pub fn plain_text(&self) -> String {
        let mut s = self.body_if_exists().map(|t| t.to_string()).unwrap_or_default();
        if s.ends_with('\n') {
            s.pop();
        }
        s
    }

    /// Word count of the body.
    pub fn word_count(&self) -> usize {
        crate::paragraphs::count_words(&self.plain_text())
    }

    pub fn tags(&self) -> Vec<String> {
        self.string_list(meta::TAGS)
    }

    pub fn aliases(&self) -> Vec<String> {
        self.string_list(meta::ALIASES)
    }

    pub fn template(&self) -> Option<String> {
        value_str(self.meta.get(meta::TEMPLATE))
    }

    pub fn set_template(&self, template: &str) -> Result<()> {
        self.meta.insert(meta::TEMPLATE, template)?;
        Ok(())
    }

    fn string_list(&self, key: &str) -> Vec<String> {
        match self.meta.get(key) {
            Some(ValueOrContainer::Container(c)) => match c.into_list() {
                Ok(l) => l
                    .to_vec()
                    .into_iter()
                    .filter_map(|v| match v {
                        LoroValue::String(s) => Some(s.to_string()),
                        _ => None,
                    })
                    .collect(),
                Err(_) => vec![],
            },
            _ => vec![],
        }
    }

    fn list(&self, key: &str) -> Result<LoroList> {
        Ok(self.meta.ensure_mergeable_list(key)?)
    }

    pub fn add_tag(&self, tag: &str) -> Result<()> {
        if !self.tags().iter().any(|t| t == tag) {
            self.list(meta::TAGS)?.push(tag)?;
        }
        Ok(())
    }

    pub fn remove_tag(&self, tag: &str) -> Result<()> {
        let l = self.list(meta::TAGS)?;
        if let Some(i) = self.tags().iter().position(|t| t == tag) {
            l.delete(i, 1)?;
        }
        Ok(())
    }

    pub fn add_alias(&self, alias: &str) -> Result<()> {
        if !self.aliases().iter().any(|a| a == alias) {
            self.list(meta::ALIASES)?.push(alias)?;
        }
        Ok(())
    }

    pub fn remove_alias(&self, alias: &str) -> Result<()> {
        let l = self.list(meta::ALIASES)?;
        if let Some(i) = self.aliases().iter().position(|a| a == alias) {
            l.delete(i, 1)?;
        }
        Ok(())
    }

    /// Template-driven sheet fields (entities). Values are plain strings.
    pub fn field(&self, key: &str) -> String {
        match self.meta.get(meta::FIELDS) {
            Some(ValueOrContainer::Container(c)) => match c.into_map() {
                Ok(m) => value_str(m.get(key)).unwrap_or_default(),
                Err(_) => String::new(),
            },
            _ => String::new(),
        }
    }

    pub fn set_field(&self, key: &str, value: &str) -> Result<()> {
        let m = self.meta.ensure_mergeable_map(meta::FIELDS)?;
        if value.is_empty() {
            if m.get(key).is_some() {
                m.delete(key)?;
            }
        } else {
            m.insert(key, value)?;
        }
        Ok(())
    }

    /// Every stored field, including ones the current template does not show.
    pub fn fields(&self) -> Vec<(String, String)> {
        match self.meta.get(meta::FIELDS) {
            Some(ValueOrContainer::Container(c)) => match c.into_map() {
                Ok(m) => {
                    let mut v: Vec<(String, String)> = Vec::new();
                    m.for_each(|k, val| {
                        if let ValueOrContainer::Value(LoroValue::String(s)) = val {
                            v.push((k.to_string(), s.to_string()));
                        }
                    });
                    v.sort();
                    v
                }
                Err(_) => vec![],
            },
            _ => vec![],
        }
    }

    pub fn relations(&self) -> Vec<Relation> {
        self.record_list(meta::RELATIONS)
            .into_iter()
            .filter_map(|m| {
                let to = m.get("to").and_then(|v| v.as_string().map(|s| s.to_string()))?;
                let to = TreeID::try_from(to.as_str()).ok()?;
                Some(Relation {
                    to,
                    kind: m.get("kind").and_then(|v| v.as_string().map(|s| s.to_string())).unwrap_or_default(),
                    note: m.get("note").and_then(|v| v.as_string().map(|s| s.to_string())).unwrap_or_default(),
                })
            })
            .collect()
    }

    pub fn add_relation(&self, to: TreeID, kind: &str, note: &str) -> Result<()> {
        let mut m: HashMap<String, LoroValue> = HashMap::new();
        m.insert("to".into(), LoroValue::from(to.to_string()));
        m.insert("kind".into(), LoroValue::from(kind));
        m.insert("note".into(), LoroValue::from(note));
        self.list(meta::RELATIONS)?.push(LoroValue::Map(m.into()))?;
        Ok(())
    }

    pub fn remove_relation(&self, ix: usize) -> Result<()> {
        let l = self.list(meta::RELATIONS)?;
        if ix < l.len() {
            l.delete(ix, 1)?;
        }
        Ok(())
    }

    pub fn attachments(&self) -> Vec<Attachment> {
        self.record_list(meta::ATTACHMENTS)
            .into_iter()
            .map(|m| Attachment {
                name: m.get("name").and_then(|v| v.as_string().map(|s| s.to_string())).unwrap_or_default(),
                path: m.get("path").and_then(|v| v.as_string().map(|s| s.to_string())).unwrap_or_default(),
                mime: m.get("mime").and_then(|v| v.as_string().map(|s| s.to_string())).unwrap_or_default(),
            })
            .collect()
    }

    pub fn add_attachment(&self, name: &str, path: &str, mime: &str) -> Result<()> {
        let mut m: HashMap<String, LoroValue> = HashMap::new();
        m.insert("name".into(), LoroValue::from(name));
        m.insert("path".into(), LoroValue::from(path));
        m.insert("mime".into(), LoroValue::from(mime));
        self.list(meta::ATTACHMENTS)?.push(LoroValue::Map(m.into()))?;
        Ok(())
    }

    pub fn remove_attachment(&self, ix: usize) -> Result<()> {
        let l = self.list(meta::ATTACHMENTS)?;
        if ix < l.len() {
            l.delete(ix, 1)?;
        }
        Ok(())
    }

    /// A list of plain map values (relations, attachments).
    fn record_list(&self, key: &str) -> Vec<HashMap<String, LoroValue>> {
        match self.meta.get(key) {
            Some(ValueOrContainer::Container(c)) => match c.into_list() {
                Ok(l) => l
                    .to_vec()
                    .into_iter()
                    .filter_map(|v| match v {
                        LoroValue::Map(m) => Some((*m).clone().into_iter().collect()),
                        _ => None,
                    })
                    .collect(),
                Err(_) => vec![],
            },
            _ => vec![],
        }
    }

    pub fn parent(&self) -> Option<TreeID> {
        match self.tree.parent(self.id) {
            Some(loro::TreeParentId::Node(p)) => Some(p),
            _ => None,
        }
    }

    pub fn children(&self) -> Vec<TreeID> {
        self.tree.children(self.id).unwrap_or_default()
    }
}
