//! Wire format: length-prefixed frames over TCP.
//!
//! Every frame is a `u32` big-endian length followed by that many bytes. A
//! control frame holds one JSON [`Msg`]; the messages that carry a payload
//! (`Updates`, `File`) are immediately followed by one raw binary frame.

use std::io::{Read, Write};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Refuse frames bigger than this (a snapshot of a whole novel is a few MB).
pub const MAX_FRAME: usize = 1 << 30;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "t")]
pub enum Msg {
    /// First message both ways.
    Hello { version: u32, project_id: String, peer: String, nonce: String },
    /// HMAC-SHA256(pairing code, role ‖ client nonce ‖ server nonce), hex.
    Auth { proof: String },
    /// Any side may send this instead of the expected message and hang up.
    Reject { reason: String },
    /// Loro version vector, encoded.
    Version { vv: Vec<u8> },
    /// Followed by one binary frame of Loro updates (may be empty).
    Updates { len: u64, ops: u64 },
    /// Every asset we have, by relative path.
    Manifest { files: Vec<FileEntry> },
    /// The entries of the other side's manifest that we are missing.
    Want { paths: Vec<String> },
    /// Followed by one binary frame with the file's bytes.
    File { path: String, len: u64 },
    /// No more files from this side.
    FilesDone,
    /// Our custom dictionary.
    Dictionary { words: Vec<String> },
    /// All done, both ways.
    Bye,
}

pub fn write_frame<W: Write>(w: &mut W, bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_FRAME {
        bail!("frame too large: {} bytes", bytes.len());
    }
    w.write_all(&(bytes.len() as u32).to_be_bytes())?;
    w.write_all(bytes)?;
    w.flush()?;
    Ok(())
}

pub fn read_frame<R: Read>(r: &mut R) -> Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).context("connection closed")?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        bail!("frame too large: {len} bytes");
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).context("connection closed mid-frame")?;
    Ok(buf)
}

pub fn write_msg<W: Write>(w: &mut W, msg: &Msg) -> Result<()> {
    let json = serde_json::to_vec(msg)?;
    write_frame(w, &json)
}

pub fn read_msg<R: Read>(r: &mut R) -> Result<Msg> {
    let bytes = read_frame(r)?;
    let msg: Msg = serde_json::from_slice(&bytes).context("malformed message")?;
    if let Msg::Reject { reason } = &msg {
        bail!("the other side refused: {reason}");
    }
    Ok(msg)
}

pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Proof that we know the pairing code, bound to this connection's nonces.
pub fn proof(code: &str, role: &str, client_nonce: &str, server_nonce: &str) -> String {
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;
    let mut mac = Hmac::<Sha256>::new_from_slice(code.trim().as_bytes()).expect("hmac accepts any key length");
    mac.update(role.as_bytes());
    mac.update(b"\0");
    mac.update(client_nonce.as_bytes());
    mac.update(b"\0");
    mac.update(server_nonce.as_bytes());
    hex(&mac.finalize().into_bytes())
}

/// Constant-time-ish comparison of two hex proofs.
pub fn proof_matches(expected: &str, got: &str) -> bool {
    if expected.len() != got.len() {
        return false;
    }
    expected.bytes().zip(got.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex(&Sha256::digest(bytes))
}

pub fn nonce() -> String {
    ulid::Ulid::new().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mut buf = Vec::new();
        write_msg(&mut buf, &Msg::Want { paths: vec!["a.png".into()] }).unwrap();
        write_frame(&mut buf, b"payload").unwrap();
        let mut r = &buf[..];
        assert_eq!(read_msg(&mut r).unwrap(), Msg::Want { paths: vec!["a.png".into()] });
        assert_eq!(read_frame(&mut r).unwrap(), b"payload");
    }

    #[test]
    fn reject_becomes_error() {
        let mut buf = Vec::new();
        write_msg(&mut buf, &Msg::Reject { reason: "nope".into() }).unwrap();
        let err = read_msg(&mut &buf[..]).unwrap_err();
        assert!(err.to_string().contains("nope"));
    }

    #[test]
    fn proofs_depend_on_everything() {
        let p = proof("1234", "client", "a", "b");
        assert!(proof_matches(&p, &proof("1234", "client", "a", "b")));
        assert!(!proof_matches(&p, &proof("1235", "client", "a", "b")));
        assert!(!proof_matches(&p, &proof("1234", "server", "a", "b")));
        assert!(!proof_matches(&p, &proof("1234", "client", "a", "c")));
        assert!(proof_matches(&proof(" 1234 ", "client", "a", "b"), &p));
    }
}
