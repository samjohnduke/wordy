//! The project room's websocket protocol, mirrored from
//! `site/src/server/rooms/protocol.ts`. Text frames are JSON tagged by `t`;
//! binary frames carry one Loro blob, prefixed by its 8-byte big-endian
//! sequence number when it comes from the server.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
/// Blobs up to this size travel inline over the socket; bigger ones go
/// through HTTP and R2 (Workers cap websocket messages at 1 MiB).
pub const MAX_INLINE: usize = 512 * 1024;
/// Largest blob the server accepts over HTTP.
pub const MAX_BLOB: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssetEntry {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PresenceEntry {
    pub device: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMsg {
    Hello {
        version: u32,
        /// Last sequence number this device has applied (0 for never).
        since: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        dictionary: Vec<String>,
    },
    Dictionary {
        words: Vec<String>,
    },
    Asset {
        path: String,
        sha256: String,
        size: u64,
    },
    Ping,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMsg {
    Welcome {
        version: u32,
        head: u64,
        base: u64,
        from: u64,
        reset: bool,
        role: String,
        #[serde(default)]
        dictionary: Vec<String>,
        #[serde(default)]
        assets: Vec<AssetEntry>,
        #[serde(default)]
        log_bytes: u64,
        #[serde(default)]
        log_count: u64,
    },
    Blob {
        seq: u64,
        size: u64,
    },
    Synced {
        head: u64,
    },
    Ack {
        seq: u64,
    },
    Dictionary {
        words: Vec<String>,
    },
    Asset {
        path: String,
        sha256: String,
        size: u64,
    },
    BaseMoved {
        base: u64,
    },
    Presence {
        devices: Vec<PresenceEntry>,
    },
    /// The owner changed what this account may do in the project.
    Role {
        role: String,
    },
    Pong,
    Error {
        message: String,
    },
}

/// Split a server binary frame into its sequence number and the blob.
pub fn split_frame(frame: &[u8]) -> Result<(u64, &[u8])> {
    if frame.len() < 8 {
        bail!("binary frame too short ({} bytes)", frame.len());
    }
    let mut seq = [0u8; 8];
    seq.copy_from_slice(&frame[..8]);
    Ok((u64::from_be_bytes(seq), &frame[8..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip_the_server_shape() {
        let hello = ClientMsg::Hello {
            version: 1,
            since: 7,
            name: Some("Novel".into()),
            dictionary: vec!["Gondor".into()],
        };
        let json = serde_json::to_value(&hello).unwrap();
        assert_eq!(json["t"], "hello");
        assert_eq!(json["since"], 7);
        assert_eq!(json["dictionary"][0], "Gondor");
        let ping = serde_json::to_string(&ClientMsg::Ping).unwrap();
        assert_eq!(ping, r#"{"t":"ping"}"#);

        let welcome: ServerMsg = serde_json::from_str(
            r#"{"t":"welcome","version":1,"head":3,"base":2,"from":2,"reset":false,"role":"owner",
                "dictionary":["a"],"assets":[{"path":"x.png","sha256":"ab","size":3}],
                "log_bytes":10,"log_count":2}"#,
        )
        .unwrap();
        assert!(matches!(welcome, ServerMsg::Welcome { head: 3, base: 2, .. }));
        let moved: ServerMsg = serde_json::from_str(r#"{"t":"base_moved","base":9}"#).unwrap();
        assert_eq!(moved, ServerMsg::BaseMoved { base: 9 });
        assert_eq!(
            serde_json::from_str::<ServerMsg>(r#"{"t":"pong"}"#).unwrap(),
            ServerMsg::Pong
        );
        assert_eq!(
            serde_json::from_str::<ServerMsg>(r#"{"t":"role","role":"editor"}"#).unwrap(),
            ServerMsg::Role { role: "editor".into() }
        );
    }

    #[test]
    fn frames_split() {
        let mut frame = 5u64.to_be_bytes().to_vec();
        frame.extend_from_slice(b"hi");
        assert_eq!(split_frame(&frame).unwrap(), (5, &b"hi"[..]));
        assert!(split_frame(&[1, 2]).is_err());
    }
}
