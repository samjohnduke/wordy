//! LAN sync glue: owns the listening server, mDNS discovery and the sync
//! settings, runs sessions off the UI thread, and applies what comes back
//! on it (so Loro events reach the editors).

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context as _, Result};
use futures::StreamExt as _;
use gpui_kit::*;
use wordy_sync::{Advertiser, Discovery, LocalState, Peer, Server, ServerEvent, SyncConfig, SyncOutcome};

use crate::app::SharedProject;

const PEER_POLL: Duration = Duration::from_millis(1500);

pub enum SyncEvent {
    /// A sync finished and its changes were imported; refresh everything.
    Applied(SyncOutcome),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncStatus {
    Idle,
    Busy(String),
    Done { peer: String, summary: String, when: String },
    Failed(String),
}

pub struct SyncManager {
    project: SharedProject,
    pub config: SyncConfig,
    server: Option<Server>,
    discovery: Option<Discovery>,
    advertiser: Option<Advertiser>,
    pub status: SyncStatus,
    pub peers: Vec<Peer>,
    peer_gen: u64,
    /// mDNS could not start; shown on the Sync page.
    pub discovery_error: Option<String>,
    _tasks: Vec<Task<()>>,
}

impl SyncManager {
    pub fn new(project: SharedProject, cx: &mut Context<Self>) -> Self {
        let config = SyncConfig::load();
        let mut tasks = Vec::new();

        // Server: its thread hands events to a channel; a foreground task
        // forwards them to us.
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<ServerEvent>();
        let server = match Server::start(config.port, move |ev| {
            let _ = tx.unbounded_send(ev);
        }) {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::error!("sync server: {e:#}");
                None
            }
        };
        tasks.push(cx.spawn(async move |this, cx| {
            while let Some(ev) = rx.next().await {
                if this.update(cx, |m, cx| m.handle_server_event(ev, cx)).is_err() {
                    break;
                }
            }
        }));

