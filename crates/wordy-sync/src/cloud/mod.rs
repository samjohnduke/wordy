//! The Wordy cloud account: linking this machine to an account on the
//! website with the OAuth device-code flow, and the few account calls the
//! app makes with the resulting bearer token.
//!
//! Passkeys cannot be used from a native app, so the app never signs in
//! itself. It asks the server for a short code, opens the browser at
//! `/device?user_code=…` where the user approves it with their passkey,
//! and polls until the server hands back a session token. Everything here
//! blocks (plain `ureq`), so call it from a worker thread.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub mod protocol;
pub mod room;
pub mod state;

pub use room::{RoomEvent, RoomHandle, RoomOptions};
pub use state::CloudState;

/// Where accounts live unless `sync.json` says otherwise.
pub const DEFAULT_SERVER: &str = "https://wordy.samduke.dev";
/// The client id the server accepts for the device flow.
pub const CLIENT_ID: &str = "wordy-app";
const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// A linked account as stored in `sync.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloudAccount {
    /// Session token sent as `Authorization: Bearer …`.
    pub token: String,
    pub user_id: String,
    pub email: String,
    /// The `device` row the server keeps for this machine.
    pub device_id: String,
    pub device_name: String,
}

/// What the server knows about a user.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct User {
    pub id: String,
    pub email: String,
    #[serde(default)]
    pub name: String,
}

/// One linked machine, as listed by the server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub platform: String,
    #[serde(default, rename = "appVersion")]
    pub app_version: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "lastSeenAt")]
    pub last_seen_at: String,
    /// True for the device whose token made the request.
    #[serde(default)]
    pub current: bool,
}

/// The account page's data: who we are and every linked machine.
#[derive(Debug, Clone, Deserialize)]
pub struct Account {
    pub user: User,
    pub devices: Vec<Device>,
}

/// A device-code flow in progress.
#[derive(Debug, Clone)]
pub struct DeviceLink {
    /// The code the user sees and types (or finds prefilled).
    pub user_code: String,
    /// The secret the app polls with.
    pub device_code: String,
    /// Open this in the browser; it has the user code in the query.
    pub verify_url: String,
    /// Minimum gap between polls.
    pub interval: Duration,
    /// When the code stops working.
    pub expires_at: Instant,
}

/// One poll of the token endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll {
    /// Not approved yet; poll again after the interval.
    Pending,
    /// The server asked us to back off; the interval was bumped.
    SlowDown,
    /// Approved: a bearer token.
    Token(String),
}

/// An HTTP client for one server.
#[derive(Clone)]
pub struct Client {
    server: String,
    agent: ureq::Agent,
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: String,
    #[serde(default)]
    message: String,
}

impl Client {
    /// `server` is an origin like `https://wordy.example`; a trailing slash
    /// is fine.
    pub fn new(server: &str) -> Self {
        Self::with_timeout(server, HTTP_TIMEOUT)
    }

