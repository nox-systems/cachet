//! The read credential a deployment issues to a laptop or a workflow
//! run.
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
//!
//! A workflow run had the same problem on a shorter clock. The action
//! wrote the job's GitHub OIDC token into the daemon's netrc, that token
//! expires within minutes, and the job lost the cache partway through
//! and built the rest from source. A run now trades its OIDC token for
//! one of these credentials as soon as it has it. The exchange checks
//! the token exactly as a CI read checks it, and the record keeps the
//! claims it proved (repository, ref, run id and attempt, commit) and
//! nothing that could be presented anywhere. Nothing re-checks a run
//! afterwards, so the record's six-hour lifetime, the longest GitHub
//! lets a job run, is the credential's whole life.

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
#[serde(try_from = "RecordWire", into = "RecordWire")]
pub struct ReadTokenRecord {
    /// The GitHub login this token speaks for: a person's, or for a
    /// workflow run the repository owner, which is the login every CI
    /// read already resolves to.
    pub login: String,
    /// When the deployment issued it, epoch milliseconds.
    pub issued_at_ms: u64,
    /// When it stops being accepted, epoch milliseconds. For a laptop
    /// this is the outer bound on the credential's life, not the
    /// revocation window: membership is re-checked against GitHub on the
    /// verdict cache's own schedule, so leaving the organisation ends
    /// access long before this. For a run it is the whole life.
    pub expires_at_ms: u64,
    /// Who the token was issued to, and what backs it.
    pub holder: ReadTokenHolder,
}

/// The two kinds of caller a deployment issues read credentials to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadTokenHolder {
    /// A person's machine, which proved a GitHub identity through the
    /// device flow. The record keeps the GitHub credentials so that
    /// membership stays checkable.
    Laptop(GitHubGrant),
    /// A workflow run, which proved itself with a GitHub OIDC token. The
    /// record keeps the claims that token carried, for audit, and
    /// nothing that could be presented anywhere.
    Run(RunBinding),
}

/// The GitHub credentials a laptop's token stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubGrant {
    /// The GitHub access token. Held here so membership stays checkable,
    /// and held only here: it never rides a request and never reaches
    /// the laptop's netrc.
    pub token: String,
    /// The refresh token GitHub issues alongside it, when the OAuth App
    /// uses expiring tokens. Empty when it does not, which is a
    /// deployment whose access tokens never expire and so never need
    /// renewing.
    pub refresh_token: String,
    /// When the access token above stops working, epoch milliseconds.
    /// Zero means GitHub did not say, which it does not for
    /// non-expiring tokens.
    pub expires_at_ms: u64,
}

impl GitHubGrant {
    /// Whether the access token needs renewing before it is used again.
    ///
    /// A token with no stated expiry never needs it: that is an OAuth
    /// App which has not opted into short-lived tokens, and its token
    /// works until someone revokes it. One with an expiry is renewed
    /// early, because a token that expires between this check and the
    /// call it is about to make would read as a membership failure.
    #[must_use]
    pub fn stale(&self, now: crate::types::UnixMillis) -> bool {
        self.expires_at_ms != 0
            && now
                .as_u64()
                .saturating_add(crate::constants::GITHUB_RENEW_SKEW_MS)
                >= self.expires_at_ms
    }

    /// Whether this grant can renew its own access token.
    #[must_use]
    pub fn can_renew(&self) -> bool {
        !self.refresh_token.is_empty()
    }
}

/// The claims a run's OIDC token proved at the exchange, kept so an
/// operator reading the record, or the event the exchange logged, can
/// tell which job of which workflow run holds the credential. They also
/// name the job, which is what the bound on credentials and revocation
/// apply to ([`RunBinding::job_name`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RunBinding {
    /// `owner/repo`, folded.
    pub repository: String,
    /// The ref the run happened on.
    #[serde(rename = "ref")]
    pub reference: String,
    /// The workflow run id.
    #[serde(rename = "runId")]
    pub run_id: String,
    /// Which attempt of that run. A re-run keeps its run id, so the pair
    /// names one execution of the whole workflow.
    #[serde(rename = "runAttempt")]
    pub run_attempt: String,
    /// The check run of the job, which tells it from the run's other jobs.
    #[serde(rename = "checkRunId")]
    pub check_run_id: String,
    /// The commit the run built.
    pub sha: String,
}

