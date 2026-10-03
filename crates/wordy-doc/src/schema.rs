//! Container names, keys and mark configuration. The single place the schema is spelled out.

use loro::{ExpandType, InternalString, LoroDoc, StyleConfig, StyleConfigMap};

pub const SCHEMA_VERSION: i64 = 1;

// Root containers
pub const PROJECT: &str = "project";
pub const NODES: &str = "nodes";
pub const TASKS: &str = "tasks";
pub const COMMENTS: &str = "comments";
pub const VERSIONS: &str = "versions";
pub const SESSIONS: &str = "sessions";

// project map keys
pub mod project {
    /// Stable id shared by every copy of this project; sync refuses to mix two.
    pub const ID: &str = "id";
    pub const NAME: &str = "name";
    pub const CREATED: &str = "created";
    pub const SCHEMA: &str = "schema";
    pub const SETTINGS: &str = "settings";
}

// node meta keys
pub mod meta {
    pub const KIND: &str = "kind";
    pub const SPACE: &str = "space";
    pub const TITLE: &str = "title";
    pub const STATUS: &str = "status";
    pub const WORD_GOAL: &str = "word_goal";
    pub const TAGS: &str = "tags";
    pub const BODY: &str = "body";
    pub const TEMPLATE: &str = "template";
    pub const ALIASES: &str = "aliases";
    pub const FIELDS: &str = "fields";
    pub const RELATIONS: &str = "relations";
    pub const ATTACHMENTS: &str = "attachments";
    pub const INCLUDE_IN_COMPILE: &str = "include_in_compile";
    pub const CREATED: &str = "created";
}

/// Inline marks that extend when you type at their right edge.
pub const INLINE_EXPAND_AFTER: &[&str] = &[
    "bold",
    "italic",
    "underline",
    "strike",
    "smallcaps",
    "highlight",
];
/// Marks that never grow: links, comments, and the paragraph attributes on `\n`.
pub const EXPAND_NONE: &[&str] = &["link", "comment", "block", "align"];

/// Known paragraph block types stored in the `block` mark on a newline.
pub mod block {
    pub const PARAGRAPH: &str = "p";
    pub const H1: &str = "h1";
    pub const H2: &str = "h2";
    pub const H3: &str = "h3";
    pub const QUOTE: &str = "quote";
    pub const BREAK: &str = "break";
}

/// Apply the mark expansion config to a doc. Must run before any text edit and
/// again after every load, since the config is not part of the stored data.
pub fn configure_text_styles(doc: &LoroDoc) {
    let mut map = StyleConfigMap::new();
    for k in INLINE_EXPAND_AFTER {
        map.insert(
            InternalString::from(*k),
            StyleConfig {
                expand: ExpandType::After,
            },
        );
    }
    for k in EXPAND_NONE {
        map.insert(
            InternalString::from(*k),
            StyleConfig {
                expand: ExpandType::None,
            },
        );
    }
    doc.config_text_style(map);
    doc.config_default_text_style(Some(StyleConfig {
        expand: ExpandType::None,
    }));
}
