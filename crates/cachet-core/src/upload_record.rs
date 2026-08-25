//! The multipart bookkeeping record at `uploads/{uploadId}`: the server's
//! copy of the part plan and the declared total. `resumeMultipartUpload`
//! carries no state, so part sizes are verified against this document, and
//! completion verifies the assembled byte count against it, which is the
//! only reason a part-number collision cannot silently corrupt an object.

use crate::constants::UPLOAD_STALE_MAX_MS;
use crate::types::UnixMillis;

/// The bookkeeping record for one in-flight multipart upload.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UploadRecord {
    /// The destination bucket key of the object being assembled.
    pub key: String,
    /// The client-declared total size of the object.
    #[serde(rename = "totalBytes")]
    pub total_bytes: u64,
    /// The number of parts the plan expects, from `plan_shape`.
    #[serde(rename = "expectedParts")]
    pub expected_parts: u64,
    /// What the client declared the assembled NAR decompresses to. The
    /// completion measures the assembled object, and a decoder needs its
    /// ceiling before it starts; the parts arrive out of order, so the
    /// declaration has to be recorded when the upload opens.
    #[serde(rename = "narBytes")]
    pub nar_bytes: u64,
    /// When the upload was created, in epoch milliseconds.
    #[serde(rename = "createdAtMs")]
    pub created_at_ms: u64,
}

impl UploadRecord {
    /// Serialize with a trailing newline.
    pub fn serialize(&self) -> String {
        let mut body = serde_json::to_string(self).expect("string and numeric fields");
        body.push('\n');
        body
    }

    /// Parse a stored record. Internal state, so a parse failure is the
    /// caller's storage error, not a client error.
    ///
    /// # Errors
    ///
    /// [`serde_json::Error`] on invalid JSON or a schema mismatch.
    pub fn parse(text: &str) -> std::result::Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Whether the upload has been idle long enough to reap.
    pub fn is_stale(&self, now: UnixMillis) -> bool {
        now.saturating_ms_since(UnixMillis::new(self.created_at_ms)) > UPLOAD_STALE_MAX_MS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let record = UploadRecord {
            key: format!("nar/{}.nar.zst", "x".repeat(52)),
            total_bytes: 3_000,
            expected_parts: 2,
            nar_bytes: 9_000,
            created_at_ms: 1_780_000_000_000,
        };
        assert_eq!(
            UploadRecord::parse(&record.serialize()).expect("the own form parses"),
            record
        );
    }

    #[test]
    fn staleness_uses_a_forward_delta() {
        let record = UploadRecord {
            key: String::new(),
            total_bytes: 1,
            expected_parts: 1,
            nar_bytes: 1,
            created_at_ms: 1_000,
        };
        assert!(!record.is_stale(UnixMillis::new(500)));
        assert!(record.is_stale(UnixMillis::new(1_000 + UPLOAD_STALE_MAX_MS + 1)));
    }
}