    /// A client whose requests may take up to `timeout` (blob transfers).
    pub fn with_timeout(server: &str, timeout: Duration) -> Self {
        let server = server.trim().trim_end_matches('/').to_string();
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(timeout))
            .user_agent(format!("wordy/{}", env!("CARGO_PKG_VERSION")))
            .build();
        Self {
            server,
            agent: ureq::Agent::new_with_config(config),
        }
    }

    pub fn server(&self) -> &str {
        &self.server
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.server)
    }

    /// POST JSON, returning the status and body text.
    fn post_json(&self, path: &str, token: Option<&str>, body: &impl Serialize) -> Result<(u16, String)> {
        let mut req = self.agent.post(self.url(path));
        if let Some(t) = token {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        let mut resp = req.send_json(body).with_context(|| format!("POST {path}"))?;
        let status = resp.status().as_u16();
        let text = resp
            .body_mut()
            .read_to_string()
            .with_context(|| format!("reading POST {path}"))?;
        Ok((status, text))
    }

    fn get(&self, path: &str, token: &str) -> Result<(u16, String)> {
        let mut resp = self
            .agent
            .get(self.url(path))
            .header("Authorization", format!("Bearer {token}"))
            .call()
            .with_context(|| format!("GET {path}"))?;
        let status = resp.status().as_u16();
        let text = resp
            .body_mut()
            .read_to_string()
            .with_context(|| format!("reading GET {path}"))?;
        Ok((status, text))
    }

    fn delete(&self, path: &str, token: &str) -> Result<u16> {
        // Astro's CSRF check rejects bodyless non-GET requests that carry
        // neither a JSON content type nor a matching Origin.
        let resp = self
            .agent
            .delete(self.url(path))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .call()
            .with_context(|| format!("DELETE {path}"))?;
        Ok(resp.status().as_u16())
    }

    /// Ask for a device code. The user must approve it in the browser at
    /// `verify_url` within `expires_at`.
    pub fn start_link(&self) -> Result<DeviceLink> {
        #[derive(Deserialize)]
        struct Body {
            device_code: String,
            user_code: String,
            #[serde(default)]
            verification_uri: String,
            #[serde(default)]
            verification_uri_complete: String,
            #[serde(default = "five")]
            interval: u64,
            #[serde(default = "fifteen_minutes")]
            expires_in: u64,
        }
        fn five() -> u64 {
            5
        }
        fn fifteen_minutes() -> u64 {
            900
        }
        let (status, text) = self.post_json(
            "/api/auth/device/code",
            None,
            &serde_json::json!({ "client_id": CLIENT_ID }),
        )?;
        if status != 200 {
            bail!("device code request failed: {}", describe(status, &text));
        }
        let b: Body = serde_json::from_str(&text).context("device code response")?;
        let verify_url = if !b.verification_uri_complete.is_empty() {
            b.verification_uri_complete
        } else if !b.verification_uri.is_empty() {
            format!("{}?user_code={}", b.verification_uri, b.user_code)
        } else {
            format!("{}/device?user_code={}", self.server, b.user_code)
        };
        Ok(DeviceLink {
            user_code: b.user_code,
            device_code: b.device_code,
            verify_url,
            interval: Duration::from_secs(b.interval.max(1)),
            expires_at: Instant::now() + Duration::from_secs(b.expires_in),
        })
    }

    /// One poll of the token endpoint. Errors mean the flow is over
    /// (denied, expired, or the server is unreachable).
    pub fn poll_token(&self, link: &DeviceLink) -> Result<Poll> {
        let body = serde_json::json!({
            "grant_type": GRANT_TYPE,
            "device_code": link.device_code,
            "client_id": CLIENT_ID,
        });
        let (status, text) = self.post_json("/api/auth/device/token", None, &body)?;
        if status == 200 {
            #[derive(Deserialize)]
            struct Body {
                access_token: String,
            }
            let b: Body = serde_json::from_str(&text).context("token response")?;
            return Ok(Poll::Token(b.access_token));
        }
        let err: ErrorBody = serde_json::from_str(&text).unwrap_or(ErrorBody {
            error: String::new(),
            error_description: String::new(),
            message: String::new(),
        });
        match err.error.as_str() {
            "authorization_pending" => Ok(Poll::Pending),
            "slow_down" => Ok(Poll::SlowDown),
            "access_denied" => bail!("the request was denied in the browser"),
            "expired_token" => bail!("the code expired before it was approved"),
            _ => bail!("token request failed: {}", describe(status, &text)),
        }
    }

    /// Poll until approved, denied or expired. `keep_going` is checked
    /// before each poll so the UI can cancel.
    pub fn wait_for_token(&self, link: &DeviceLink, keep_going: impl Fn() -> bool) -> Result<String> {
        let mut interval = link.interval;
        loop {
            if !keep_going() {
                bail!("cancelled");
            }
            if Instant::now() >= link.expires_at {
                bail!("the code expired before it was approved");
            }
            match self.poll_token(link)? {
                Poll::Token(t) => return Ok(t),
                Poll::Pending => {}
                Poll::SlowDown => interval += Duration::from_secs(5),
            }
            std::thread::sleep(interval);
        }
    }

    /// Tell the server about this machine. Idempotent per session.
    pub fn register_device(
        &self,
        token: &str,
        name: &str,
        platform: &str,
        app_version: &str,
    ) -> Result<(User, Device)> {
        #[derive(Deserialize)]
        struct Body {
            user: User,
            device: Device,
        }
        let body = serde_json::json!({ "name": name, "platform": platform, "appVersion": app_version });
        let (status, text) = self.post_json("/api/devices", Some(token), &body)?;
        if status != 200 {
            bail!("registering this device failed: {}", describe(status, &text));
        }
        let b: Body = serde_json::from_str(&text).context("device response")?;
        Ok((b.user, b.device))
    }

    /// Who the token belongs to and every linked machine. A 401 means the
    /// token was revoked (the device was removed on the website).
    pub fn account(&self, token: &str) -> Result<Account> {
        let (status, text) = self.get("/api/devices", token)?;
        if status == 401 {
            return Err(Unauthorized.into());
        }
        if status != 200 {
            bail!("account request failed: {}", describe(status, &text));
        }
        serde_json::from_str(&text).context("account response")
    }

    /// Remove another linked machine.
    pub fn revoke_device(&self, token: &str, device_id: &str) -> Result<()> {
        match self.delete(&format!("/api/devices/{device_id}"), token)? {
            204 | 404 => Ok(()),
            401 => Err(Unauthorized.into()),
            s => bail!("removing the device failed (HTTP {s})"),
        }
    }

    /// End this machine's session on the server. A token that is already
    /// invalid counts as success.
    pub fn unlink(&self, token: &str) -> Result<()> {
        let (status, text) = self.post_json("/api/auth/sign-out", Some(token), &serde_json::json!({}))?;
        match status {
            200 | 401 => Ok(()),
            _ => bail!("sign-out failed: {}", describe(status, &text)),
        }
    }

    /// Raw GET: status and body bytes.
    fn get_bytes(&self, path: &str, token: &str) -> Result<(u16, Vec<u8>)> {
        let mut resp = self
            .agent
            .get(self.url(path))
            .header("Authorization", format!("Bearer {token}"))
            .call()
            .with_context(|| format!("GET {path}"))?;
        let status = resp.status().as_u16();
        let bytes = resp
            .body_mut()
            .with_config()
            .limit(protocol::MAX_BLOB)
            .read_to_vec()
            .with_context(|| format!("reading GET {path}"))?;
        Ok((status, bytes))
    }

    /// Raw PUT of a byte body.
    fn put_bytes(&self, path: &str, token: &str, bytes: &[u8], extra: &[(&str, String)]) -> Result<(u16, String)> {
        let mut req = self
            .agent
            .put(self.url(path))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/octet-stream");
        for (k, v) in extra {
            req = req.header(*k, v);
        }
        let mut resp = req.send(bytes).with_context(|| format!("PUT {path}"))?;
        let status = resp.status().as_u16();
        let text = resp
            .body_mut()
            .read_to_string()
            .with_context(|| format!("reading PUT {path}"))?;
        Ok((status, text))
    }

    /// One stored update of a project room by sequence number.
    pub fn fetch_update(&self, token: &str, project_id: &str, seq: u64) -> Result<Vec<u8>> {
        let (status, bytes) = self.get_bytes(&format!("/api/projects/{project_id}/updates/{seq}"), token)?;
        match status {
            200 => Ok(bytes),
            401 => Err(Unauthorized.into()),
            s => bail!(
                "fetching update {seq} failed: {}",
                describe(s, &String::from_utf8_lossy(&bytes))
            ),
        }
    }

    /// Store a blob too big for the socket. With `snapshot_at`, the blob is
    /// a full snapshot taken after applying that sequence number and becomes
    /// the room's base if nothing landed since. Returns the new sequence
    /// number and the room's base.
    pub fn put_update(
        &self,
        token: &str,
        project_id: &str,
        bytes: &[u8],
        snapshot_at: Option<u64>,
    ) -> Result<(u64, u64)> {
        let mut extra = Vec::new();
        if let Some(at) = snapshot_at {
            extra.push(("x-wordy-snapshot-at", at.to_string()));
        }
        let (status, text) = self.put_bytes(&format!("/api/projects/{project_id}/updates"), token, bytes, &extra)?;
        #[derive(Deserialize)]
        struct Put {
            seq: u64,
            base: u64,
        }
        match status {
            200 => {
                let p: Put = serde_json::from_str(&text).context("put update response")?;
                Ok((p.seq, p.base))
            }
            401 => Err(Unauthorized.into()),
            s => bail!("uploading the update failed: {}", describe(s, &text)),
        }
    }

    /// Download a project asset by content hash.
    pub fn fetch_asset(&self, token: &str, project_id: &str, sha256: &str) -> Result<Vec<u8>> {
        let (status, bytes) = self.get_bytes(&format!("/api/projects/{project_id}/assets/{sha256}"), token)?;
        match status {
            200 => Ok(bytes),
            401 => Err(Unauthorized.into()),
            s => bail!(
                "fetching asset {sha256} failed: {}",
                describe(s, &String::from_utf8_lossy(&bytes))
            ),
        }
    }

    /// Upload a project asset (idempotent by content hash).
    pub fn put_asset(&self, token: &str, project_id: &str, sha256: &str, bytes: &[u8]) -> Result<()> {
        let (status, text) = self.put_bytes(
            &format!("/api/projects/{project_id}/assets/{sha256}"),
            token,
            bytes,
            &[],
        )?;
        match status {
            200 => Ok(()),
            401 => Err(Unauthorized.into()),
            s => bail!("uploading asset {sha256} failed: {}", describe(s, &text)),
        }
    }

    /// The whole linking flow on the current thread: get a code, hand it to
    /// `on_code` (which should open the browser and show the code), wait for
    /// approval, register the device. Returns the account to store.
    pub fn link(
        &self,
        device_name: &str,
        app_version: &str,
        on_code: impl FnOnce(&DeviceLink),
        keep_going: impl Fn() -> bool,
    ) -> Result<CloudAccount> {
        let link = self.start_link()?;
        on_code(&link);
        let token = self.wait_for_token(&link, keep_going)?;
        let (user, device) = self.register_device(&token, device_name, platform(), app_version)?;
        Ok(CloudAccount {
            token,
            user_id: user.id,
            email: user.email,
            device_id: device.id,
            device_name: device.name,
        })
    }
}

/// The token was rejected: the device was removed on the website, or the
/// session expired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unauthorized;

impl std::fmt::Display for Unauthorized {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("this machine is no longer linked (sign-in was revoked)")
    }
}

impl std::error::Error for Unauthorized {}

/// Whether an error chain is an [`Unauthorized`].
pub fn is_unauthorized(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.downcast_ref::<Unauthorized>().is_some())
}

/// The platform string the server accepts (`linux`, `macos`, …).
pub fn platform() -> &'static str {
    std::env::consts::OS
}

fn describe(status: u16, text: &str) -> String {
    if let Ok(e) = serde_json::from_str::<ErrorBody>(text) {
        for s in [&e.error_description, &e.message, &e.error] {
            if !s.is_empty() {
                return format!("{s} (HTTP {status})");
            }
        }
    }
    let text = text.trim();
    if text.is_empty() || text.len() > 200 || text.starts_with('<') {
        format!("HTTP {status}")
    } else {
        format!("{text} (HTTP {status})")
    }
}
