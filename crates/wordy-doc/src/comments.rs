//! Inline comments: the `comments` map keyed by id, with one small map per
//! comment. The anchor lives in the body text as a `comment` mark whose value
//! is the id; this module only knows about the metadata side.

use anyhow::Result;
use loro::{LoroMap, LoroValue, ValueOrContainer};

use crate::project::now_ms;

/// One comment's stored fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub id: String,
    pub text: String,
    pub created_ms: i64,
    pub resolved: bool,
    /// The node whose body carries the anchor, as a `TreeID` string.
    pub node: Option<String>,
}

/// Handle to the project's comments map.
#[derive(Clone)]
pub struct Comments {
    map: LoroMap,
}

mod keys {
    pub const TEXT: &str = "text";
    pub const CREATED: &str = "created";
    pub const RESOLVED: &str = "resolved";
    pub const NODE: &str = "node";
}

impl Comments {
    pub(crate) fn new(map: LoroMap) -> Self {
        Self { map }
    }

    /// Create a new comment and return its id (a ULID, also used as the mark value).
    pub fn add(&self, node: Option<&str>, text: &str) -> Result<String> {
        let id = ulid::Ulid::new().to_string();
        let m = self.map.insert_container(&id, LoroMap::new())?;
        m.insert(keys::TEXT, text)?;
        m.insert(keys::CREATED, now_ms())?;
        m.insert(keys::RESOLVED, false)?;
        if let Some(n) = node {
            m.insert(keys::NODE, n)?;
        }
        Ok(id)
    }

    fn entry(&self, id: &str) -> Option<LoroMap> {
        match self.map.get(id) {
            Some(ValueOrContainer::Container(c)) => c.into_map().ok(),
            _ => None,
        }
    }

    pub fn get(&self, id: &str) -> Option<Comment> {
        let m = self.entry(id)?;
        let str_of = |k: &str| match m.get(k) {
            Some(ValueOrContainer::Value(LoroValue::String(s))) => Some(s.to_string()),
            _ => None,
        };
        let created_ms = match m.get(keys::CREATED) {
            Some(ValueOrContainer::Value(LoroValue::I64(n))) => n,
            _ => 0,
        };
        let resolved = matches!(
            m.get(keys::RESOLVED),
            Some(ValueOrContainer::Value(LoroValue::Bool(true)))
        );
        Some(Comment {
            id: id.to_string(),
            text: str_of(keys::TEXT).unwrap_or_default(),
            created_ms,
            resolved,
            node: str_of(keys::NODE),
        })
    }

    pub fn set_text(&self, id: &str, text: &str) -> Result<()> {
        if let Some(m) = self.entry(id) {
            m.insert(keys::TEXT, text)?;
        }
        Ok(())
    }

    pub fn set_resolved(&self, id: &str, resolved: bool) -> Result<()> {
        if let Some(m) = self.entry(id) {
            m.insert(keys::RESOLVED, resolved)?;
        }
        Ok(())
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        self.map.delete(id)?;
        Ok(())
    }

    pub fn ids(&self) -> Vec<String> {
        self.map.keys().map(|k| k.to_string()).collect()
    }

    /// All comments anchored in `node`, oldest first.
    pub fn for_node(&self, node: &str) -> Vec<Comment> {
        let mut out: Vec<Comment> = self
            .ids()
            .iter()
            .filter_map(|id| self.get(id))
            .filter(|c| c.node.as_deref() == Some(node))
            .collect();
        out.sort_by_key(|c| c.created_ms);
        out
    }
}

#[cfg(test)]
mod tests {
    use crate::Project;

    #[test]
    fn add_edit_resolve_remove() {
        let p = Project::new_in_memory("T").unwrap();
        let c = p.comments();
        let id = c.add(Some("1@2"), "first").unwrap();
        assert_eq!(c.get(&id).unwrap().text, "first");
        c.set_text(&id, "second").unwrap();
        c.set_resolved(&id, true).unwrap();
        let got = c.get(&id).unwrap();
        assert_eq!(got.text, "second");
        assert!(got.resolved);
        assert_eq!(c.for_node("1@2").len(), 1);
        c.remove(&id).unwrap();
        assert!(c.get(&id).is_none());
    }
}
