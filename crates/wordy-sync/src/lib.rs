//! wordy-sync: the app's side of the Wordy account and project rooms.
//!
//! [`cloud`] links a machine to an account (device-code flow, bearer
//! token), keeps a project's room open over a websocket and fetches copies
//! of projects the account can reach. [`config`] is the per-machine
//! `sync.json`. Nothing here touches the live document directly: the room
//! thread hands Loro updates to the app, which imports them on the UI
//! thread so editors see the events.

pub mod cloud;
pub mod config;
mod util;

pub use cloud::CloudAccount;
pub use config::SyncConfig;
/// The CRDT crate, re-exported so room users share one version.
pub use loro;
