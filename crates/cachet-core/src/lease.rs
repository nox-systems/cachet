//! The lease document: one per project, under `roots/{project}`.
//!
//! A lease is cachet's only garbage-collection state. There is no database:
//! the collector reads these documents, walks the closures of the paths
//! they name, and treats everything unmarked and older than the grace
//! window as collectable.
//!
//! Two consequences follow. Last write wins: a lease asserts what a project
//! needs now rather than a history of what it once needed, so a renewal
//! replaces the previous document wholesale and a path that drops out of
//! the closure stops being protected — which is the only reason the cache
//! does not grow forever. And provenance comes from the verified OIDC
//! token, never from the request body: a commit SHA a client could choose
//! for itself would be worthless as an audit trail.

use crate::constants::UNKNOWN_CLAIM;
use crate::error::{ClientError, Result};
use crate::types::{ProjectName, UnixMillis};

/// What `roots/{project}` holds.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LeaseDocument {
    /// The project this lease covers; also the key suffix.
    pub project: String,
    /// When this lease was last renewed, in epoch milliseconds. The
    /// collector compares it against lease retention.
    #[serde(rename = "renewedAtMs")]
    pub renewed_at_ms: u64,
    /// `owner/repo`, from the verified token.
    pub repository: String,
    /// The ref that renewed it: always the configured default branch by the
    /// time it is written.
    #[serde(rename = "ref")]
    pub ref_: String,
    /// The workflow run, from the verified token.
    #[serde(rename = "runId")]
    pub run_id: String,
    /// The commit that built these paths, from the verified token.
    #[serde(rename = "commitSha")]
    pub commit_sha: String,
    /// The flake installables the job built, for the record.
    pub installables: Vec<String>,
    /// The store paths whose closures must stay alive.
    #[serde(rename = "storePaths")]
    pub store_paths: Vec<String>,
}

impl LeaseDocument {
    /// Serialize the lease. Golden-locked: the worker writes it, the
    /// collector reads it, and offline tooling depends on the shape, so the
    /// keys emit in a fixed order with two-space indent and a trailing
    /// newline and a diff between two runs stays legible.
    pub fn serialize(&self) -> String {
        let mut body =
            serde_json::to_string_pretty(self).expect("the document fields are string-like");
        body.push('\n');
        body
    }

    /// Parse a lease, defensive about everything except the two fields the
    /// collector's safety depends on. A missing audit field degrades to
    /// `unknown`, because refusing to read a lease over incomplete provenance
    /// would make the collector fail, and a collector that cannot read a
    /// lease must abort rather than proceed: a lenient parse is what keeps a
    /// cosmetic gap from looking like a reason to delete. The project name
    /// and the renewal timestamp are not negotiable.
    ///
    /// # Errors
    ///
    /// [`ClientError::MalformedRoots`] when the body is not a JSON object,
    /// the project name is invalid, or the renewal time is missing or
    /// negative.
    pub fn parse(text: &str) -> Result<Self> {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|_| ClientError::MalformedRoots)?;
        let raw = value.as_object().ok_or(ClientError::MalformedRoots)?;

        let project = raw
            .get("project")
            .and_then(serde_json::Value::as_str)
            .and_then(|text| ProjectName::parse(text).ok())
            .ok_or(ClientError::MalformedRoots)?;

        let renewed_at_ms = raw
            .get("renewedAtMs")
            .and_then(serde_json::Value::as_u64)
            .ok_or(ClientError::MalformedRoots)?;

        Ok(Self {
            project: project.as_str().to_string(),
            renewed_at_ms,
            repository: text_field(raw, "repository"),
            ref_: text_field(raw, "ref"),
            run_id: text_field(raw, "runId"),
            commit_sha: text_field(raw, "commitSha"),
            installables: list_field(raw, "installables"),
            store_paths: list_field(raw, "storePaths"),
        })
    }

    /// Whether the lease is recent enough to protect what it names at
    /// `now`. Computed forward from the renewal with a saturating clock
    /// delta, so a clock that jumped backwards makes a lease look more
    /// current rather than expired: the conservative direction for a
    /// decision that gates deletion.
    pub fn is_active(&self, now: UnixMillis, retention_ms: u64) -> bool {
        now.saturating_ms_since(UnixMillis::new(self.renewed_at_ms)) < retention_ms
    }
}

/// A field that must be a string, defaulting when absent so an older
/// document still reads.
fn text_field(raw: &serde_json::Map<String, serde_json::Value>, name: &str) -> String {
    raw.get(name)
        .and_then(serde_json::Value::as_str)
        .unwrap_or(UNKNOWN_CLAIM)
        .to_string()
}

/// A field that must be an array of strings, defaulting to empty.
fn list_field(raw: &serde_json::Map<String, serde_json::Value>, name: &str) -> Vec<String> {
    raw.get(name)
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> String {
        serde_json::json!({
            "project": "my-org-my-repo",
            "renewedAtMs": 1_780_000_000_000_u64,
            "repository": "my-org/my-repo",
            "ref": "refs/heads/main",
            "runId": "123",
            "commitSha": "abc",
            "installables": [".#devShells.aarch64-darwin.default"],
            "storePaths": ["/nix/store/0123456789abcdfghijklmnpqrsvwxyz-bash-5.2"],
        })
        .to_string()
    }

    #[test]
    fn round_trip_preserves_every_field() {
        let document = LeaseDocument::parse(&body()).expect("a real document parses");
        let again = LeaseDocument::parse(&document.serialize()).expect("the own form parses");
        assert_eq!(document, again);
    }

    #[test]
    fn missing_provenance_defaults_to_unknown() {
        let sparse = r#"{"project":"my-org-my-repo","renewedAtMs":1780000000000}"#;
        let document = LeaseDocument::parse(sparse).expect("audit gaps degrade");
        assert_eq!(document.repository, "unknown");
        assert_eq!(document.commit_sha, "unknown");
        assert!(document.store_paths.is_empty());
    }

    #[test]
    fn project_and_renewal_are_not_negotiable() {
        for bad in [
            r"{}",
            r#"{"project":42,"renewedAtMs":1}"#,
            r#"{"project":"a..b","renewedAtMs":1}"#,
            r#"{"project":"ok","renewedAtMs":-1}"#,
            r#"{"project":"ok","renewedAtMs":"1"}"#,
            r"[]",
        ] {
            assert!(LeaseDocument::parse(bad).is_err(), "{bad} refused");
        }
    }

    #[test]
    fn backwards_clock_keeps_a_lease_active() {
        use crate::constants::LEASE_RETENTION_MS;
        let mut document = LeaseDocument::parse(&body()).expect("parses");
        document.renewed_at_ms = 1_780_000_000_000;
        // Reading the lease from one second in its past must not expire it.
        let behind = UnixMillis::new(document.renewed_at_ms - 1_000);
        assert!(document.is_active(behind, LEASE_RETENTION_MS));
        let beyond = UnixMillis::new(document.renewed_at_ms + LEASE_RETENTION_MS);
        assert!(!document.is_active(beyond, LEASE_RETENTION_MS));
    }
}
