//! The generation document at `meta/generation`: the edge cache's global
//! epoch. Cache keys compose it, so one destructive sweep invalidates
//! every cached object within the document's own edge TTL. It lives in R2
//! with everything else, so it can corrupt like anything else, and the
//! read path treats corruption as "no edge caching this request" rather
//! than as a generation to trust: never assume zero.

use crate::constants::{EDGE_CACHE_KEY_ORIGIN, GENERATION_DOCUMENT_BYTES_MAX};

/// The document's parse failure. Not a client error: the document is
/// internal state, and its reader decides how to degrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerationCorrupt;

/// The generation document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GenerationDocument {
    /// The monotonic edge-caching epoch. Absent on disk means zero.
    pub generation: u64,
    /// When the last destructive sweep bumped it, in epoch milliseconds.
    #[serde(rename = "bumpedAtMs")]
    pub bumped_at_ms: u64,
}

impl GenerationDocument {
    /// The document an empty bucket starts from.
    pub const ZERO: Self = Self {
        generation: 0,
        bumped_at_ms: 0,
    };

    /// Serialize with a trailing newline, the shape the first cachet
    /// deployment wrote.
    pub fn serialize(&self) -> String {
        let mut body = serde_json::to_string(self).expect("numeric fields");
        body.push('\n');
        body
    }

    /// Parse the document, bounded before reading: a well-formed document
    /// is under a hundred bytes, so anything approaching the cap is
    /// corrupt without ever reaching the parser.
    ///
    /// # Errors
    ///
    /// [`GenerationCorrupt`] over the byte cap, on invalid JSON, or on a
    /// missing or non-numeric field.
    pub fn parse(text: &str) -> std::result::Result<Self, GenerationCorrupt> {
        if u64::try_from(text.len()).expect("len fits") > GENERATION_DOCUMENT_BYTES_MAX {
            return Err(GenerationCorrupt);
        }
        serde_json::from_str(text).map_err(|_| GenerationCorrupt)
    }

    /// The document after one destructive sweep.
    #[must_use]
    pub fn bump(&self, now_ms: u64) -> Self {
        Self {
            generation: self.generation + 1,
            bumped_at_ms: now_ms,
        }
    }
}

/// The edge-cache key for an object, built under the synthetic origin
/// rather than the request's own, so the key is ours and the generation
/// can be part of it. Bumping the generation changes every object's cache
/// key at once: no point of presence is asked for its stale entries again,
/// and no purge API or per-key walk is needed.
#[must_use]
pub fn object_cache_key(generation: u64, request_path: &str) -> String {
    let path = request_path.strip_prefix('/').unwrap_or(request_path);
    format!("{EDGE_CACHE_KEY_ORIGIN}/g{generation}/{path}")
}

/// The edge-cache key for the generation document itself. It sits outside
/// the generation-prefixed space: its short TTL is the bound on believing
/// a stale generation, so it cannot be keyed by the value it carries.
#[must_use]
pub fn generation_cache_key() -> String {
    format!("{EDGE_CACHE_KEY_ORIGIN}/meta/generation")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let document = GenerationDocument {
            generation: 7,
            bumped_at_ms: 1_780_000_000_000,
        };
        assert_eq!(
            GenerationDocument::parse(&document.serialize()),
            Ok(document)
        );
    }

    #[test]
    fn corrupt_documents_are_typed() {
        for text in [
            "",
            "not json",
            "{}",
            "{\"generation\":\"7\"}",
            &"x".repeat(300),
        ] {
            assert_eq!(GenerationDocument::parse(text), Err(GenerationCorrupt));
        }
    }

    #[test]
    fn cache_keys_compose_the_generation() {
        assert_eq!(
            object_cache_key(7, "/abc.narinfo"),
            "https://cachet-edge.invalid/g7/abc.narinfo"
        );
        assert_eq!(
            object_cache_key(7, "nar/xyz"),
            "https://cachet-edge.invalid/g7/nar/xyz"
        );
        assert_eq!(
            generation_cache_key(),
            "https://cachet-edge.invalid/meta/generation"
        );
    }
}