        let mut discovery_error = None;
        let discovery = match Discovery::start() {
            Ok(d) => Some(d),
            Err(e) => {
                tracing::error!("mdns: {e:#}");
                discovery_error = Some(format!("Peer discovery is off ({e:#}). Enter an address by hand."));
                None
            }
        };
        tasks.push(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(PEER_POLL).await;
            if this.update(cx, |m, cx| m.poll_peers(cx)).is_err() {
                break;
            }
        }));

        let mut m = Self {
            project,
            config,
            server,
            discovery,
            advertiser: None,
            status: SyncStatus::Idle,
            peers: Vec::new(),
            peer_gen: 0,
            discovery_error,
            _tasks: tasks,
        };
        m.advertise();
        m
    }

    pub fn port(&self) -> Option<u16> {
        self.server.as_ref().map(|s| s.port())
    }

    pub fn busy(&self) -> bool {
        matches!(self.status, SyncStatus::Busy(_))
    }

    pub fn project_id(&self) -> String {
        self.project.project.id()
    }

    fn advertise(&mut self) {
        self.advertiser = None;
        let Some(port) = self.port() else { return };
        let project_id = self.project.project.id();
        let Some(d) = self.discovery.as_mut() else {
            return;
        };
        match d.advertise(&project_id, &self.config.peer_name, port) {
            Ok(a) => self.advertiser = Some(a),
            Err(e) => tracing::error!("mdns advertise: {e:#}"),
        }
    }

    fn poll_peers(&mut self, cx: &mut Context<Self>) {
        let Some(d) = &self.discovery else { return };
        let g = d.generation();
        if g != self.peer_gen {
            self.peer_gen = g;
            self.peers = d.peers();
            cx.notify();
        }
    }

    pub fn set_peer_name(&mut self, name: &str, cx: &mut Context<Self>) {
        let name = name.trim();
        if name.is_empty() || name == self.config.peer_name {
            return;
        }
        self.config.peer_name = name.to_string();
        self.save_config();
        self.advertise();
        cx.notify();
    }

    pub fn set_pairing_code(&mut self, code: &str, cx: &mut Context<Self>) {
        let code = code.trim();
        if code == self.config.pairing_code {
            return;
        }
        self.config.pairing_code = code.to_string();
        self.save_config();
        cx.notify();
    }

    fn save_config(&self) {
        if let Err(e) = self.config.save() {
            tracing::error!("save sync config: {e:#}");
        }
    }

    /// Save, then capture what the sync thread needs.
    fn local_state(&self) -> Result<LocalState> {
        let p = &self.project.project;
        p.save().context("saving before sync")?;
        let dir = self.project.dir().cloned().context("this project has no folder")?;
        Ok(LocalState {
            project_id: p.id(),
            peer_name: self.config.peer_name.clone(),
            pairing_code: self.config.pairing_code.clone(),
            snapshot: p.export_snapshot()?,
            assets_dir: dir.join("assets"),
            dictionary: self.project.load_dictionary(),
        })
    }

    /// Connect to `addr` and sync (the "I pressed Sync" side).
    pub fn sync_with(&mut self, addr: SocketAddr, label: &str, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        if !self.config.ready() {
            self.status = SyncStatus::Failed("Set a pairing code first (the same on both machines).".into());
            cx.notify();
            return;
        }
        let state = match self.local_state() {
            Ok(s) => s,
            Err(e) => {
                self.status = SyncStatus::Failed(format!("{e:#}"));
                cx.notify();
                return;
            }
        };
        self.status = SyncStatus::Busy(format!("Syncing with {label}…"));
        cx.notify();
        let label = label.to_string();
        let work = cx.background_executor().spawn(async move { wordy_sync::sync_with(addr, &state) });
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |m, cx| m.finish(result, &label, cx)).ok();
        }));
    }

    fn handle_server_event(&mut self, ev: ServerEvent, cx: &mut Context<Self>) {
        match ev {
            ServerEvent::NeedState { peer, reply } => {
                let result = if self.busy() {
                    Err(anyhow::anyhow!("busy with another sync"))
                } else {
                    self.status = SyncStatus::Busy(format!("Syncing with {peer}…"));
                    cx.notify();
                    self.local_state()
                };
                if let Err(e) = &result {
                    self.status = SyncStatus::Failed(format!("{e:#}"));
                    cx.notify();
                }
                let _ = reply.send(result);
            }
            ServerEvent::Finished { peer, result } => {
                if self.busy() {
                    self.finish(result, &peer.to_string(), cx);
                }
            }
        }
    }

    /// Import the incoming updates, union the dictionary, save, report.
    fn finish(&mut self, result: Result<SyncOutcome>, label: &str, cx: &mut Context<Self>) {
        match result {
            Ok(outcome) => {
                let p = &self.project.project;
                if !outcome.incoming_updates.is_empty() {
                    if let Err(e) = p.import_bytes(&outcome.incoming_updates) {
                        self.status = SyncStatus::Failed(format!("applying changes from {label}: {e:#}"));
                        cx.notify();
                        return;
                    }
                }
                for w in &outcome.new_words {
                    self.project.append_dictionary_word(w);
                }
                if let Err(e) = p.save() {
                    tracing::error!("save after sync: {e:#}");
                }
                let peer = if outcome.peer.is_empty() { label.to_string() } else { outcome.peer.clone() };
                tracing::info!("synced with {peer}: {}", outcome.summary());
                self.status = SyncStatus::Done {
                    peer,
                    summary: outcome.summary(),
                    when: wordy_doc::chrono::Local::now().format("%H:%M").to_string(),
                };
                cx.emit(SyncEvent::Applied(outcome));
            }
            Err(e) => {
                tracing::warn!("sync with {label} failed: {e:#}");
                self.status = SyncStatus::Failed(format!("{e:#}"));
            }
        }
        cx.notify();
    }
}

impl EventEmitter<SyncEvent> for SyncManager {}
