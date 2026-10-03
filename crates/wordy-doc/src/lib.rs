//! wordy-doc: the Loro document model for a Wordy project.
//!
//! Everything stored lives in one `LoroDoc`. This crate owns the schema, typed
//! accessors over it, and save/load. Nothing here knows about the UI.

pub mod comments;
pub mod mentions;
pub mod momentum;
pub mod node;
pub mod paragraphs;
pub mod project;
pub mod schema;
pub mod storage;
pub mod templates;

pub use chrono;
pub use comments::{Comment, Comments};
pub use loro;
pub use loro::TreeID;
pub use mentions::{EntityNames, Matcher, Mention};
pub use momentum::{Goals, Placeholder, Session, Task, Version};
pub use node::{Attachment, Node, NodeKind, Relation, Space, Status};
pub use paragraphs::{count_words, Block, Highlight, Marks, Paragraph, Paragraphs, Run};
pub use project::Project;
pub use project::META_ORIGIN;
