//! Sync glue: owns the sync settings and the cloud account, and runs this
//! project's cloud room (one thread holding the websocket). Whatever comes
//! back is applied on the UI thread, so Loro events reach the editors.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use futures::StreamExt as _;
use gpui_kit::*;
use wordy_doc::{storage, Project};
use wordy_sync::cloud::state::absorb_remote;
use wordy_sync::cloud::{self, CloudAccount, CloudState, DeviceLink, RoomEvent, RoomHandle, RoomOptions};
use wordy_sync::loro::{ExportMode, VersionVector};
use wordy_sync::SyncConfig;

use crate::app::SharedProject;

/// Edits from the cloud wait for this long a pause in typing before they land.
const REMOTE_IDLE: Duration = Duration::from_millis(1200);
/// Past either of these the room's log is folded into a fresh snapshot.
const COMPACT_COUNT: u64 = 500;
const COMPACT_BYTES: u64 = 4 * 1024 * 1024;

pub enum SyncEvent {
    /// Edits from the cloud are waiting and the user has paused: the
    /// workspace calls `apply_cloud` so it can keep the carets in place.
    RemoteReady,
    /// Words arrived from the cloud; they are already in the dictionary file.
    CloudWords,
    /// Files under `assets/` came down from the cloud.
    CloudAssets,
}

/// Where this project's cloud sync stands, for the Account card and the
/// status bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudSyncStatus {
    /// Not turned on for this project (or no account).
    Off,
    Connecting,
    Live {
        /// Devices in the room, this one included.
        devices: usize,
        /// Something is still on its way up or down.
        pending: bool,
        /// Shared with this account as a reader: edits stay local.
        read_only: bool,
    },
    /// The socket is down; the room keeps retrying.
    Offline(String),
    /// Given up: the server refused the project.
    Failed(String),
}

/// This project's live connection to its room.
struct CloudSync {
    room: RoomHandle,
    state: CloudState,
    dir: PathBuf,
    /// Pushes awaiting their ack, oldest first: the version each one brings
    /// the room up to, its size, and whether it was a snapshot.
    in_flight: VecDeque<(VersionVector, usize, bool)>,
    /// Remote updates delivered but not imported yet.
    pending: Vec<(u64, Vec<u8>)>,
    /// Highest sequence number the room has delivered or acked to us.
    delivered: u64,
    connected: bool,
    synced: bool,
    role: String,
    log_count: u64,
    log_bytes: u64,
    /// Size of the last snapshot we uploaded (0 if none this session).
    base_bytes: u64,
    presence: Vec<String>,
    error: Option<String>,
    /// A fatal disconnect: the thread has given up.
    stopped: bool,
}

impl CloudSync {
    /// Persist the cursor; the delivered position counts only once
    /// everything delivered has been imported.
    fn save_state(&mut self) {
        if self.pending.is_empty() {
            self.state.last_seq = self.delivered;
        }
        if let Err(e) = self.state.save(&self.dir) {
            tracing::error!("save cloud state: {e:#}");
        }
    }

    fn status(&self) -> CloudSyncStatus {
        if self.stopped {
            return CloudSyncStatus::Failed(self.error.clone().unwrap_or_default());
        }
        if !self.connected {
            return match &self.error {
                Some(e) => CloudSyncStatus::Offline(e.clone()),
                None => CloudSyncStatus::Connecting,
            };
        }
        if !self.synced {
            return CloudSyncStatus::Connecting;
        }
        CloudSyncStatus::Live {
            devices: self.presence.len().max(1),
            pending: !self.in_flight.is_empty() || !self.pending.is_empty(),
            read_only: !self.writer(),
        }
    }

    /// Readers pull but never push; the room would close on them.
    fn writer(&self) -> bool {
        self.role != "reader"
    }
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
    Projects(Result<Vec<cloud::RemoteProject>>),
    /// A copy of a server project landed in the folder (or failed).
    Fetched(String, Result<PathBuf>),
    Unlinked(Result<()>),
    Removed(Result<()>),
    Room(RoomEvent),
}

