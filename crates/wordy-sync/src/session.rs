//! One sync conversation over an established TCP stream.
//!
//! Both sides run the same steps in the same order; the initiator (the side
//! that pressed Sync) always sends first within a step and the responder
//! answers, so the protocol never deadlocks and needs no concurrency.

use std::collections::HashMap;
use std::io::{BufReader, BufWriter};
use std::net::TcpStream;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use wordy_doc::loro::{ExportMode, LoroDoc, VersionVector};

use crate::protocol::{self, read_frame, read_msg, write_frame, write_msg, FileEntry, Msg};
use crate::{LocalState, SyncOutcome, PROTOCOL_VERSION};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Initiator,
    Responder,
}

impl Role {
    fn label(self) -> &'static str {
        match self {
            Role::Initiator => "client",
            Role::Responder => "server",
        }
    }
}

const IO_TIMEOUT: Duration = Duration::from_secs(60);

struct Session<'a> {
    role: Role,
    state: &'a LocalState,
    r: BufReader<TcpStream>,
    w: BufWriter<TcpStream>,
}

/// Run a whole sync over `stream`. Returns what to apply locally.
pub fn run(stream: TcpStream, role: Role, state: &LocalState) -> Result<SyncOutcome> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    stream.set_nodelay(true)?;
    let r = BufReader::new(stream.try_clone()?);
    let w = BufWriter::new(stream);
    let mut s = Session { role, state, r, w };
    s.run()
}

