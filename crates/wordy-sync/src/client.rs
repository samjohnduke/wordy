//! The side that pressed Sync: connect to a peer and run the session.

use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::session::{self, Role};
use crate::{LocalState, SyncOutcome};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

/// Connect to `addr` and sync. Blocking; run it on a background thread.
pub fn sync_with(addr: SocketAddr, state: &LocalState) -> Result<SyncOutcome> {
    let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).with_context(|| format!("connecting to {addr}"))?;
    session::run(stream, Role::Initiator, state)
}
