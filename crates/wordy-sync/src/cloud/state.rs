//! What a project remembers about its cloud room, in `<project>/cloud.json`:
//! whether it syncs at all, the last sequence number applied and the
//! version vector the server is known to hold (so pushes send only what is
//! new). Both are advisory: a stale file just means some re-sending.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use loro::VersionVector;
use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "cloud.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct CloudState {
    /// The user turned cloud sync on for this project.
    pub enabled: bool,
    /// Last room sequence number applied locally.
    pub last_seq: u64,
    /// Encoded version vector of everything the server has from us or that
    /// we got from it; empty means "push a snapshot first".
    #[serde(with = "hex_bytes")]
    pub pushed_vv: Vec<u8>,
}

impl CloudState {
    pub fn path(project_dir: &Path) -> PathBuf {
        project_dir.join(FILE_NAME)
    }

    pub fn load(project_dir: &Path) -> Self {
        match std::fs::read(Self::path(project_dir)) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, project_dir: &Path) -> Result<()> {
        let path = Self::path(project_dir);
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self)?;
        std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("renaming to {}", path.display()))?;
        Ok(())
    }

    pub fn vv(&self) -> VersionVector {
        if self.pushed_vv.is_empty() {
            return VersionVector::new();
        }
        VersionVector::decode(&self.pushed_vv).unwrap_or_default()
    }

    pub fn set_vv(&mut self, vv: &VersionVector) {
        self.pushed_vv = if vv.is_empty() { Vec::new() } else { vv.encode() };
    }
}

mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&crate::protocol::hex(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        if s.len() % 2 != 0 {
            return Err(serde::de::Error::custom("odd hex length"));
        }
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(serde::de::Error::custom))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loro::LoroDoc;

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(CloudState::load(dir.path()), CloudState::default());
        let doc = LoroDoc::new();
        doc.get_text("t").insert(0, "hello").unwrap();
        doc.commit();
        let mut st = CloudState {
            enabled: true,
            last_seq: 42,
            pushed_vv: Vec::new(),
        };
        st.set_vv(&doc.oplog_vv());
        st.save(dir.path()).unwrap();
        let back = CloudState::load(dir.path());
        assert_eq!(back, st);
        assert_eq!(back.vv(), doc.oplog_vv());
        assert!(back.enabled && back.last_seq == 42);
    }
}

/// Ops that `after` has beyond `before` arrived from the cloud, so `pushed`
/// may skip ahead over them: per peer, as long as nothing of that peer was
/// still waiting to go up (then it all goes up together, which is harmless).
pub fn absorb_remote(pushed: &mut VersionVector, before: &VersionVector, after: &VersionVector) {
    for (peer, end) in after.iter() {
        let had = before.get(peer).copied().unwrap_or(0);
        let sent = pushed.get(peer).copied().unwrap_or(0);
        if *end > had && sent >= had {
            pushed.set_end(loro::ID::new(*peer, *end));
        }
    }
}

#[cfg(test)]
mod absorb_tests {
    use super::*;
    use loro::ID;

    fn vv(pairs: &[(u64, i32)]) -> VersionVector {
        let mut v = VersionVector::new();
        for (peer, end) in pairs {
            v.set_end(ID::new(*peer, *end));
        }
        v
    }

    #[test]
    fn absorb_skips_remote_ops_but_not_unsent_local_ones() {
        // Peer 1 is us (all pushed), peer 2 arrived from the cloud, peer 3
        // came by LAN sync earlier and was never pushed.
        let before = vv(&[(1, 10), (3, 5)]);
        let after = vv(&[(1, 10), (2, 7), (3, 9)]);
        let mut pushed = vv(&[(1, 10), (3, 2)]);
        absorb_remote(&mut pushed, &before, &after);
        assert_eq!(pushed.get(&1), Some(&10));
        assert_eq!(pushed.get(&2), Some(&7), "remote peer skipped ahead");
        assert_eq!(pushed.get(&3), Some(&2), "peer 3 still has unsent ops");
    }
}
