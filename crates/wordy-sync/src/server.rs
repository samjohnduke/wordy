//! The listening side: accept one connection at a time, ask the app for the
//! current local state, run the session, report the result.

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

use crate::session::{self, Role};
use crate::{LocalState, SyncOutcome};

/// Sent to the app (on whatever thread the callback runs; forward to the UI).
pub enum ServerEvent {
    /// A peer connected. Save, build the state and send it back on `reply`.
    /// The server waits up to [`STATE_TIMEOUT`] for the answer.
    NeedState {
        peer: SocketAddr,
        reply: mpsc::Sender<Result<LocalState>>,
    },
    /// The session with `peer` ended. On success the outcome is yours to apply.
    Finished {
        peer: SocketAddr,
        result: Result<SyncOutcome>,
    },
}

pub const STATE_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
}

impl Server {
    /// Bind `port` (0 = any free port) on all interfaces and start accepting.
    pub fn start(
        port: u16,
        on_event: impl Fn(ServerEvent) + Send + Sync + 'static,
    ) -> Result<Self> {
        let listener =
            TcpListener::bind(("0.0.0.0", port)).with_context(|| format!("binding port {port}"))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        thread::Builder::new()
            .name("wordy-sync-server".into())
            .spawn(move || accept_loop(listener, stop2, Arc::new(on_event)))
            .context("spawning the sync server thread")?;
        tracing::info!("sync server listening on port {port}");
        Ok(Self { port, stop })
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn accept_loop(
    listener: TcpListener,
    stop: Arc<AtomicBool>,
    on_event: Arc<dyn Fn(ServerEvent) + Send + Sync>,
) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, peer)) => {
                // One session at a time; a second peer waits in the backlog.
                let _ = stream.set_nonblocking(false);
                let result = serve(stream, peer, &on_event);
                on_event(ServerEvent::Finished { peer, result });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(150))
            }
            Err(e) => {
                tracing::error!("sync accept: {e}");
                thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

fn serve(
    stream: TcpStream,
    peer: SocketAddr,
    on_event: &Arc<dyn Fn(ServerEvent) + Send + Sync>,
) -> Result<SyncOutcome> {
    tracing::info!("sync: connection from {peer}");
    let (tx, rx) = mpsc::channel();
    on_event(ServerEvent::NeedState { peer, reply: tx });
    let state = rx
        .recv_timeout(STATE_TIMEOUT)
        .map_err(|_| anyhow!("the app did not provide its state in time"))??;
    session::run(stream, Role::Responder, &state)
}
