//! A project's cloud room from the app's side: one thread per open project
//! keeps a websocket to `/parties/project-room/<id>`, replays what the
//! server has since our last sequence number, pushes the blobs the app
//! hands it, fetches big blobs over HTTP, keeps `assets/` in step with R2
//! and reconnects with backoff. Everything it learns comes back as
//! [`RoomEvent`]s on the callback; the app applies updates on its own
//! schedule.

use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{anyhow, bail, Context, Result};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use super::protocol::{split_frame, AssetEntry, ClientMsg, ServerMsg, MAX_INLINE, PROTOCOL_VERSION};
use super::{is_unauthorized, Client};
use crate::util::{safe_relative, sha256_hex};

/// How long a read waits before the thread checks for commands.
const READ_TICK: Duration = Duration::from_millis(250);
/// Idle time before we ping the server, and how long we wait for the pong.
const PING_AFTER: Duration = Duration::from_secs(30);
const PONG_WITHIN: Duration = Duration::from_secs(20);
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// A connection that lasted this long resets the backoff.
const STABLE_AFTER: Duration = Duration::from_secs(30);
/// HTTP timeout for blob and asset transfers (whole novels, images).
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(300);

pub struct RoomOptions {
    pub server: String,
    pub token: String,
    pub project_id: String,
    /// Shown on the account page; sent with every hello.
    pub project_name: String,
    /// The project's `assets/` folder, mirrored to R2.
    pub assets_dir: PathBuf,
    /// Last sequence number already applied locally.
    pub since: u64,
    /// This machine's custom dictionary, merged into the room's on connect.
    pub dictionary: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoomEvent {
    /// The socket is up and the server accepted us.
    Connected { role: String },
    /// Updates from other devices, in sequence order: one batch for the
    /// replay after connecting, then one event per live update.
    Updates(Vec<(u64, Vec<u8>)>),
    /// Replay finished. `reset` means the room had less than we claimed
    /// (it was recreated): forget what was pushed and send a snapshot.
    Synced {
        head: u64,
        log_count: u64,
        log_bytes: u64,
        reset: bool,
    },
    /// Our push was stored under this sequence number.
    Pushed(u64),
    /// The owner changed our role while we were connected.
    Role(String),
    /// Words other devices added (or the room's whole list on connect).
    Dictionary(Vec<String>),
    /// Files that changed under `assets/` because of the room.
    Assets {
        downloaded: Vec<String>,
        uploaded: Vec<String>,
    },
    /// Names of the devices currently in the room (ourselves included).
    Presence(Vec<String>),
    /// Something non-fatal went wrong (an asset transfer, say).
    Warning(String),
    /// The socket dropped. `fatal` means we stop trying: the token was
    /// revoked or the project is not ours.
    Disconnected {
        reason: String,
        fatal: bool,
        /// The server no longer accepts our token.
        unauthorized: bool,
        retry_in: Option<Duration>,
    },
}

enum Cmd {
    Push(Vec<u8>),
    Snapshot { bytes: Vec<u8>, at: u64 },
    Words(Vec<String>),
    RescanAssets,
    Stop,
}

/// The app's handle on the room thread. Dropping it stops the thread.
pub struct RoomHandle {
    tx: Sender<Cmd>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl RoomHandle {
    pub fn start(opts: RoomOptions, on_event: impl Fn(RoomEvent) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let name = format!(
            "wordy-room-{}",
            &opts.project_id[opts.project_id.len().saturating_sub(6)..]
        );
        let thread = std::thread::Builder::new()
            .name(name)
            .spawn(move || run(opts, rx, flag, Box::new(on_event)))
            .expect("spawn room thread");
        Self {
            tx,
            stop,
            thread: Some(thread),
        }
    }

    /// Send one Loro blob (updates since the last push, or a snapshot).
    pub fn push(&self, bytes: Vec<u8>) {
        let _ = self.tx.send(Cmd::Push(bytes));
    }

    /// Upload a full snapshot taken after applying sequence `at`; it
    /// becomes the room's base when nothing else landed meanwhile.
    pub fn upload_snapshot(&self, bytes: Vec<u8>, at: u64) {
        let _ = self.tx.send(Cmd::Snapshot { bytes, at });
    }

    pub fn add_words(&self, words: Vec<String>) {
        if !words.is_empty() {
            let _ = self.tx.send(Cmd::Words(words));
        }
    }

    /// Look for new or changed files under `assets/` and upload them.
    pub fn rescan_assets(&self) {
        let _ = self.tx.send(Cmd::RescanAssets);
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.tx.send(Cmd::Stop);
    }

    /// Stop and wait for the thread to wind up, so whatever it was in the
    /// middle of (an asset download, say) is on disk when this returns.
    pub fn finish(mut self) {
        self.stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for RoomHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

type Emit = Box<dyn Fn(RoomEvent) + Send>;
type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

fn run(opts: RoomOptions, rx: Receiver<Cmd>, stop: Arc<AtomicBool>, emit: Emit) {
    let client = Client::with_timeout(&opts.server, TRANSFER_TIMEOUT);
    let mut since = opts.since;
    let mut backoff = BACKOFF_MIN;
    let mut assets = AssetCache::new(opts.assets_dir.clone());
    while !stop.load(Ordering::Relaxed) {
        let started = Instant::now();
        let (reason, fatal, unauthorized) = match connect(&opts) {
            Ok(ws) => {
                let mut conn = Conn {
                    ws,
                    client: &client,
                    opts: &opts,
                    emit: &emit,
                    assets: &mut assets,
                    since,
                    own: HashSet::new(),
                    synced: false,
                    role: String::new(),
                    server_assets: HashMap::new(),
                    batch: Vec::new(),
                    welcome: None,
                    last_rx: Instant::now(),
                    ping_sent: None,
                };
                let result = conn.run(&rx, &stop);
                since = conn.since;
                match result {
                    Ok(()) => break,
                    Err(e) => (format!("{e:#}"), is_unauthorized(&e), is_unauthorized(&e)),
                }
            }
            Err(e) => (format!("{e:#}"), is_fatal(&e), is_unauthorized(&e)),
        };
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if started.elapsed() >= STABLE_AFTER {
            backoff = BACKOFF_MIN;
        }
        tracing::warn!("room {}: {reason}", opts.project_id);
        emit(RoomEvent::Disconnected {
            reason,
            fatal,
            unauthorized,
            retry_in: (!fatal).then_some(backoff),
        });
        if fatal {
            break;
        }
        // Wait out the backoff, still answering Stop promptly. Pushes that
        // arrive meanwhile are dropped: the app re-sends from its version
        // vector once we are back.
        let until = Instant::now() + backoff;
        while Instant::now() < until && !stop.load(Ordering::Relaxed) {
            match rx.recv_timeout(READ_TICK) {
                Ok(Cmd::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                _ => {}
            }
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// A refusal at the handshake that will not change by retrying.
fn is_fatal(e: &anyhow::Error) -> bool {
    is_unauthorized(e) || e.downcast_ref::<Refused>().is_some()
}

/// The server answered the upgrade with an error status.
#[derive(Debug)]
struct Refused(u16, String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (HTTP {})", self.1, self.0)
    }
}

impl std::error::Error for Refused {}

fn connect(opts: &RoomOptions) -> Result<Socket> {
    let base = opts.server.trim_end_matches('/');
    let ws_base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        bail!("server address must start with http:// or https://");
    };
    let url = format!("{ws_base}/parties/project-room/{}", opts.project_id);
    let mut req = url.into_client_request().context("room url")?;
    req.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {}", opts.token)).context("token header")?,
    );
    let (ws, _) = match tungstenite::connect(req) {
        Ok(ok) => ok,
        Err(tungstenite::Error::Http(resp)) => {
            let status = resp.status().as_u16();
            let text = resp
                .body()
                .as_ref()
                .map(|b| String::from_utf8_lossy(b).trim().to_string())
                .unwrap_or_default();
            if status == 401 {
                return Err(super::Unauthorized.into());
            }
            if status == 403 || status == 404 {
                return Err(Refused(status, text).into());
            }
            bail!("server refused the connection: {text} (HTTP {status})");
        }
        Err(e) => return Err(anyhow!("connecting: {e}")),
    };
    let tcp = match ws.get_ref() {
        MaybeTlsStream::Plain(s) => Some(s),
        MaybeTlsStream::Rustls(s) => Some(s.get_ref()),
        _ => None,
    };
    if let Some(tcp) = tcp {
        tcp.set_read_timeout(Some(READ_TICK)).context("read timeout")?;
    }
    Ok(ws)
}

struct Conn<'a> {
    ws: Socket,
    client: &'a Client,
    opts: &'a RoomOptions,
    emit: &'a Emit,
    assets: &'a mut AssetCache,
    /// Last sequence number delivered to the app or skipped as our own.
    since: u64,
    /// Sequence numbers the server acked for our pushes; their echoes are skipped.
    own: HashSet<u64>,
    synced: bool,
    role: String,
    server_assets: HashMap<String, AssetEntry>,
    /// Replayed updates, delivered together at `synced`.
    batch: Vec<(u64, Vec<u8>)>,
    /// (log_count, log_bytes, reset) from the welcome, for the Synced event.
    welcome: Option<(u64, u64, bool)>,
    last_rx: Instant,
    ping_sent: Option<Instant>,
}

impl Conn<'_> {
    fn emit(&self, ev: RoomEvent) {
        (self.emit)(ev);
    }

    fn send(&mut self, msg: &ClientMsg) -> Result<()> {
        let text = serde_json::to_string(msg)?;
        self.ws.send(Message::Text(text.into())).context("sending")?;
        Ok(())
    }

    fn writer(&self) -> bool {
        self.role != "reader"
    }

    /// Returns `Ok(())` only when asked to stop; otherwise the reason the
    /// connection ended.
    fn run(&mut self, rx: &Receiver<Cmd>, stop: &AtomicBool) -> Result<()> {
        self.send(&ClientMsg::Hello {
            version: PROTOCOL_VERSION,
            since: self.since,
            name: Some(self.opts.project_name.clone()),
            dictionary: self.opts.dictionary.clone(),
        })?;
        loop {
            loop {
                match rx.try_recv() {
                    Ok(Cmd::Stop) | Err(TryRecvError::Disconnected) => return self.bye(),
                    Ok(cmd) => self.handle_cmd(cmd)?,
                    Err(TryRecvError::Empty) => break,
                }
            }
            if stop.load(Ordering::Relaxed) {
                return self.bye();
            }
            match self.ws.read() {
                Ok(Message::Text(t)) => {
                    self.last_rx = Instant::now();
                    let msg: ServerMsg = serde_json::from_str(&t).with_context(|| format!("bad message: {t}"))?;
                    self.handle_msg(msg)?;
                }
                Ok(Message::Binary(b)) => {
                    self.last_rx = Instant::now();
                    let (seq, bytes) = split_frame(&b)?;
                    self.deliver(seq, bytes.to_vec());
                }
                Ok(Message::Close(frame)) => {
                    let reason = frame
                        .map(|f| format!("{} ({})", f.reason, u16::from(f.code)))
                        .unwrap_or_else(|| "closed".into());
                    let code = reason
                        .rsplit('(')
                        .next()
                        .and_then(|c| c.trim_end_matches(')').parse::<u16>().ok());
                    if code == Some(4401) {
                        return Err(super::Unauthorized.into());
                    }
                    bail!("server closed the connection: {reason}");
                }
                Ok(_) => {
                    // Ping/pong frames: tungstenite queues the reply; make sure it goes out.
                    self.last_rx = Instant::now();
                    self.flush_quietly();
                }
                Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    self.flush_quietly();
                    self.keepalive()?;
                }
                Err(e) => bail!("connection lost: {e}"),
            }
        }
    }

    fn bye(&mut self) -> Result<()> {
        let _ = self.ws.close(None);
        let _ = self.ws.flush();
        Ok(())
    }

    fn flush_quietly(&mut self) {
        match self.ws.flush() {
            Ok(()) => {}
            Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {}
            Err(e) => tracing::debug!("flush: {e}"),
        }
    }

    fn keepalive(&mut self) -> Result<()> {
        if let Some(sent) = self.ping_sent {
            if sent.elapsed() > PONG_WITHIN {
                bail!(
                    "no answer from the server for {}s",
                    (PING_AFTER + PONG_WITHIN).as_secs()
                );
            }
        } else if self.last_rx.elapsed() > PING_AFTER {
            self.send(&ClientMsg::Ping)?;
            self.ping_sent = Some(Instant::now());
        }
        Ok(())
    }

    fn handle_cmd(&mut self, cmd: Cmd) -> Result<()> {
        match cmd {
            Cmd::Push(bytes) => {
                if bytes.len() <= MAX_INLINE {
                    self.ws.send(Message::Binary(bytes.into())).context("pushing")?;
                } else {
                    // The ack still arrives over the socket.
                    self.client
                        .put_update(&self.opts.token, &self.opts.project_id, &bytes, None)
                        .context("uploading a large update")?;
                }
            }
            Cmd::Snapshot { bytes, at } => {
                self.client
                    .put_update(&self.opts.token, &self.opts.project_id, &bytes, Some(at))
                    .context("uploading a snapshot")?;
            }
            Cmd::Words(words) => {
                if self.writer() {
                    self.send(&ClientMsg::Dictionary { words })?;
                }
            }
            Cmd::RescanAssets => {
                if self.synced {
                    self.reconcile_assets();
                }
            }
            Cmd::Stop => unreachable!("handled by the caller"),
        }
        Ok(())
    }

    fn handle_msg(&mut self, msg: ServerMsg) -> Result<()> {
        match msg {
            ServerMsg::Welcome {
                version,
                head,
                base,
                from,
                reset,
                role,
                dictionary,
                assets,
                log_bytes,
                log_count,
            } => {
                if version != PROTOCOL_VERSION {
                    bail!("the server speaks sync protocol v{version}; this app speaks v{PROTOCOL_VERSION}");
                }
                tracing::info!(
                    "room {}: welcome as {role}, head {head}, base {base}, replay from {from}, reset {reset}",
                    self.opts.project_id
                );
                self.role = role.clone();
                self.server_assets = assets.into_iter().map(|a| (a.path.clone(), a)).collect();
                self.welcome = Some((log_count, log_bytes, reset));
                self.emit(RoomEvent::Connected { role });
                if !dictionary.is_empty() {
                    self.emit(RoomEvent::Dictionary(dictionary));
                }
            }
            ServerMsg::Blob { seq, size } => {
                if self.own.remove(&seq) {
                    self.since = self.since.max(seq);
                    return Ok(());
                }
                tracing::info!("room {}: fetching update {seq} ({size} bytes)", self.opts.project_id);
                let mut last_err = None;
                for _ in 0..3 {
                    match self.client.fetch_update(&self.opts.token, &self.opts.project_id, seq) {
                        Ok(bytes) => {
                            self.deliver(seq, bytes);
                            return Ok(());
                        }
                        Err(e) if is_unauthorized(&e) => return Err(e),
                        Err(e) => last_err = Some(e),
                    }
                }
                return Err(last_err.unwrap().context(format!("fetching update {seq}")));
            }
            ServerMsg::Synced { head } => {
                self.synced = true;
                let batch = std::mem::take(&mut self.batch);
                if !batch.is_empty() {
                    self.emit(RoomEvent::Updates(batch));
                }
                self.since = head;
                let (log_count, log_bytes, reset) = self.welcome.take().unwrap_or_default();
                self.emit(RoomEvent::Synced {
                    head,
                    log_count,
                    log_bytes,
                    reset,
                });
                self.reconcile_assets();
            }
            ServerMsg::Ack { seq } => {
                self.own.insert(seq);
                self.emit(RoomEvent::Pushed(seq));
            }
            ServerMsg::Dictionary { words } => self.emit(RoomEvent::Dictionary(words)),
            ServerMsg::Asset { path, sha256, size } => {
                let entry = AssetEntry { path, sha256, size };
                self.server_assets.insert(entry.path.clone(), entry.clone());
                if self.synced {
                    match self.assets.download_if_needed(self.client, self.opts, &entry) {
                        Ok(true) => self.emit(RoomEvent::Assets {
                            downloaded: vec![entry.path],
                            uploaded: Vec::new(),
                        }),
                        Ok(false) => {}
                        Err(e) => self.emit(RoomEvent::Warning(format!("asset {}: {e:#}", entry.path))),
                    }
                }
            }
            ServerMsg::BaseMoved { base } => {
                tracing::info!("room {}: base moved to {base}", self.opts.project_id);
            }
            ServerMsg::Presence { devices } => {
                self.emit(RoomEvent::Presence(devices.into_iter().map(|d| d.name).collect()));
            }
            ServerMsg::Role { role } => {
                tracing::info!("room {}: now {role}", self.opts.project_id);
                self.role = role.clone();
                self.emit(RoomEvent::Role(role));
            }
            ServerMsg::Pong => self.ping_sent = None,
            ServerMsg::Error { message } => bail!("server: {message}"),
        }
        Ok(())
    }

    fn deliver(&mut self, seq: u64, bytes: Vec<u8>) {
        if self.own.remove(&seq) {
            self.since = self.since.max(seq);
            return;
        }
        self.since = self.since.max(seq);
        if self.synced {
            self.emit(RoomEvent::Updates(vec![(seq, bytes)]));
        } else {
            self.batch.push((seq, bytes));
        }
    }

    /// Bring `assets/` and the room's manifest into agreement: download
    /// what the server has and we lack (the server wins on a clash),
    /// upload what we have and it lacks.
    fn reconcile_assets(&mut self) {
        let local = match self.assets.scan() {
            Ok(l) => l,
            Err(e) => {
                self.emit(RoomEvent::Warning(format!("scanning assets: {e:#}")));
                return;
            }
        };
        let mut downloaded = Vec::new();
        let mut uploaded = Vec::new();
        let mut warnings = Vec::new();
        for entry in self.server_assets.clone().values() {
            match self.assets.download_if_needed(self.client, self.opts, entry) {
                Ok(true) => downloaded.push(entry.path.clone()),
                Ok(false) => {}
                Err(e) => warnings.push(format!("asset {}: {e:#}", entry.path)),
            }
        }
        if self.writer() {
            for entry in local {
                if self.server_assets.contains_key(&entry.path) {
                    continue;
                }
                let full = self.assets.dir.join(&entry.path);
                let result = std::fs::read(&full)
                    .with_context(|| format!("reading {}", full.display()))
                    .and_then(|bytes| {
                        self.client
                            .put_asset(&self.opts.token, &self.opts.project_id, &entry.sha256, &bytes)
                    })
                    .and_then(|()| {
                        self.send(&ClientMsg::Asset {
                            path: entry.path.clone(),
                            sha256: entry.sha256.clone(),
                            size: entry.size,
                        })
                    });
                match result {
                    Ok(()) => {
                        self.server_assets.insert(entry.path.clone(), entry.clone());
                        uploaded.push(entry.path);
                    }
                    Err(e) => warnings.push(format!("uploading {}: {e:#}", entry.path)),
                }
            }
        }
        for w in warnings {
            self.emit(RoomEvent::Warning(w));
        }
        if !downloaded.is_empty() || !uploaded.is_empty() {
            downloaded.sort();
            uploaded.sort();
            self.emit(RoomEvent::Assets { downloaded, uploaded });
        }
    }
}

/// The local `assets/` folder with hashes cached by size and mtime, so a
/// rescan after every save stays cheap.
struct AssetCache {
    dir: PathBuf,
    known: HashMap<String, (u64, SystemTime, String)>,
}

impl AssetCache {
    fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            known: HashMap::new(),
        }
    }