impl Session<'_> {
    fn send(&mut self, msg: &Msg) -> Result<()> {
        write_msg(&mut self.w, msg)
    }
    fn recv(&mut self) -> Result<Msg> {
        read_msg(&mut self.r)
    }
    fn reject(&mut self, reason: &str) -> Result<()> {
        let _ = self.send(&Msg::Reject {
            reason: reason.to_string(),
        });
        bail!("{reason}")
    }

    /// Send `ours` and receive theirs, in role order, checking what arrives
    /// before we answer (responder) or before we go on (initiator). A failed
    /// check tells the other side why and aborts.
    fn exchange(&mut self, ours: Msg, check: impl Fn(&Msg) -> Result<()>) -> Result<Msg> {
        let theirs = match self.role {
            Role::Initiator => {
                self.send(&ours)?;
                let theirs = self.recv()?;
                if let Err(e) = check(&theirs) {
                    self.reject(&e.to_string())?;
                }
                theirs
            }
            Role::Responder => {
                let theirs = self.recv()?;
                if let Err(e) = check(&theirs) {
                    self.reject(&e.to_string())?;
                }
                self.send(&ours)?;
                theirs
            }
        };
        Ok(theirs)
    }

    /// Like `exchange` for a message followed by one binary frame.
    fn exchange_payload(&mut self, ours: Msg, bytes: &[u8]) -> Result<(Msg, Vec<u8>)> {
        match self.role {
            Role::Initiator => {
                self.send(&ours)?;
                write_frame(&mut self.w, bytes)?;
                let theirs = self.recv()?;
                let payload = read_frame(&mut self.r)?;
                Ok((theirs, payload))
            }
            Role::Responder => {
                let theirs = self.recv()?;
                let payload = read_frame(&mut self.r)?;
                self.send(&ours)?;
                write_frame(&mut self.w, bytes)?;
                Ok((theirs, payload))
            }
        }
    }

    fn run(&mut self) -> Result<SyncOutcome> {
        let mut outcome = SyncOutcome::default();

        // 1. Hello: same protocol, same project.
        let my_nonce = protocol::nonce();
        let hello = Msg::Hello {
            version: PROTOCOL_VERSION,
            project_id: self.state.project_id.clone(),
            peer: self.state.peer_name.clone(),
            nonce: my_nonce.clone(),
        };
        let my_id = self.state.project_id.clone();
        let theirs = self.exchange(hello, |m| match m {
            Msg::Hello { version, .. } if *version != PROTOCOL_VERSION => {
                bail!("the other Wordy speaks sync protocol v{version}, this one v{PROTOCOL_VERSION}; update both")
            }
            Msg::Hello { project_id, .. } if *project_id != my_id => bail!(
                "that is a different project (id {project_id} vs {my_id}); copy this project folder or a backup zip to the other machine first"
            ),
            Msg::Hello { .. } => Ok(()),
            other => bail!("expected Hello, got {other:?}"),
        })?;
        let Msg::Hello {
            peer,
            nonce: their_nonce,
            ..
        } = theirs
        else {
            unreachable!()
        };
        outcome.peer = peer;
        let (client_nonce, server_nonce) = match self.role {
            Role::Initiator => (my_nonce.clone(), their_nonce.clone()),
            Role::Responder => (their_nonce.clone(), my_nonce.clone()),
        };

        // 2. Pairing: each side proves it knows the code, bound to both nonces.
        let code = self.state.pairing_code.clone();
        if code.trim().is_empty() {
            self.reject("no pairing code set on this side")?;
        }
        let mine = protocol::proof(&code, self.role.label(), &client_nonce, &server_nonce);
        let other_role = match self.role {
            Role::Initiator => Role::Responder,
            Role::Responder => Role::Initiator,
        };
        let expected = protocol::proof(&code, other_role.label(), &client_nonce, &server_nonce);
        self.exchange(Msg::Auth { proof: mine }, |m| match m {
            Msg::Auth { proof } if protocol::proof_matches(&expected, proof) => Ok(()),
            Msg::Auth { .. } => bail!("pairing code does not match"),
            other => bail!("expected Auth, got {other:?}"),
        })?;

        // 3. Versions, then the updates each side is missing.
        let doc = LoroDoc::new();
        doc.import(&self.state.snapshot)
            .map_err(|e| anyhow!("local snapshot: {e}"))?;
        let my_vv = doc.oplog_vv();
        let theirs = self.exchange(Msg::Version { vv: my_vv.encode() }, |m| match m {
            Msg::Version { .. } => Ok(()),
            other => bail!("expected Version, got {other:?}"),
        })?;
        let Msg::Version { vv } = theirs else { unreachable!() };
        let their_vv = VersionVector::decode(&vv).map_err(|e| anyhow!("peer version vector: {e}"))?;
        // An export with nothing new is still a few header bytes; send none.
        let ops_out = ops_between(&their_vv, &my_vv);
        let out = if ops_out == 0 {
            Vec::new()
        } else {
            doc.export(ExportMode::updates(&their_vv))
                .map_err(|e| anyhow!("export updates: {e}"))?
        };
        let (theirs, incoming) = self.exchange_payload(
            Msg::Updates {
                len: out.len() as u64,
                ops: ops_out as u64,
            },
            &out,
        )?;
        let Msg::Updates { len, .. } = theirs else {
            bail!("expected Updates, got {theirs:?}")
        };
        if len as usize != incoming.len() {
            bail!("updates frame was {} bytes, header said {len}", incoming.len());
        }
        outcome.bytes_out = out.len();
        outcome.bytes_in = incoming.len();
        outcome.ops_out = ops_out;
        if !incoming.is_empty() {
            // Validate on the scratch copy before the UI touches the live doc.
            doc.import(&incoming).map_err(|e| {
                let msg = e.to_string();
                if msg.contains("shallow") {
                    anyhow!(
                        "peer updates do not apply: one copy compacted its history after the other \
                         last synced; copy the project folder across instead ({msg})"
                    )
                } else {
                    anyhow!("peer updates do not apply: {msg}")
                }
            })?;
            outcome.ops_in = ops_between(&my_vv, &doc.oplog_vv());
            if outcome.ops_in > 0 {
                outcome.incoming_updates = incoming;
            }
        }

        // 4. Assets: swap manifests, ask for what is missing, copy both ways.
        let mine = manifest(&self.state.assets_dir)?;
        let theirs = self.exchange(Msg::Manifest { files: mine.clone() }, |m| match m {
            Msg::Manifest { .. } => Ok(()),
            other => bail!("expected Manifest, got {other:?}"),
        })?;
        let Msg::Manifest { files: their_files } = theirs else {
            unreachable!()
        };
        let mine_by_path: HashMap<&str, &FileEntry> = mine.iter().map(|f| (f.path.as_str(), f)).collect();
        let mut wants = Vec::new();
        let mut expected_hash: HashMap<String, String> = HashMap::new();
        for f in &their_files {
            if !safe_relative(&f.path) {
                tracing::warn!("ignoring unsafe asset path from peer: {}", f.path);
                continue;
            }
            match mine_by_path.get(f.path.as_str()) {
                None => {
                    wants.push(f.path.clone());
                    expected_hash.insert(f.path.clone(), f.sha256.clone());
                }
                Some(local) if local.sha256 != f.sha256 => {
                    tracing::warn!("asset {} differs on both sides; keeping the local copy", f.path);
                }
                Some(_) => {}
            }
        }
        let theirs = self.exchange(Msg::Want { paths: wants.clone() }, |m| match m {
            Msg::Want { .. } => Ok(()),
            other => bail!("expected Want, got {other:?}"),
        })?;
        let Msg::Want { paths: they_want } = theirs else {
            unreachable!()
        };
        let they_want: Vec<String> = they_want
            .into_iter()
            .filter(|p| mine_by_path.contains_key(p.as_str()))
            .collect();
        match self.role {
            Role::Initiator => {
                self.send_files(&they_want, &mut outcome)?;
                self.receive_files(&expected_hash, &mut outcome)?;
            }
            Role::Responder => {
                self.receive_files(&expected_hash, &mut outcome)?;
                self.send_files(&they_want, &mut outcome)?;
            }
        }

        // 5. Dictionary union.
        let theirs = self.exchange(
            Msg::Dictionary {
                words: self.state.dictionary.clone(),
            },
            |m| match m {
                Msg::Dictionary { .. } => Ok(()),
                other => bail!("expected Dictionary, got {other:?}"),
            },
        )?;
        let Msg::Dictionary { words } = theirs else {
            unreachable!()
        };
        outcome.new_words = words
            .into_iter()
            .filter(|w| !w.trim().is_empty() && !self.state.dictionary.contains(w))
            .collect();
        outcome.new_words.sort_unstable();
        outcome.new_words.dedup();

        // 6. Bye.
        self.exchange(Msg::Bye, |m| match m {
            Msg::Bye => Ok(()),
            other => bail!("expected Bye, got {other:?}"),
        })?;
        Ok(outcome)
    }

    fn send_files(&mut self, paths: &[String], outcome: &mut SyncOutcome) -> Result<()> {
        for rel in paths {
            let path = self.state.assets_dir.join(rel);
            let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            self.send(&Msg::File {
                path: rel.clone(),
                len: bytes.len() as u64,
            })?;
            write_frame(&mut self.w, &bytes)?;
            outcome.files_out.push(rel.clone());
        }
        self.send(&Msg::FilesDone)
    }

    fn receive_files(&mut self, expected: &HashMap<String, String>, outcome: &mut SyncOutcome) -> Result<()> {
        loop {
            match self.recv()? {
                Msg::FilesDone => return Ok(()),
                Msg::File { path, len } => {
                    let bytes = read_frame(&mut self.r)?;
                    if bytes.len() as u64 != len {
                        bail!("file {path} was {} bytes, header said {len}", bytes.len());
                    }
                    let Some(hash) = expected.get(&path) else {
                        tracing::warn!("peer sent unrequested file {path}; ignoring");
                        continue;
                    };
                    if protocol::sha256_hex(&bytes) != *hash {
                        bail!("file {path} arrived corrupted (hash mismatch)");
                    }
                    let dest = self.state.assets_dir.join(&path);
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    let tmp = dest.with_extension("part.tmp");
                    std::fs::write(&tmp, &bytes)?;
                    std::fs::rename(&tmp, &dest)?;
                    outcome.files_in.push(path);
                }
                other => bail!("expected File or FilesDone, got {other:?}"),
            }
        }
    }
}

