//! The read credential a deployment issues to a laptop.
//!
//! A laptop used to put its GitHub token in the daemon's netrc, so every
//! substitution sent a live GitHub credential to the cache. Two things
//! were wrong with that. GitHub's own tokens now expire after eight
//! hours by default, and nothing is positioned to refresh the copy the
//! daemon reads: nix reads that file directly, so the credential simply
//! goes stale and every build quietly recompiles from source. And the
//! cache ended up holding replayable `read:org` tokens for every person
//! who ever logged in, which is a store of other people's credentials it
//! has no reason to keep.
//!
//! So the deployment issues its own credential for the daemon to carry,
//! and keeps the GitHub tokens itself. The issued token is opaque and
//! names nothing; it is a pointer to a record holding the GitHub
//! credentials that back it. Two things follow, and they are the whole
//! reason for the shape.
//!
//! Nothing replayable crosses the wire. What the daemon sends a thousand
//! times a build authenticates against this deployment and nowhere else.
//!
//! And membership stays checkable. The record holds the GitHub token, so
//! every read still resolves through the verdict cache against GitHub as
//! the source of truth, and someone who leaves the organisation loses
//! access within one verdict TTL rather than whenever their credential
//! happens to expire. The record also holds the refresh token, so when
//! GitHub's eight-hour access token dies the worker mints another one
//! itself. Nobody logs in again for that.

use crate::constants::{READ_TOKEN_BODY_LENGTH, READ_TOKEN_PREFIX};

/// Whether a credential is one this deployment issued.
///
/// The prefix is what makes the read path able to tell three credential
/// shapes apart without trying each in turn: an OIDC token is three
/// base64url segments, one of these is the prefix and a fixed-length
/// body, and anything else is a GitHub token to be checked upstream.
#[must_use]
pub fn looks_like_read_token(token: &str) -> bool {
    let Some(body) = token.strip_prefix(READ_TOKEN_PREFIX) else {
        return false;
    };
    body.len() == READ_TOKEN_BODY_LENGTH
        && body
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// Render the token a mint hands back, from the random body it drew.
#[must_use]
pub fn format_read_token(body: &str) -> String {
    format!("{READ_TOKEN_PREFIX}{body}")
}

/// What the deployment remembers about one issued token.
///
/// The token itself is never stored: the record is keyed by the token's
/// SHA-256, so a reader of the deployment's KV cannot present what they
/// find there. Only the holder can.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReadTokenRecord {
    /// The GitHub login this token speaks for.
    pub login: String,
    /// When the deployment issued it, epoch milliseconds.
    #[serde(rename = "issuedAtMs")]
    pub issued_at_ms: u64,
    /// When it stops being accepted, epoch milliseconds. This is the
    /// outer bound on the credential's life, not the revocation window:
    /// membership is re-checked against GitHub on the verdict cache's
    /// own schedule, so leaving the organisation ends access long before
    /// this.
    #[serde(rename = "expiresAtMs")]
    pub expires_at_ms: u64,
    /// The GitHub access token this credential stands for. Held here so
    /// membership stays checkable, and held only here: it never rides a
    /// request and never reaches the laptop's netrc.
    #[serde(rename = "githubToken")]
    pub github_token: String,
    /// The refresh token GitHub issues alongside it, when the OAuth App
    /// uses expiring tokens. Empty when it does not, which is a
    /// deployment whose access tokens never expire and so never need
    /// renewing.
    #[serde(rename = "githubRefreshToken", default)]
    pub github_refresh_token: String,
    /// When the access token above stops working, epoch milliseconds.
    /// Zero means GitHub did not say, which it does not for
    /// non-expiring tokens.
    #[serde(rename = "githubExpiresAtMs", default)]
    pub github_expires_at_ms: u64,
}

impl ReadTokenRecord {
    /// Serialize with a trailing newline.
    #[must_use]
    pub fn serialize(&self) -> String {
        let mut body = serde_json::to_string(self).expect("string and numeric fields");
        body.push('\n');
        body
    }

