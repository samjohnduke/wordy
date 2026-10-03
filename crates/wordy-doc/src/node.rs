//! Typed view over one tree node's meta map.

use anyhow::{anyhow, Result};
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

    pub fn plain_text(&self) -> String {
        self.body_if_exists().map(|t| t.to_string()).unwrap_or_default()
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
