//! LAN sync glue: owns the listening server, mDNS discovery and the sync
//! settings, runs sessions off the UI thread, and applies what comes back
//! on it (so Loro events reach the editors).

use std::net::SocketAddr;
use std::time::Duration;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use futures::StreamExt as _;
use gpui_kit::*;
use wordy_sync::cloud::{self, CloudAccount, DeviceLink};
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
    Done {
        peer: String,
        summary: String,
        when: String,
    },
    Failed(String),
}

/// Where the cloud account link stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudStatus {
    Idle,
    /// Asking the server for a code.
    Starting,
    /// Code issued; waiting for the user to approve it in the browser.
    Waiting {
        user_code: String,
        verify_url: String,
    },
    /// Talking to the server (refresh, unlink, remove).
    Busy(String),
    Failed(String),
}

/// What a background cloud job sends back.
enum CloudMsg {
    Code(DeviceLink),
    Linked(Result<CloudAccount>),
    Account(Result<cloud::Account>, bool),
    Unlinked(Result<()>),
    Removed(Result<()>),
}

pub struct SyncManager {
    project: SharedProject,
    pub config: SyncConfig,
    pub cloud_status: CloudStatus,
    /// Linked machines as of the last refresh.
    pub cloud_devices: Vec<cloud::Device>,
    cloud_cancel: Option<Arc<AtomicBool>>,
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
            cloud_status: CloudStatus::Idle,
            cloud_devices: Vec::new(),
            cloud_cancel: None,
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
        if m.config.cloud.is_some() {
            m.refresh_cloud(true, cx);
        }
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
        let work = cx
            .background_executor()
            .spawn(async move { wordy_sync::sync_with(addr, &state) });
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
                let peer = if outcome.peer.is_empty() {
                    label.to_string()
                } else {
                    outcome.peer.clone()
                };
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

/// Cloud account: linking, refreshing and unlinking. The HTTP calls block,
/// so each runs on its own thread and reports back through a channel.
impl SyncManager {
    pub fn cloud_busy(&self) -> bool {
        matches!(
            self.cloud_status,
            CloudStatus::Starting | CloudStatus::Waiting { .. } | CloudStatus::Busy(_)
        )
    }

    pub fn set_cloud_server(&mut self, url: &str, cx: &mut Context<Self>) {
        let url = url.trim().trim_end_matches('/');
        if url == self.config.cloud_server || self.config.cloud.is_some() {
            return;
        }
        self.config.cloud_server = url.to_string();
        self.save_config();
        cx.notify();
    }

    fn cloud_client(&self) -> cloud::Client {
        cloud::Client::new(&self.config.cloud_server())
    }

    /// Run `job` on a thread; its messages arrive in `handle_cloud_msg`.
    fn spawn_cloud(&mut self, cx: &mut Context<Self>, job: impl FnOnce(&dyn Fn(CloudMsg)) + Send + 'static) {
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<CloudMsg>();
        std::thread::Builder::new()
            .name("wordy-cloud".into())
            .spawn(move || {
                let send = |m: CloudMsg| {
                    let _ = tx.unbounded_send(m);
                };
                job(&send);
            })
            .expect("spawn cloud thread");
        self._tasks.push(cx.spawn(async move |this, cx| {
            while let Some(msg) = rx.next().await {
                if this.update(cx, |m, cx| m.handle_cloud_msg(msg, cx)).is_err() {
                    break;
                }
            }
        }));
    }

    /// Start the device-code flow: the browser opens at the approval page
    /// once the server hands out a code.
    pub fn link_cloud(&mut self, cx: &mut Context<Self>) {
        if self.cloud_busy() || self.config.cloud.is_some() {
            return;
        }
        let keep = Arc::new(AtomicBool::new(true));
        self.cloud_cancel = Some(keep.clone());
        self.cloud_status = CloudStatus::Starting;
        cx.notify();
        let client = self.cloud_client();
        let name = self.config.peer_name.clone();
        let version = env!("CARGO_PKG_VERSION").to_string();
        self.spawn_cloud(cx, move |send| {
            let result = client.link(
                &name,
                &version,
                |link| send(CloudMsg::Code(link.clone())),
                || keep.load(Ordering::Relaxed),
            );
            send(CloudMsg::Linked(result));
        });
    }

    pub fn cancel_link(&mut self, cx: &mut Context<Self>) {
        if let Some(flag) = self.cloud_cancel.take() {
            flag.store(false, Ordering::Relaxed);
        }
        if matches!(self.cloud_status, CloudStatus::Starting | CloudStatus::Waiting { .. }) {
            self.cloud_status = CloudStatus::Idle;
            cx.notify();
        }
    }

    /// Re-open the approval page (the user closed the tab).
    pub fn open_verify_url(&self, cx: &mut Context<Self>) {
        if let CloudStatus::Waiting { verify_url, .. } = &self.cloud_status {
            cx.open_url(verify_url);
        }
    }