    /// Parse a stored record. The worker wrote it, so a parse failure is
    /// a storage fault rather than anything a client did.
    ///
    /// # Errors
    ///
    /// [`serde_json::Error`] on invalid JSON or a schema mismatch.
    pub fn parse(text: &str) -> std::result::Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Whether the record still speaks for its holder at this instant.
    ///
    /// Checked on top of the KV entry's own expiry rather than instead of
    /// it. KV expiry is eventual, and a credential's lifetime is not a
    /// thing to leave to eventual.
    #[must_use]
    pub fn is_live(&self, now: crate::types::UnixMillis) -> bool {
        now.as_u64() < self.expires_at_ms
    }

    /// Whether the GitHub token behind this record needs renewing before
    /// it is used again.
    ///
    /// A token with no stated expiry never needs it: that is an OAuth
    /// App which has not opted into short-lived tokens, and its token
    /// works until someone revokes it. One with an expiry is renewed
    /// early, because a token that expires between this check and the
    /// call it is about to make would read as a membership failure.
    #[must_use]
    pub fn github_token_stale(&self, now: crate::types::UnixMillis) -> bool {
        self.github_expires_at_ms != 0
            && now
                .as_u64()
                .saturating_add(crate::constants::GITHUB_RENEW_SKEW_MS)
                >= self.github_expires_at_ms
    }

    /// Whether this record can renew its own GitHub token.
    #[must_use]
    pub fn can_renew(&self) -> bool {
        !self.github_refresh_token.is_empty()
    }
}

/// Where one issued token's record lives in KV.
#[must_use]
pub fn read_token_key(token_digest_hex: &str) -> String {
    format!(
        "{}{token_digest_hex}",
        crate::constants::READ_TOKEN_KEY_PREFIX
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::UnixMillis;

    fn body() -> String {
        "a".repeat(READ_TOKEN_BODY_LENGTH)
    }

    #[test]
    fn the_prefix_and_length_together_identify_the_shape() {
        assert!(looks_like_read_token(&format_read_token(&body())));
        // A GitHub token is not one of ours.
        assert!(!looks_like_read_token(
            "gho_16C7e42F292c6912E7710c838347Ae178B4a"
        ));
        // Neither is an OIDC token.
        assert!(!looks_like_read_token("aaa.bbb.ccc"));
        // The prefix alone is not enough: length is part of the grammar.
        assert!(!looks_like_read_token(&format_read_token("short")));
        // Nor is a body outside base64url.
        assert!(!looks_like_read_token(&format_read_token(
            &"!".repeat(READ_TOKEN_BODY_LENGTH)
        )));
    }

    #[test]
    fn a_record_round_trips() {
        let record = ReadTokenRecord {
            login: "octocat".to_string(),
            issued_at_ms: 1_780_000_000_000,
            expires_at_ms: 1_780_000_600_000,
            github_token: "gho_live".to_string(),
            github_refresh_token: "ghr_live".to_string(),
            github_expires_at_ms: 1_780_000_300_000,
        };
        assert_eq!(
            ReadTokenRecord::parse(&record.serialize()).expect("its own form"),
            record
        );
    }

    #[test]
    fn liveness_is_checked_here_and_not_left_to_kv() {
        let record = ReadTokenRecord {
            login: "octocat".to_string(),
            issued_at_ms: 1_000,
            expires_at_ms: 2_000,
            github_token: "gho_live".to_string(),
            github_refresh_token: String::new(),
            github_expires_at_ms: 0,
        };
        assert!(record.is_live(UnixMillis::new(1_999)));
        assert!(
            !record.is_live(UnixMillis::new(2_000)),
            "expiry is exclusive"
        );
        assert!(!record.is_live(UnixMillis::new(9_999)));
    }

    #[test]
    fn the_key_namespaces_the_digest() {
        assert_eq!(read_token_key("abc123"), "readtoken/abc123");
    }
}