impl ReadTokenRecord {
    /// The record a workflow run's exchange stores, from the identity its
    /// OIDC token proved.
    ///
    /// The lifetime starts at the exchange and runs for
    /// [`RUN_READ_TOKEN_TTL_MS`](crate::constants::RUN_READ_TOKEN_TTL_MS),
    /// which is the longest a job may run. It is unrelated to the OIDC
    /// token's own expiry, which is the point: that token lapses within
    /// minutes, and the daemon goes on presenting this credential for
    /// the rest of the job.
    ///
    /// # Errors
    ///
    /// [`ClientError::Unauthorized`](crate::error::ClientError::Unauthorized)
    /// for a token without a run id, run attempt, or check run id. The
    /// credentials one job may hold are counted and revoked by those
    /// three, so a token missing one would share its count and its
    /// revocation with every other such token.
    pub fn for_run(
        identity: &crate::auth::OidcIdentity,
        now: crate::types::UnixMillis,
    ) -> crate::error::Result<Self> {
        let unknown = crate::constants::UNKNOWN_CLAIM;
        if [
            &identity.run_id,
            &identity.run_attempt,
            &identity.check_run_id,
        ]
        .iter()
        .any(|claim| claim.as_str() == unknown)
        {
            return Err(crate::error::ClientError::Unauthorized);
        }
        Ok(Self {
            login: crate::auth::fold_identifier(&identity.repository_owner),
            issued_at_ms: now.as_u64(),
            expires_at_ms: now
                .as_u64()
                .saturating_add(crate::constants::RUN_READ_TOKEN_TTL_MS),
            holder: ReadTokenHolder::Run(RunBinding {
                repository: identity.repository.clone(),
                reference: identity.ref_.clone(),
                run_id: identity.run_id.clone(),
                run_attempt: identity.run_attempt.clone(),
                check_run_id: identity.check_run_id.clone(),
                sha: identity.sha.clone(),
            }),
        })
    }

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
    /// [`serde_json::Error`] on invalid JSON, a schema mismatch, or a
    /// record that names both kinds of holder or neither.
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

    /// The run a run's record is bound to; `None` for a laptop's.
    #[must_use]
    pub const fn run(&self) -> Option<&RunBinding> {
        match &self.holder {
            ReadTokenHolder::Run(run) => Some(run),
            ReadTokenHolder::Laptop(_) => None,
        }
    }

    /// What the record says when it is issued or used, for the event log.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self.holder {
            ReadTokenHolder::Laptop(_) => "laptop",
            ReadTokenHolder::Run(_) => "run",
        }
    }
}

/// The stored form of a record: one flat object.
///
/// why flat: a laptop's record keeps exactly the bytes it was always
/// written with, so records issued before runs could exchange still
/// parse, and a run's record carries a `run` object and no `githubToken`.
/// That absence matters in a rollback. The worker before this change
/// requires `githubToken`, so it refuses a run's record as corrupt, and
/// a run's credential can never be mistaken there for a laptop's.
#[derive(serde::Serialize, serde::Deserialize)]
struct RecordWire {
    login: String,
    #[serde(rename = "issuedAtMs")]
    issued_at_ms: u64,
    #[serde(rename = "expiresAtMs")]
    expires_at_ms: u64,
    #[serde(
        rename = "githubToken",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    github_token: Option<String>,
    #[serde(
        rename = "githubRefreshToken",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    github_refresh_token: Option<String>,
    #[serde(
        rename = "githubExpiresAtMs",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    github_expires_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run: Option<RunBinding>,
}

impl TryFrom<RecordWire> for ReadTokenRecord {
    type Error = &'static str;

    fn try_from(wire: RecordWire) -> std::result::Result<Self, Self::Error> {
        let holder = match (wire.github_token, wire.run) {
            (Some(token), None) => ReadTokenHolder::Laptop(GitHubGrant {
                token,
                refresh_token: wire.github_refresh_token.unwrap_or_default(),
                expires_at_ms: wire.github_expires_at_ms.unwrap_or(0),
            }),
            // why: a run record carrying any GitHub field is not one the
            // worker wrote, and guessing which half to believe would be
            // deciding a credential's kind from damaged state.
            (None, Some(run))
                if wire.github_refresh_token.is_none() && wire.github_expires_at_ms.is_none() =>
            {
                ReadTokenHolder::Run(run)
            }
            _ => return Err("a read-token record names exactly one holder"),
        };
        Ok(Self {
            login: wire.login,
            issued_at_ms: wire.issued_at_ms,
            expires_at_ms: wire.expires_at_ms,
            holder,
        })
    }
}

