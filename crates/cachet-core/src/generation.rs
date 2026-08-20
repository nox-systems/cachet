//! The generation document at `meta/generation`: the edge cache's global
//! epoch. Cache keys compose it, so one destructive sweep invalidates
//! every cached object within the document's own edge TTL. It lives in R2
//! with everything else, so it can corrupt like anything else, and the
//! read path treats corruption as "no edge caching this request" rather
//! than as a generation to trust: never assume zero.

use crate::constants::GENERATION_DOCUMENT_BYTES_MAX;

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
}