    fn scan(&mut self) -> Result<Vec<AssetEntry>> {
        let mut out = Vec::new();
        if !self.dir.is_dir() {
            return Ok(out);
        }
        let mut seen = HashSet::new();
        let mut stack = vec![PathBuf::new()];
        while let Some(rel) = stack.pop() {
            for entry in std::fs::read_dir(self.dir.join(&rel))? {
                let entry = entry?;
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                if name.ends_with(".tmp") || name.starts_with('.') {
                    continue;
                }
                let rel_path = if rel.as_os_str().is_empty() {
                    PathBuf::from(name)
                } else {
                    rel.join(name)
                };
                let ty = entry.file_type()?;
                if ty.is_dir() {
                    stack.push(rel_path);
                    continue;
                }
                if !ty.is_file() {
                    continue;
                }
                let path = rel_path
                    .components()
                    .filter_map(|c| c.as_os_str().to_str())
                    .collect::<Vec<_>>()
                    .join("/");
                let meta = entry.metadata()?;
                let size = meta.len();
                let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                let sha256 = match self.known.get(&path) {
                    Some((s, m, sha)) if *s == size && *m == mtime => sha.clone(),
                    _ => {
                        let bytes = std::fs::read(entry.path())?;
                        let sha = sha256_hex(&bytes);
                        self.known.insert(path.clone(), (size, mtime, sha.clone()));
                        sha
                    }
                };
                seen.insert(path.clone());
                out.push(AssetEntry { path, sha256, size });
            }
        }
        self.known.retain(|p, _| seen.contains(p));
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    /// Local sha of `path`, hashing if the cache is stale.
    fn local_sha(&mut self, path: &str) -> Result<Option<String>> {
        let full = self.dir.join(path);
        let Ok(meta) = std::fs::metadata(&full) else {
            return Ok(None);
        };
        if !meta.is_file() {
            return Ok(None);
        }
        let size = meta.len();
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if let Some((s, m, sha)) = self.known.get(path) {
            if *s == size && *m == mtime {
                return Ok(Some(sha.clone()));
            }
        }
        let bytes = std::fs::read(&full)?;
        let sha = sha256_hex(&bytes);
        self.known.insert(path.to_string(), (size, mtime, sha.clone()));
        Ok(Some(sha))
    }

    /// Fetch `entry` unless the local file already matches. True when a
    /// file was written.
    fn download_if_needed(&mut self, client: &Client, opts: &RoomOptions, entry: &AssetEntry) -> Result<bool> {
        if !safe_relative(&entry.path) {
            bail!("unsafe path from the server");
        }
        if self.local_sha(&entry.path)?.as_deref() == Some(entry.sha256.as_str()) {
            return Ok(false);
        }
        let bytes = client.fetch_asset(&opts.token, &opts.project_id, &entry.sha256)?;
        if sha256_hex(&bytes) != entry.sha256 {
            bail!("downloaded file does not match its hash");
        }
        write_file(&self.dir.join(&entry.path), &bytes)?;
        self.known.remove(&entry.path);
        Ok(true)
    }
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("bin")
    ));
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("renaming to {}", path.display()))?;
    Ok(())
}