pub struct SyncManager {
    project: SharedProject,
    pub config: SyncConfig,
    pub cloud_status: CloudStatus,
    /// Linked machines as of the last refresh.
    pub cloud_devices: Vec<cloud::Device>,
    /// Projects the account can reach, as of the last refresh.
    pub cloud_projects: Vec<cloud::RemoteProject>,
    /// Ids of the projects this machine already has a folder for.
    pub local_ids: Vec<String>,
    cloud_cancel: Option<Arc<AtomicBool>>,
    /// The project room, while cloud sync is on for this project.
    cloud: Option<CloudSync>,
    /// `cloud.json` says this project syncs (kept across unlink/relink).
    cloud_enabled: bool,
    /// When the user last typed, for the idle gap before remote edits land.
    last_edit: Option<Instant>,
    apply_scheduled: bool,
    apply_task: Option<Task<()>>,
    _tasks: Vec<Task<()>>,
}

impl SyncManager {
    pub fn new(project: SharedProject, cx: &mut Context<Self>) -> Self {
        let config = SyncConfig::load();
        let cloud_enabled = project.dir().map(|d| CloudState::load(d).enabled).unwrap_or(false);
        let mut m = Self {
            project,
            config,
            cloud_status: CloudStatus::Idle,
            cloud_devices: Vec::new(),
            cloud_projects: Vec::new(),
            local_ids: Vec::new(),
            cloud_cancel: None,
            cloud: None,
            cloud_enabled,
            last_edit: None,
            apply_scheduled: false,
            apply_task: None,
            _tasks: Vec::new(),
        };
        if m.config.cloud.is_some() {
            m.refresh_cloud(true, cx);
            m.start_room(cx);
        }
        m
    }

    /// Rename this machine; the name goes to the server on the next link.
    pub fn set_device_name(&mut self, name: &str, cx: &mut Context<Self>) {
        let name = name.trim();
        if name.is_empty() || name == self.config.device_name {
            return;
        }
        self.config.device_name = name.to_string();
        self.save_config();
        cx.notify();
    }

    fn save_config(&self) {
        if let Err(e) = self.config.save() {
            tracing::error!("save sync config: {e:#}");
        }
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
        let name = self.config.device_name.clone();
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
        self.local_ids = self.local_project_ids();
        let client = self.cloud_client();
        self.spawn_cloud(cx, move |send| {
            send(CloudMsg::Account(client.account(&acct.token), quiet));
            send(CloudMsg::Projects(client.list_projects(&acct.token)));
        });
    }

    /// The ids of every project folder under the projects root, plus the
    /// open project (which may live elsewhere).
    fn local_project_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = storage::list_projects(&storage::projects_root())
            .iter()
            .filter_map(|d| storage::mirrored_id(d))
            .collect();
        let own = self.project.project.id();
        if !own.is_empty() && !ids.contains(&own) {
            ids.push(own);
        }
        ids
    }

    /// Download a copy of a server project into the projects folder and
    /// open it in a new window. Its cloud sync is on from the start.
    pub fn fetch_cloud_project(&mut self, id: &str, name: &str, cx: &mut Context<Self>) {
        let Some(acct) = self.config.cloud.clone() else { return };
        if self.cloud_busy() {
            return;
        }
        let root = storage::projects_root();
        if let Err(e) = std::fs::create_dir_all(&root) {
            self.cloud_status = CloudStatus::Failed(format!("creating {}: {e}", root.display()));
            cx.notify();
            return;
        }
        let dir = storage::free_project_dir(&root, name);
        self.cloud_status = CloudStatus::Busy(format!("Downloading “{name}” to {}…", dir.display()));
        cx.notify();
        let server = self.config.cloud_server();
        let id = id.to_string();
        let name = name.to_string();
        self.spawn_cloud(cx, move |send| {
            let result = cloud::fetch_project(&server, &acct.token, &id, &name, &dir).map(|()| dir);
            send(CloudMsg::Fetched(id, result));
        });
    }

    /// Forget the account here and end the session on the server. The local
    /// side always succeeds; a server failure is only reported.
    pub fn unlink_cloud(&mut self, cx: &mut Context<Self>) {
        let Some(acct) = self.config.cloud.take() else { return };
        self.cloud = None;
        self.cloud_devices.clear();
        self.cloud_projects.clear();
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
        self.cloud = None;
        self.cloud_devices.clear();
        self.cloud_projects.clear();
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
                        self.start_room(cx);
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
            CloudMsg::Projects(result) => match result {
                Ok(list) => self.cloud_projects = list,
                // An expired token is reported by the account message.
                Err(e) if cloud::is_unauthorized(&e) => {}
                Err(e) => tracing::warn!("listing cloud projects failed: {e:#}"),
            },
            CloudMsg::Fetched(id, result) => match result {
                Ok(dir) => {
                    self.local_ids.push(id);
                    match Project::open(&dir) {
                        Ok(project) => {
                            self.cloud_status = CloudStatus::Idle;
                            crate::app::open_project_window(project, None, cx);
                        }
                        Err(e) => {
                            self.cloud_status = CloudStatus::Failed(format!(
                                "Downloaded to {}, but it would not open: {e:#}",
                                dir.display()
                            ))
                        }
                    }
                }
                Err(e) if cloud::is_unauthorized(&e) => self.cloud_revoked(),
                Err(e) => self.cloud_status = CloudStatus::Failed(format!("Could not get the copy: {e:#}")),
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
            CloudMsg::Room(ev) => self.handle_room_event(ev, cx),
        }
        cx.notify();
    }
}