/// Number of ops in `to` that `from` does not have.
pub fn ops_between(from: &VersionVector, to: &VersionVector) -> usize {
    to.iter()
        .map(|(peer, counter)| {
            let had = from.get(peer).copied().unwrap_or(0);
            (*counter - had).max(0) as usize
        })
        .sum()
}

/// Relative, no parent components, forward slashes only.
pub fn safe_relative(path: &str) -> bool {
    if path.is_empty() || path.contains('\\') || path.contains('\0') {
        return false;
    }
    Path::new(path).components().all(|c| matches!(c, Component::Normal(_)))
}

/// Every file under `dir` (recursively) with its size and hash.
pub fn manifest(dir: &Path) -> Result<Vec<FileEntry>> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        for entry in std::fs::read_dir(dir.join(&rel))? {
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
            } else if ty.is_file() {
                let bytes = std::fs::read(entry.path())?;
                let path = rel_path
                    .components()
                    .filter_map(|c| c.as_os_str().to_str())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push(FileEntry {
                    path,
                    size: bytes.len() as u64,
                    sha256: protocol::sha256_hex(&bytes),
                });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_paths() {
        assert!(safe_relative("abc.png"));
        assert!(safe_relative("sub/abc.png"));
        assert!(!safe_relative("../abc.png"));
        assert!(!safe_relative("/abc.png"));
        assert!(!safe_relative("a\\b.png"));
        assert!(!safe_relative(""));
    }

    #[test]
    fn ops_between_counts_only_new() {
        let a = VersionVector::from(vec![wordy_doc::loro::ID::new(1, 4), wordy_doc::loro::ID::new(2, 9)]);
        let b = VersionVector::from(vec![wordy_doc::loro::ID::new(1, 9), wordy_doc::loro::ID::new(3, 2)]);
        // from a to b: peer 1 gained 5, peer 3 is new with 3 ops (counter 2 → 3 ops), peer 2 dropped (ignored).
        assert_eq!(ops_between(&a, &b), 5 + 3);
    }
}