impl From<ReadTokenRecord> for RecordWire {
    fn from(record: ReadTokenRecord) -> Self {
        let (github_token, github_refresh_token, github_expires_at_ms, run) = match record.holder {
            ReadTokenHolder::Laptop(grant) => (
                Some(grant.token),
                Some(grant.refresh_token),
                Some(grant.expires_at_ms),
                None,
            ),
            ReadTokenHolder::Run(run) => (None, None, None, Some(run)),
        };
        Self {
            login: record.login,
            issued_at_ms: record.issued_at_ms,
            expires_at_ms: record.expires_at_ms,
            github_token,
            github_refresh_token,
            github_expires_at_ms,
            run,
        }
    }
}

impl RunBinding {
    /// The bytes that name this job, which the worker hashes into the key
    /// its credentials are counted and revoked under. Newlines separate
    /// the parts because none of them can contain one.
    ///
    /// why the check run: every job of a workflow run shares the run id
    /// and attempt, so a name without it made one bound and one revocation
    /// cover the whole run. A run with eighteen jobs refused the fifth
    /// job onwards, and the first job to finish revoked everyone else's
    /// credential mid-build.
    #[must_use]
    pub fn job_name(&self) -> String {
        format!(
            "{}\n{}\n{}\n{}",
            self.repository, self.run_id, self.run_attempt, self.check_run_id
        )
    }
}

/// The credentials one job was issued, counted so a job cannot mint them
/// without bound.
///
/// The list bounds the exchange and nothing else: a read never consults
/// it, and revocation lives under its own key, which no exchange writes
/// ([`run_revoked_key`]). An exchange reads this list and writes it whole,
/// so two exchanges for one job that race can lose an entry and admit
/// one credential more than the bound. That is the whole cost of the
/// race, because nothing else depends on what the list holds.
///
/// why one list rather than one KV key per credential: listing KV keys
/// lags writes by up to a minute, so a count built on a listing could
/// miss credentials minted moments earlier.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RunCredentials {
    /// The digests of the job's credentials, in issue order.
    pub digests: Vec<String>,
}

impl RunCredentials {
    /// Add a newly issued credential.
    ///
    /// # Errors
    ///
    /// [`ClientError::CredentialLimit`](crate::error::ClientError::CredentialLimit)
    /// when the job already holds
    /// [`RUN_READ_TOKENS_MAX`](crate::constants::RUN_READ_TOKENS_MAX).
    pub fn admit(&mut self, digest_hex: &str) -> crate::error::Result<()> {
        if self.digests.len() >= crate::constants::RUN_READ_TOKENS_MAX {
            return Err(crate::error::ClientError::CredentialLimit);
        }
        self.digests.push(digest_hex.to_string());
        Ok(())
    }

    /// Serialize with a trailing newline.
    #[must_use]
    pub fn serialize(&self) -> String {
        let mut body = serde_json::to_string(self).expect("string fields");
        body.push('\n');
        body
    }

    /// Parse a stored list. The worker wrote it, so a parse failure is a
    /// storage fault.
    ///
    /// # Errors
    ///
    /// [`serde_json::Error`] on invalid JSON or a schema mismatch.
    pub fn parse(text: &str) -> std::result::Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }
}

/// Where one job's [`RunCredentials`] live in KV, from the digest of its
/// [`RunBinding::job_name`].
#[must_use]
pub fn run_tokens_key(job_digest_hex: &str) -> String {
    format!(
        "{}{job_digest_hex}",
        crate::constants::RUN_TOKENS_KEY_PREFIX
    )
}