/// Cloud sync of this project: the room thread holds the socket; here we
/// decide what to push (after each save), when to apply what came down
/// (after a pause in typing) and when to fold the log into a snapshot.
impl SyncManager {
    /// Cloud sync can be turned on: the project has a folder and an account.
    pub fn cloud_syncable(&self) -> bool {
        self.config.cloud.is_some() && self.project.dir().is_some() && !self.project.project.id().is_empty()
    }

    pub fn cloud_enabled(&self) -> bool {
        self.cloud_enabled
    }

    pub fn cloud_sync_status(&self) -> CloudSyncStatus {
        match &self.cloud {
            Some(c) => c.status(),
            None => CloudSyncStatus::Off,
        }
    }

    /// A few words for the status bar; nothing when cloud sync is off.
    pub fn cloud_sync_short(&self) -> Option<&'static str> {
        Some(match self.cloud_sync_status() {
            CloudSyncStatus::Off => return None,
            CloudSyncStatus::Connecting => "cloud connecting",
            CloudSyncStatus::Live { pending: true, .. } => "cloud syncing",
            CloudSyncStatus::Live { read_only: true, .. } => "cloud read-only",
            CloudSyncStatus::Live { .. } => "cloud live",
            CloudSyncStatus::Offline(_) => "cloud offline",
            CloudSyncStatus::Failed(_) => "cloud stopped",
        })
    }

    /// Turn cloud sync on or off for this project (remembered in `cloud.json`).
    pub fn set_cloud_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        let Some(dir) = self.project.dir().cloned() else { return };
        let mut state = match &self.cloud {
            Some(c) => c.state.clone(),
            None => CloudState::load(&dir),
        };
        state.enabled = on;
        if let Err(e) = state.save(&dir) {
            tracing::error!("save cloud state: {e:#}");
        }
        self.cloud_enabled = on;
        if on {
            self.start_room(cx);
        } else {
            self.cloud = None;
        }
        cx.notify();
    }

    fn start_room(&mut self, cx: &mut Context<Self>) {
        if self.cloud.is_some() || !self.cloud_enabled || !self.cloud_syncable() {
            return;
        }
        let Some(acct) = self.config.cloud.clone() else { return };
        let Some(dir) = self.project.dir().cloned() else { return };
        let state = CloudState::load(&dir);
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<CloudMsg>();
        let room = RoomHandle::start(
            RoomOptions {
                server: self.config.cloud_server(),
                token: acct.token,
                project_id: self.project.project.id(),
                project_name: self.project.project.name(),
                assets_dir: dir.join("assets"),
                since: state.last_seq,
                dictionary: self.project.load_dictionary(),
            },
            move |ev| {
                let _ = tx.unbounded_send(CloudMsg::Room(ev));
            },
        );
        self._tasks.push(cx.spawn(async move |this, cx| {
            while let Some(msg) = rx.next().await {
                if this.update(cx, |m, cx| m.handle_cloud_msg(msg, cx)).is_err() {
                    break;
                }
            }
        }));
        self.cloud = Some(CloudSync {
            room,
            delivered: state.last_seq,
            state,
            dir,
            in_flight: VecDeque::new(),
            pending: Vec::new(),
            connected: false,
            synced: false,
            role: String::new(),
            log_count: 0,
            log_bytes: 0,
            base_bytes: 0,
            presence: Vec::new(),
            error: None,
            stopped: false,
        });
    }

    fn handle_room_event(&mut self, ev: RoomEvent, cx: &mut Context<Self>) {
        let Some(c) = self.cloud.as_mut() else { return };
        match ev {
            RoomEvent::Connected { role } => {
                c.connected = true;
                c.role = role;
                c.error = None;
                c.in_flight.clear();
            }
            RoomEvent::Updates(batch) => {
                if let Some((seq, _)) = batch.last() {
                    c.delivered = c.delivered.max(*seq);
                }
                c.pending.extend(batch);
                self.schedule_apply(cx);
            }
            RoomEvent::Synced {
                head,
                log_count,
                log_bytes,
                reset,
            } => {
                c.synced = true;
                c.log_count = log_count;
                c.log_bytes = log_bytes;
                if reset {
                    // The room was recreated: everything goes up again.
                    tracing::warn!("cloud room was reset; sending a snapshot");
                    c.state.set_vv(&VersionVector::new());
                    c.delivered = head;
                } else {
                    c.delivered = c.delivered.max(head);
                }
                c.save_state();
                self.push_cloud();
                self.maybe_compact();
            }
            RoomEvent::Pushed(seq) => {
                c.delivered = c.delivered.max(seq);
                if let Some((vv, len, snapshot)) = c.in_flight.pop_front() {
                    c.state.set_vv(&vv);
                    if snapshot {
                        c.log_count = 1;
                        c.log_bytes = len as u64;
                        c.base_bytes = len as u64;
                    } else {
                        c.log_count += 1;
                        c.log_bytes += len as u64;
                    }
                }
                c.save_state();
                self.maybe_compact();
            }
            RoomEvent::Dictionary(words) => {
                let have = self.project.load_dictionary();
                let mut added = false;
                for w in words {
                    if !have.contains(&w) {
                        self.project.append_dictionary_word(&w);
                        added = true;
                    }
                }
                if added {
                    cx.emit(SyncEvent::CloudWords);
                }
            }
            RoomEvent::Assets { downloaded, uploaded } => {
                if !downloaded.is_empty() || !uploaded.is_empty() {
                    tracing::info!("cloud assets: {} down, {} up", downloaded.len(), uploaded.len());
                }
                if !downloaded.is_empty() {
                    cx.emit(SyncEvent::CloudAssets);
                }
            }
            RoomEvent::Presence(names) => c.presence = names,
            RoomEvent::Role(role) => {
                c.role = role;
                self.push_cloud();
            }
            RoomEvent::Warning(w) => tracing::warn!("cloud: {w}"),
            RoomEvent::Disconnected {
                reason,
                fatal,
                unauthorized,
                ..
            } => {
                c.connected = false;
                c.synced = false;
                c.in_flight.clear();
                c.presence.clear();
                c.error = Some(reason);
                c.stopped = fatal;
                if unauthorized {
                    self.cloud_revoked();
                }
            }
        }
        cx.notify();
    }

    /// The workspace reports typing so remote edits wait for a pause.
    pub fn note_edit(&mut self) {
        self.last_edit = Some(Instant::now());
    }

    /// How long until pending remote edits may land; None when there are none.
    fn idle_wait(&self) -> Option<Duration> {
        let c = self.cloud.as_ref()?;
        if c.pending.is_empty() {
            return None;
        }
        Some(
            self.last_edit
                .map(|t| REMOTE_IDLE.saturating_sub(t.elapsed()))
                .unwrap_or(Duration::ZERO),
        )
    }

    fn schedule_apply(&mut self, cx: &mut Context<Self>) {
        if self.apply_scheduled {
            return;
        }
        self.apply_scheduled = true;
        self.apply_task = Some(cx.spawn(async move |this, cx| loop {
            let wait = this.update(cx, |m, _| m.idle_wait()).unwrap_or(None);
            match wait {
                Some(d) if !d.is_zero() => cx.background_executor().timer(d).await,
                Some(_) => {
                    this.update(cx, |m, cx| {
                        m.apply_scheduled = false;
                        cx.emit(SyncEvent::RemoteReady);
                    })
                    .ok();
                    break;
                }
                None => {
                    this.update(cx, |m, _| m.apply_scheduled = false).ok();
                    break;
                }
            }
        }));
    }

    /// Import what the room delivered, save, and move the pushed version
    /// past the remote ops so they are not sent back. The workspace calls
    /// this from `RemoteReady` after noting where its carets are. Returns
    /// how many updates landed.
    pub fn apply_cloud(&mut self) -> Option<usize> {
        let c = self.cloud.as_mut()?;
        if c.pending.is_empty() {
            return None;
        }
        let batch: Vec<Vec<u8>> = std::mem::take(&mut c.pending).into_iter().map(|(_, b)| b).collect();
        let p = &self.project.project;
        let before = p.doc.oplog_vv();
        if let Err(e) = p.import_many(&batch) {
            tracing::error!("apply cloud updates: {e:#}");
            c.error = Some(format!("Could not apply changes from the cloud: {e:#}"));
            return None;
        }
        let after = p.doc.oplog_vv();
        let mut pushed = c.state.vv();
        absorb_remote(&mut pushed, &before, &after);
        c.state.set_vv(&pushed);
        for (vv, _, _) in c.in_flight.iter_mut() {
            absorb_remote(vv, &before, &after);
        }
        if let Err(e) = p.save() {
            tracing::error!("save after cloud sync: {e:#}");
        }
        c.save_state();
        let count = batch.len();
        tracing::info!("applied {count} cloud update(s)");
        self.push_cloud();
        self.maybe_compact();
        Some(count)
    }

    /// Send what the room does not have yet. Called after every save; a
    /// no-op while the replay is still coming in or remote edits wait.
    pub fn push_cloud(&mut self) {
        let Some(c) = self.cloud.as_mut() else { return };
        if !c.synced || !c.pending.is_empty() || !c.writer() {
            return;
        }
        let doc = &self.project.project.doc;
        let now = doc.oplog_vv();
        let from = c
            .in_flight
            .back()
            .map(|(vv, _, _)| vv.clone())
            .unwrap_or_else(|| c.state.vv());
        if from == now {
            return;
        }
        let bytes = if from.is_empty() {
            doc.export(ExportMode::Snapshot)
        } else {
            doc.export(ExportMode::updates(&from))
        };
        match bytes {
            Ok(b) => {
                let len = b.len();
                c.room.push(b);
                c.in_flight.push_back((now, len, false));
            }
            Err(e) => tracing::error!("export for the cloud: {e}"),
        }
    }

    /// Upload a snapshot as the room's new base once the log has grown
    /// past the thresholds and everything is settled on both sides.
    fn maybe_compact(&mut self) {
        let Some(c) = self.cloud.as_mut() else { return };
        if !c.synced || !c.in_flight.is_empty() || !c.pending.is_empty() || c.log_count < 2 || !c.writer() {
            return;
        }
        let bytes_limit = COMPACT_BYTES.max(2 * c.base_bytes);
        if c.log_count <= COMPACT_COUNT && c.log_bytes <= bytes_limit {
            return;
        }
        let p = &self.project.project;
        if p.doc.oplog_vv() != c.state.vv() {
            return;
        }
        match p.export_snapshot() {
            Ok(b) => {
                let len = b.len();
                tracing::info!("compacting the cloud log at {} ({len} bytes)", c.delivered);
                c.room.upload_snapshot(b, c.delivered);
                c.in_flight.push_back((p.doc.oplog_vv(), len, true));
            }
            Err(e) => tracing::error!("snapshot for the cloud: {e:#}"),
        }
    }

    pub fn cloud_add_words(&self, words: Vec<String>) {
        if let Some(c) = &self.cloud {
            c.room.add_words(words);
        }
    }

    /// Something under `assets/` may have changed: let the room look.
    pub fn cloud_rescan_assets(&self) {
        if let Some(c) = &self.cloud {
            c.room.rescan_assets();
        }
    }
}

impl EventEmitter<SyncEvent> for SyncManager {}
