//! mDNS: advertise this copy as `_wordy._tcp` and watch for the others.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

pub const SERVICE_TYPE: &str = "_wordy._tcp.local.";
const PROP_PROJECT: &str = "project";
const PROP_PEER: &str = "peer";

/// Another Wordy on the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub name: String,
    pub project_id: String,
    pub addr: SocketAddr,
    pub fullname: String,
}

/// Browses for peers and (optionally) advertises us. Drop to stop.
pub struct Discovery {
    daemon: ServiceDaemon,
    peers: Arc<Mutex<HashMap<String, Peer>>>,
    generation: Arc<AtomicU64>,
    own_fullname: Option<String>,
}

/// We advertise under this name. Drop to unregister.
pub struct Advertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Discovery {
    pub fn start() -> Result<Self> {
        let daemon = ServiceDaemon::new().context("starting mDNS")?;
        let rx = daemon.browse(SERVICE_TYPE).context("browsing for peers")?;
        let peers: Arc<Mutex<HashMap<String, Peer>>> = Arc::default();
        let generation = Arc::new(AtomicU64::new(0));
        let (p2, g2) = (peers.clone(), generation.clone());
        thread::Builder::new()
            .name("wordy-mdns".into())
            .spawn(move || {
                while let Ok(ev) = rx.recv() {
                    match ev {
                        ServiceEvent::ServiceResolved(info) => {
                            let Some(addr) = pick_addr(info.addresses.iter().map(|a| a.to_ip_addr()), info.port) else {
                                continue;
                            };
                            let peer = Peer {
                                name: info
                                    .txt_properties
                                    .get_property_val_str(PROP_PEER)
                                    .unwrap_or("Wordy")
                                    .to_string(),
                                project_id: info
                                    .txt_properties
                                    .get_property_val_str(PROP_PROJECT)
                                    .unwrap_or_default()
                                    .to_string(),
                                addr,
                                fullname: info.fullname.clone(),
                            };
                            tracing::debug!("mdns: {} at {addr}", peer.name);
                            p2.lock().unwrap().insert(info.fullname.clone(), peer);
                            g2.fetch_add(1, Ordering::Relaxed);
                        }
                        ServiceEvent::ServiceRemoved(_, fullname) => {
                            if p2.lock().unwrap().remove(&fullname).is_some() {
                                g2.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        _ => {}
                    }
                }
            })
            .context("spawning the mDNS thread")?;
        Ok(Self {
            daemon,
            peers,
            generation,
            own_fullname: None,
        })
    }

    /// Advertise this copy. Peers with this exact name are hidden from `peers()`.
    pub fn advertise(&mut self, project_id: &str, peer_name: &str, port: u16) -> Result<Advertiser> {
        let host = gethostname::gethostname()
            .to_string_lossy()
            .trim_end_matches('.')
            .to_string();
        let host = host.strip_suffix(".local").unwrap_or(&host).to_string();
        let instance = format!("{peer_name} {}", std::process::id());
        let mut props = HashMap::new();
        props.insert(PROP_PROJECT.to_string(), project_id.to_string());
        props.insert(PROP_PEER.to_string(), peer_name.to_string());
        let info = ServiceInfo::new(SERVICE_TYPE, &instance, &format!("{host}.local."), (), port, props)
            .context("describing our service")?
            .enable_addr_auto();
        let fullname = info.get_fullname().to_string();
        self.daemon.register(info).context("registering with mDNS")?;
        self.own_fullname = Some(fullname.clone());
        tracing::info!("mdns: advertising {fullname} on port {port}");
        Ok(Advertiser {
            daemon: self.daemon.clone(),
            fullname,
        })
    }

    /// Bumps whenever the peer list changes; poll it cheaply.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// Current peers, stable order, without ourselves.
    pub fn peers(&self) -> Vec<Peer> {
        let mut v: Vec<Peer> = self
            .peers
            .lock()
            .unwrap()
            .values()
            .filter(|p| Some(&p.fullname) != self.own_fullname.as_ref())
            .cloned()
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name).then(a.addr.cmp(&b.addr)));
        v
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        let _ = self.daemon.stop_browse(SERVICE_TYPE);
        let _ = self.daemon.shutdown();
    }
}

impl Advertiser {
    pub fn fullname(&self) -> &str {
        &self.fullname
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
    }
}

/// Prefer a routable IPv4, then any IPv4, then IPv6.
fn pick_addr(addrs: impl Iterator<Item = IpAddr>, port: u16) -> Option<SocketAddr> {
    let addrs: Vec<IpAddr> = addrs.collect();
    let best = addrs
        .iter()
        .find(|a| matches!(a, IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_link_local()))
        .or_else(|| addrs.iter().find(|a| a.is_ipv4()))
        .or_else(|| addrs.first())?;
    Some(SocketAddr::new(*best, port))
}