/// Where one job's revocation is marked in KV, from the digest of its
/// [`RunBinding::job_name`]. Only revocation writes this key. A read of
/// any of the job's credentials refuses while it exists, and so does an
/// exchange for the job, so an exchange that read a stale list can never
/// bring a revoked job back.
#[must_use]
pub fn run_revoked_key(job_digest_hex: &str) -> String {
    format!(
        "{}{job_digest_hex}",
        crate::constants::RUN_REVOKED_KEY_PREFIX
    )
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

    fn laptop(issued_at_ms: u64, expires_at_ms: u64) -> ReadTokenRecord {
        ReadTokenRecord {
            login: "octocat".to_string(),
            issued_at_ms,
            expires_at_ms,
            holder: ReadTokenHolder::Laptop(GitHubGrant {
                token: "gho_live".to_string(),
                refresh_token: "ghr_live".to_string(),
                expires_at_ms: 1_780_000_300_000,
            }),
        }
    }

    fn run_claims(iat_seconds: u64) -> serde_json::Value {
        // The shape GitHub's OIDC tokens carry, with the five-minute life
        // they have: `exp` is `iat` plus 300 seconds.
        serde_json::json!({
            "iss": crate::auth::OIDC_ISSUER,
            "aud": "cachet",
            "iat": iat_seconds,
            "exp": iat_seconds + 300,
            "repository": "My-Org/my-repo",
            "repository_owner": "My-Org",
            "ref": "refs/heads/main",
            "run_id": "38468685026",
            "run_attempt": "2",
            "check_run_id": "115735782585",
            "sha": "abc",
        })
    }

    fn run_config() -> crate::auth::OidcConfig {
        crate::auth::OidcConfig {
            orgs: vec!["my-org".to_string()],
            audience: "cachet".to_string(),
            default_branch_ref: "refs/heads/main".to_string(),
        }
    }

    #[test]
    fn a_laptop_record_round_trips() {
        let record = laptop(1_780_000_000_000, 1_780_000_600_000);
        assert_eq!(
            ReadTokenRecord::parse(&record.serialize()).expect("its own form"),
            record
        );
    }

    #[test]
    fn a_laptop_record_keeps_the_bytes_it_was_always_written_with() {
        // Records issued before runs could exchange sit in KV in this
        // form for up to thirty days. They must keep parsing, and a
        // laptop record written now must be indistinguishable from them.
        let stored = concat!(
            r#"{"login":"octocat","issuedAtMs":1780000000000,"expiresAtMs":1780000600000,"#,
            r#""githubToken":"gho_live","githubRefreshToken":"ghr_live","githubExpiresAtMs":1780000300000}"#,
            "\n"
        );
        let parsed = ReadTokenRecord::parse(stored).expect("the old form parses");
        assert_eq!(parsed, laptop(1_780_000_000_000, 1_780_000_600_000));
        assert_eq!(parsed.serialize(), stored);
        // The two optional fields were optional before too.
        let bare = r#"{"login":"octocat","issuedAtMs":1,"expiresAtMs":2,"githubToken":"gho_live"}"#;
        let ReadTokenHolder::Laptop(grant) = ReadTokenRecord::parse(bare).expect("parses").holder
        else {
            panic!("a record with a GitHub token is a laptop's");
        };
        assert!(!grant.can_renew());
        assert!(
            !grant.stale(UnixMillis::new(u64::MAX)),
            "no expiry, never stale"
        );
    }

    #[test]
    fn a_run_record_carries_its_claims_and_no_github_credential() {
        let now = UnixMillis::new(1_780_000_000_000);
        let identity = crate::auth::verify_claims(&run_claims(1_780_000_000), &run_config(), now)
            .expect("a valid run token");
        let record = ReadTokenRecord::for_run(&identity, now).expect("a run's token");
        assert_eq!(record.login, "my-org", "the repository owner, folded");
        assert_eq!(record.kind(), "run");
        assert_eq!(
            record.holder,
            ReadTokenHolder::Run(RunBinding {
                repository: "my-org/my-repo".to_string(),
                reference: "refs/heads/main".to_string(),
                run_id: "38468685026".to_string(),
                run_attempt: "2".to_string(),
                check_run_id: "115735782585".to_string(),
                sha: "abc".to_string(),
            })
        );
        let text = record.serialize();
        assert!(!text.contains("github"), "{text}");
        assert!(text.contains(r#""runAttempt":"2""#), "{text}");
        assert_eq!(ReadTokenRecord::parse(&text).expect("its own form"), record);
    }

    #[test]
    fn a_run_credential_outlives_the_oidc_token_it_was_traded_for() {
        // The bug this record exists for: on October 13, 2026 a job wrote
        // its OIDC token into the daemon's netrc, the token expired about
        // seven minutes in, and every read after that answered 401 until
        // the runner's disk filled with source builds. The credential the
        // exchange issues has to keep reading after the token behind it
        // is refused.
        let issued = UnixMillis::new(1_780_000_000_000);
        let claims = run_claims(1_780_000_000);
        let identity = crate::auth::verify_claims(&claims, &run_config(), issued)
            .expect("the token verifies at the exchange");
        let record = ReadTokenRecord::for_run(&identity, issued).expect("a run's token");

        let an_hour_in = UnixMillis::new(issued.as_u64() + 3_600_000);
        assert_eq!(
            crate::auth::verify_claims(&claims, &run_config(), an_hour_in),
            Err(crate::error::ClientError::Unauthorized),
            "the OIDC token itself is long dead"
        );
        assert!(record.is_live(an_hour_in), "the issued credential is not");

        // And it lasts as long as GitHub lets a job run, and no longer.
        let ttl = crate::constants::RUN_READ_TOKEN_TTL_MS;
        assert_eq!(ttl, 6 * 3_600_000);
        assert!(record.is_live(UnixMillis::new(issued.as_u64() + ttl - 1)));
        assert!(!record.is_live(UnixMillis::new(issued.as_u64() + ttl)));
    }

    #[test]
    fn a_record_names_exactly_one_holder() {
        for (text, why) in [
            (
                r#"{"login":"o","issuedAtMs":1,"expiresAtMs":2}"#,
                "neither: the shape a withdrawn version once wrote",
            ),
            (
                r#"{"login":"o","issuedAtMs":1,"expiresAtMs":2,"githubToken":"gho_live","run":{"repository":"o/r","ref":"refs/heads/main","runId":"1","runAttempt":"1","checkRunId":"1","sha":"abc"}}"#,
                "both",
            ),
            (
                r#"{"login":"o","issuedAtMs":1,"expiresAtMs":2,"githubRefreshToken":"ghr_live","run":{"repository":"o/r","ref":"refs/heads/main","runId":"1","runAttempt":"1","checkRunId":"1","sha":"abc"}}"#,
                "a run with half a GitHub grant",
            ),
        ] {
            assert!(ReadTokenRecord::parse(text).is_err(), "{why} refused");
        }
    }

    #[test]
    fn liveness_is_checked_here_and_not_left_to_kv() {
        let record = laptop(1_000, 2_000);
        assert!(record.is_live(UnixMillis::new(1_999)));
        assert!(
            !record.is_live(UnixMillis::new(2_000)),
            "expiry is exclusive"
        );
        assert!(!record.is_live(UnixMillis::new(9_999)));
    }

    #[test]
    fn a_grant_renews_early_and_only_when_it_can() {
        let grant = GitHubGrant {
            token: "gho_live".to_string(),
            refresh_token: "ghr_live".to_string(),
            expires_at_ms: 1_000_000,
        };
        let skew = crate::constants::GITHUB_RENEW_SKEW_MS;
        assert!(!grant.stale(UnixMillis::new(1_000_000 - skew - 1)));
        assert!(grant.stale(UnixMillis::new(1_000_000 - skew)));
        assert!(grant.can_renew());
    }

    #[test]
    fn a_token_without_its_run_is_not_issued_a_credential() {
        let now = UnixMillis::new(1_780_000_000_000);
        for missing in ["run_id", "run_attempt", "check_run_id"] {
            let mut claims = run_claims(1_780_000_000);
            claims.as_object_mut().expect("an object").remove(missing);
            let identity = crate::auth::verify_claims(&claims, &run_config(), now)
                .expect("reads accept it, as they always have");
            assert_eq!(
                ReadTokenRecord::for_run(&identity, now),
                Err(crate::error::ClientError::Unauthorized),
                "{missing} absent"
            );
        }
    }

    #[test]
    fn a_job_is_issued_four_credentials() {
        let mut held = RunCredentials::default();
        for digest in ["a1", "b2", "c3", "d4"] {
            held.admit(digest).expect("under the bound");
        }
        assert_eq!(
            held.admit("e5"),
            Err(crate::error::ClientError::CredentialLimit),
            "the fifth is refused"
        );
        assert_eq!(held.digests.len(), 4, "a refusal adds nothing");
        assert_eq!(
            RunCredentials::parse(&held.serialize()).expect("its own form"),
            held
        );
        assert_eq!(
            held.serialize(),
            "{\"digests\":[\"a1\",\"b2\",\"c3\",\"d4\"]}\n"
        );
    }

    fn binding(check_run_id: &str) -> RunBinding {
        RunBinding {
            repository: "my-org/my-repo".to_string(),
            reference: "refs/heads/main".to_string(),
            run_id: "38468685026".to_string(),
            run_attempt: "2".to_string(),
            check_run_id: check_run_id.to_string(),
            sha: "abc".to_string(),
        }
    }

    #[test]
    fn a_job_is_named_by_its_run_its_attempt_and_its_check_run() {
        assert_eq!(
            binding("115735782585").job_name(),
            "my-org/my-repo\n38468685026\n2\n115735782585"
        );
        // Two jobs of one run attempt are two names: one job's bound and
        // revocation must never reach another's.
        assert_ne!(binding("1").job_name(), binding("2").job_name());
        // The ref and commit do not split a job: they are fixed for it.
        let other_ref = RunBinding {
            reference: "refs/heads/other".to_string(),
            ..binding("1")
        };
        assert_eq!(other_ref.job_name(), binding("1").job_name());
        assert_eq!(run_tokens_key("abc"), "runtokens/abc");
        assert_eq!(run_revoked_key("abc"), "runrevoked/abc");
    }

    #[test]
    fn the_key_namespaces_the_digest() {
        assert_eq!(read_token_key("abc123"), "readtoken/abc123");
    }
}
