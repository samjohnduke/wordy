//! wordy-doc: the Loro document model for a Wordy project.
//!
//! Everything stored lives in one `LoroDoc`. This crate owns the schema, typed
//! accessors over it, and save/load. Nothing here knows about the UI.

pub mod node;
pub mod project;
pub mod schema;
pub mod storage;

pub use loro;
pub use loro::TreeID;
pub use node::{Node, NodeKind, Space, Status};
pub use project::Project;
