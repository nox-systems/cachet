//! SHA-256: the one-line answer over a slice, plus the incremental form
//! the streaming pipeline uses (hash what passes through, never buffer).

use sha2::Digest;

/// SHA-256 digest of `bytes`.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

/// Lowercase hex of a digest, for the KV keys that index tokens by their
/// hash rather than by the token itself.
pub fn hex_digest(digest: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

/// The incremental hasher: one allocation-free driver for the tee paths in
/// the upload verification pipeline.
#[derive(Debug, Default, Clone)]
pub struct Sha256Stream {
    inner: sha2::Sha256,
    byte_count: u64,
}

impl Sha256Stream {
    /// Start an empty stream.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed bytes.
    pub fn update(&mut self, bytes: &[u8]) {
        self.byte_count = self
            .byte_count
            .checked_add(u64::try_from(bytes.len()).expect("len fits"))
            .expect("2^64 bytes is past any budget");
        self.inner.update(bytes);
    }

    /// The digest so far. Finalization is consumption-free: the stream can
    /// keep counting after a read (used when a pipeline verifies while
    /// still streaming).
    pub fn digest_so_far(&self) -> [u8; 32] {
        self.inner.clone().finalize().into()
    }

    /// Bytes seen.
    pub const fn byte_count(&self) -> u64 {
        self.byte_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_rfc_vector() {
        let digest = sha256(b"abc");
        assert_eq!(
            hex::encode(digest),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn streaming_equals_one_shot() {
        let mut stream = Sha256Stream::new();
        stream.update(b"a");
        stream.update(b"bc");
        assert_eq!(stream.digest_so_far(), sha256(b"abc"));
        assert_eq!(stream.byte_count(), 3);
    }
}