    /// Fetch who we are and the linked machines. `quiet` swallows network
    /// errors (used at launch, when being offline is normal).
    pub fn refresh_cloud(&mut self, quiet: bool, cx: &mut Context<Self>) {
        let Some(acct) = self.config.cloud.clone() else { return };
        if self.cloud_busy() {
            return;
        }
        if !quiet {
            self.cloud_status = CloudStatus::Busy("Checking the account…".into());
            cx.notify();
        }
        let client = self.cloud_client();
        self.spawn_cloud(cx, move |send| {
            send(CloudMsg::Account(client.account(&acct.token), quiet))
        });
    }

    /// Forget the account here and end the session on the server. The local
    /// side always succeeds; a server failure is only reported.
    pub fn unlink_cloud(&mut self, cx: &mut Context<Self>) {
        let Some(acct) = self.config.cloud.take() else { return };
        self.cloud_devices.clear();
        self.save_config();
        self.cloud_status = CloudStatus::Busy("Unlinking…".into());
        cx.notify();
        let client = self.cloud_client();
        self.spawn_cloud(cx, move |send| send(CloudMsg::Unlinked(client.unlink(&acct.token))));
    }

    /// Remove another linked machine.
    pub fn remove_cloud_device(&mut self, device_id: &str, cx: &mut Context<Self>) {
        let Some(acct) = self.config.cloud.clone() else { return };
        if self.cloud_busy() {
            return;
        }
        self.cloud_status = CloudStatus::Busy("Removing the device…".into());
        cx.notify();
        let client = self.cloud_client();
        let id = device_id.to_string();
        self.spawn_cloud(cx, move |send| {
            send(CloudMsg::Removed(client.revoke_device(&acct.token, &id)))
        });
    }

    /// The server said our token is dead: drop it locally.
    fn cloud_revoked(&mut self) {
        self.config.cloud = None;
        self.cloud_devices.clear();
        self.save_config();
        self.cloud_status =
            CloudStatus::Failed("This machine was unlinked on the website. Link it again to sign in.".into());
    }

    fn handle_cloud_msg(&mut self, msg: CloudMsg, cx: &mut Context<Self>) {
        match msg {
            CloudMsg::Code(link) => {
                if !matches!(self.cloud_status, CloudStatus::Starting) {
                    return;
                }
                cx.open_url(&link.verify_url);
                self.cloud_status = CloudStatus::Waiting {
                    user_code: link.user_code,
                    verify_url: link.verify_url,
                };
            }
            CloudMsg::Linked(result) => {
                self.cloud_cancel = None;
                match result {
                    Ok(acct) => {
                        tracing::info!("linked to cloud account {}", acct.email);
                        self.config.cloud = Some(acct);
                        self.save_config();
                        self.cloud_status = CloudStatus::Idle;
                        self.refresh_cloud(false, cx);
                    }
                    Err(e) if e.to_string() == "cancelled" => self.cloud_status = CloudStatus::Idle,
                    Err(e) => {
                        tracing::warn!("cloud link failed: {e:#}");
                        self.cloud_status = CloudStatus::Failed(format!("Linking failed: {e:#}"));
                    }
                }
            }
            CloudMsg::Account(result, quiet) => match result {
                Ok(a) => {
                    if let Some(acct) = self.config.cloud.as_mut() {
                        if acct.email != a.user.email {
                            acct.email = a.user.email.clone();
                            self.save_config();
                        }
                    }
                    self.cloud_devices = a.devices;
                    self.cloud_status = CloudStatus::Idle;
                }
                Err(e) if cloud::is_unauthorized(&e) => self.cloud_revoked(),
                Err(e) => {
                    tracing::warn!("cloud account refresh failed: {e:#}");
                    self.cloud_status = if quiet {
                        CloudStatus::Idle
                    } else {
                        CloudStatus::Failed(format!("Could not reach the server: {e:#}"))
                    };
                }
            },
            CloudMsg::Unlinked(result) => {
                self.cloud_status = match result {
                    Ok(()) => CloudStatus::Idle,
                    Err(e) => CloudStatus::Failed(format!(
                        "Unlinked here, but the server could not be told ({e:#}). Remove this machine on the website too."
                    )),
                };
            }
            CloudMsg::Removed(result) => match result {
                Ok(()) => {
                    self.cloud_status = CloudStatus::Idle;
                    self.refresh_cloud(false, cx);
                }
                Err(e) if cloud::is_unauthorized(&e) => self.cloud_revoked(),
                Err(e) => self.cloud_status = CloudStatus::Failed(format!("{e:#}")),
            },
        }
        cx.notify();
    }
}

impl EventEmitter<SyncEvent> for SyncManager {}
