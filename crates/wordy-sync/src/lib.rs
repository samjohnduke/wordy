//! wordy-sync: manual LAN sync between two copies of a project.
//!
//! Both machines advertise `_wordy._tcp` over mDNS. The user presses Sync,
//! picks a peer, and the two sides exchange Loro updates (each sends what the
//! other is missing, by version vector), copy missing assets both ways and
//! union their custom dictionaries. Everything travels over one plain TCP
//! connection, authenticated with a pre-shared pairing code.
//!
//! Nothing here touches the live document. The UI thread builds a
//! [`LocalState`] (a snapshot of the saved doc plus the asset folder and the
//! dictionary), the sync thread works on that, and hands back a
//! [`SyncOutcome`] whose `incoming_updates` the UI imports on its own thread,
//! so Loro events keep firing where the editors live.

pub mod client;
pub mod config;
pub mod discovery;
pub mod protocol;
pub mod server;
mod session;

use std::path::PathBuf;

pub use client::sync_with;
pub use config::SyncConfig;
pub use discovery::{Advertiser, Discovery, Peer};
pub use server::{Server, ServerEvent};

/// The protocol version both sides must agree on.
pub const PROTOCOL_VERSION: u32 = 1;

/// Everything the sync thread needs from the local project, captured on the
/// UI thread right after a save.
#[derive(Debug, Clone)]
pub struct LocalState {
    /// `project.id` in the document; both sides must match.
    pub project_id: String,
    /// Human name shown to the other side ("Sam's laptop").
    pub peer_name: String,
    /// The pairing code both machines were given.
    pub pairing_code: String,
    /// A Loro snapshot of the saved document.
    pub snapshot: Vec<u8>,
    /// `<project>/assets`, may not exist yet.
    pub assets_dir: PathBuf,
    /// Custom dictionary words.
    pub dictionary: Vec<String>,
}

/// What one sync did, for the UI to apply and to summarise.
#[derive(Debug, Clone, Default)]
pub struct SyncOutcome {
    /// The other side's name.
    pub peer: String,
    /// Loro updates to import into the live doc (empty when nothing was new).
    pub incoming_updates: Vec<u8>,
    /// Bytes of updates received / sent.
    pub bytes_in: usize,
    pub bytes_out: usize,
    /// Number of changes received / sent (Loro ops, counted per peer range).
    pub ops_in: usize,
    pub ops_out: usize,
    /// Asset files copied here / sent over.
    pub files_in: Vec<String>,
    pub files_out: Vec<String>,
    /// Dictionary words the other side had that we did not.
    pub new_words: Vec<String>,
}

impl SyncOutcome {
    /// One line for the status bar: "12 changes in, 3 out · 1 file in · 2 words".
    pub fn summary(&self) -> String {
        let mut parts = vec![format!("{} changes in, {} out", self.ops_in, self.ops_out)];
        if !self.files_in.is_empty() || !self.files_out.is_empty() {
            parts.push(format!(
                "{} files in, {} out",
                self.files_in.len(),
                self.files_out.len()
            ));
        }
        if !self.new_words.is_empty() {
            parts.push(format!("{} new words", self.new_words.len()));
        }
        parts.join(" · ")
    }
}
