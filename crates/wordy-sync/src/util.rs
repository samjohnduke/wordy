//! Small helpers shared by the cloud code: hashing and path checks.

use std::path::{Component, Path};

/// Lower-case hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// SHA-256 of `bytes` as hex, the asset name used on disk and on the server.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex(&Sha256::digest(bytes))
}

/// Relative, no parent components, forward slashes only.
pub fn safe_relative(path: &str) -> bool {
    if path.is_empty() || path.contains('\\') || path.contains('\0') {
        return false;
    }
    Path::new(path).components().all(|c| matches!(c, Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_and_hash() {
        assert_eq!(hex(&[0, 255, 16]), "00ff10");
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn safe_paths() {
        assert!(safe_relative("a/b.png"));
        assert!(!safe_relative(""));
        assert!(!safe_relative("../x"));
        assert!(!safe_relative("/abs"));
        assert!(!safe_relative("a\\b"));
    }
}
