//! What a stored NAR measured when it landed, recorded beside it at
//! `meta/nar/<hash>.nar.zst`.
//!
//! The narinfo signing path needs four numbers about the NAR it is about
//! to vouch for: the decompressed size and hash, and the compressed size
//! and hash. Measuring them means reading every byte and decoding the
//! zstd frame, which is the most expensive work the worker does. Doing it
//! on the narinfo request means doing it after the bytes are already
//! stored, so the worker reads the whole object back out of the bucket to
//! learn what it just wrote.
//!
//! This document moves that work to the request that stores the bytes,
//! where the bytes are already streaming past. The write measures as it
//! stores and records the result here; the narinfo request reads a few
//! hundred bytes instead of a few hundred megabytes. The measurement is
//! unchanged, and so is the order it happens in: this document exists
//! only for a NAR whose bytes were measured in full, so a narinfo can
//! still be signed only after its NAR verifies (CLAUDE.md §1).

use crate::constants::NAR_FACTS_KEY_PREFIX;
use crate::keys::NarKey;

/// The measured facts of one stored NAR.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NarFacts {
    /// The decompressed bytes' hash, as `sha256:<nix32>`.
    #[serde(rename = "narHash")]
    pub nar_hash: String,
    /// How many bytes the NAR decompressed to.
    #[serde(rename = "narSizeBytes")]
    pub nar_size_bytes: u64,
    /// The stored bytes' hash, bare nix32, as a NAR key spells it.
    #[serde(rename = "fileHash")]
    pub file_hash_nix32: String,
    /// How many bytes the object holds.
    #[serde(rename = "fileSizeBytes")]
    pub file_size_bytes: u64,
}

/// Where one NAR's facts live. A NAR key already begins `nar/`, so the
/// reserved `meta/` prefix in front of it names the facts without a
/// second grammar to keep in step: `meta/nar/<hash>.nar.zst` belongs to
/// `nar/<hash>.nar.zst` and to nothing else.
pub fn facts_key(nar_key: &NarKey) -> String {
    format!("{NAR_FACTS_KEY_PREFIX}{}", nar_key.as_str())
}

/// The same key from a NAR's bucket key, for callers holding the string
/// form (the collector walks keys, not parsed types).
pub fn facts_key_for(nar_bucket_key: &str) -> String {
    format!("{NAR_FACTS_KEY_PREFIX}{nar_bucket_key}")
}

impl NarFacts {
    /// Serialize with a trailing newline.
    pub fn serialize(&self) -> String {
        let mut body = serde_json::to_string(self).expect("string and numeric fields");
        body.push('\n');
        body
    }

    /// Parse a stored document. This is the worker's own writing, so a
    /// parse failure is a storage fault rather than anything a client did.
    ///
    /// # Errors
    ///
    /// [`serde_json::Error`] on invalid JSON or a schema mismatch.
    pub fn parse(text: &str) -> std::result::Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> NarFacts {
        NarFacts {
            nar_hash: format!("sha256:{}", "0".repeat(52)),
            nar_size_bytes: 409_205_960,
            file_hash_nix32: "g".repeat(52),
            file_size_bytes: 131_961_582,
        }
    }

    #[test]
    fn round_trip() {
        assert_eq!(
            NarFacts::parse(&facts().serialize()).expect("the own form parses"),
            facts()
        );
    }

    #[test]
    fn a_facts_document_stays_far_under_its_cap() {
        assert!(
            facts().serialize().len()
                < usize::try_from(crate::constants::NAR_FACTS_BYTES_MAX).expect("fits"),
            "a facts document fits its read cap"
        );
    }

    #[test]
    fn the_key_is_the_nar_key_under_the_reserved_prefix() {
        let nar =
            crate::keys::parse_nar_key(&format!("nar/{}.nar.zst", "g".repeat(52))).expect("valid");
        assert_eq!(
            facts_key(&nar),
            format!("meta/nar/{}.nar.zst", "g".repeat(52))
        );
        assert_eq!(facts_key(&nar), facts_key_for(nar.as_str()));
        assert!(
            crate::keys::is_reserved_key(&facts_key(&nar)),
            "facts are unreachable from a request"
        );
    }
}
