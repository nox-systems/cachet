//! The auth decision policy (CLAUDE.md §1, §7): what a GitHub OIDC token
//! must prove to write, and how long a validated GitHub user token stays
//! believed for reads. These functions are total and clock-injected; the
//! worker's edge adapts headers and JWKS bytes into the values they
//! decide over.
//!
//! The boundaries, stated once: writes are OIDC-only; laptops are
//! read-only; an OIDC token's `repository_owner` must be one of the
//! configured orgs; lease renewal additionally requires the configured
//! default branch and a project equal to the token's `repository` claim.

use crate::constants::{
    ACCEPTED_ORGS_MAX, JWKS_CACHE_TTL_MS, OAUTH_STATE_TTL_MS, OIDC_CLOCK_TOLERANCE_MS,
    SESSION_TTL_MS, VERDICT_ALLOW_TTL_MS, VERDICT_DENY_TTL_MS,
};
use crate::error::{ClientError, Result};
use crate::types::{ProjectName, UnixMillis};

/// The OIDC issuer URL every token must carry exactly.
pub const OIDC_ISSUER: &str = "https://token.actions.githubusercontent.com";

/// The deployment configuration the policy checks against. Vars in the
/// stack supply it; production code never hardcodes identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcConfig {
    /// The GitHub orgs whose OIDC tokens may write.
    pub orgs: Vec<String>,
    /// The exact audience a token must name, one string, never a match
    /// against a list inside the token.
    pub audience: String,
    /// The default-branch ref lease renewal is pinned to.
    pub default_branch_ref: String,
}

impl OidcConfig {
    /// Validate the configuration itself: an empty org list, an org entry
    /// with whitespace, an empty audience, or more orgs than the cap is a
    /// deployment mistake, and the request path refuses it with
    /// `auth_unavailable` by construction because no token can pass the
    /// empty list. Validating here is cheaper than correcting a security
    /// boundary in production.
    pub fn validate(&self) -> Result<()> {
        if self.orgs.is_empty() || self.orgs.len() > ACCEPTED_ORGS_MAX {
            return Err(ClientError::AuthUnavailable);
        }
        if self
            .orgs
            .iter()
            .any(|org| org.is_empty() || org.bytes().any(|b| b.is_ascii_whitespace()))
        {
            return Err(ClientError::AuthUnavailable);
        }
        if self.audience.is_empty() || self.default_branch_ref.is_empty() {
            return Err(ClientError::AuthUnavailable);
        }
        Ok(())
    }
}

/// The claims a verified OIDC token proves, extracted for the rest of the
/// system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcIdentity {
    /// `owner/repo` of the workflow run.
    pub repository: String,
    /// The owner half of `repository`.
    pub repository_owner: String,
    /// The ref the run happened on.
    pub ref_: String,
    /// The workflow run id, when present.
    pub run_id: String,
    /// The commit sha, when present.
    pub sha: String,
}

/// Read one required string claim; a missing or mistyped claim refuses the
/// token, it never degrades.
fn string_claim(raw: &serde_json::Value, name: &str) -> Result<String> {
    raw.get(name)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or(ClientError::Unauthorized)
}

/// Read one required integer time claim.
fn time_claim(raw: &serde_json::Value, name: &str) -> Result<u64> {
    raw.get(name)
        .and_then(serde_json::Value::as_u64)
        .ok_or(ClientError::Unauthorized)
}

/// Check a decoded OIDC claim set against the deployment configuration.
/// Algorithm selection and signature verification happen before this runs;
/// this function decides whether a cryptographically valid token carries
/// what the deployment requires.
///
/// # Errors
///
/// [`ClientError::Unauthorized`] for issuer, audience, time, or shape
/// failures; [`ClientError::ForbiddenOrg`] for an out-of-org owner.
pub fn verify_claims(
    raw: &serde_json::Value,
    config: &OidcConfig,
    now: UnixMillis,
) -> Result<OidcIdentity> {
    config.validate()?;

    if string_claim(raw, "iss")? != OIDC_ISSUER {
        return Err(ClientError::Unauthorized);
    }
    // why: an aud that is an array is a *different* protocol obligation
    // (evaluate every entry), so it refuses rather than running partial
    // logic over one entry. Exact single-string equality is the whole
    // audience rule.
    match raw.get("aud") {
        Some(serde_json::Value::String(value)) if value == &config.audience => {}
        _ => return Err(ClientError::Unauthorized),
    }

    let now_ms = now.as_u64();
    let exp = time_claim(raw, "exp")?;
    if exp.checked_mul(1_000).ok_or(ClientError::Unauthorized)?
        < now_ms.saturating_sub(OIDC_CLOCK_TOLERANCE_MS)
    {
        return Err(ClientError::Unauthorized);
    }
    let iat = time_claim(raw, "iat")?;
    if iat.checked_mul(1_000).ok_or(ClientError::Unauthorized)?
        > now_ms.saturating_add(OIDC_CLOCK_TOLERANCE_MS)
    {
        return Err(ClientError::Unauthorized);
    }
    if let Some(nbf) = raw.get("nbf").and_then(serde_json::Value::as_u64) {
        if nbf.checked_mul(1_000).ok_or(ClientError::Unauthorized)?
            > now_ms.saturating_add(OIDC_CLOCK_TOLERANCE_MS)
        {
            return Err(ClientError::Unauthorized);
        }
    }

    let repository = string_claim(raw, "repository")?;
    let repository_owner = string_claim(raw, "repository_owner")?;
    let ref_ = string_claim(raw, "ref")?;

    // Exact membership in the configured list. No prefix matches, no
    // case folding: GitHub emits owner names in canonical form.
    if !config.orgs.iter().any(|org| org == &repository_owner) {
        return Err(ClientError::ForbiddenOrg);
    }

    Ok(OidcIdentity {
        repository,
        repository_owner,
        ref_,
        run_id: raw
            .get("run_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(crate::constants::UNKNOWN_CLAIM)
            .to_string(),
        sha: raw
            .get("sha")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(crate::constants::UNKNOWN_CLAIM)
            .to_string(),
    })
}

/// The lease-renewal gate: renewal must come from the configured default
/// branch.
///
/// # Errors
///
/// [`ClientError::ForbiddenRef`] on any other ref.
pub fn require_default_branch(identity: &OidcIdentity, config: &OidcConfig) -> Result<()> {
    if identity.ref_ == config.default_branch_ref {
        Ok(())
    } else {
        Err(ClientError::ForbiddenRef)
    }
}

/// The tenancy gate: a token may renew only its own repository's lease.
///
/// # Errors
///
/// [`ClientError::ForbiddenProject`] when the project's name does not equal
/// the hyphenated repository claim.
pub fn require_project_binding(identity: &OidcIdentity, project: &ProjectName) -> Result<()> {
    let derived = ProjectName::from_repository(&identity.repository)?;
    if &derived == project {
        Ok(())
    } else {
        Err(ClientError::ForbiddenProject)
    }
}

/// The kind of decision the JWKS cache policy produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JwksDecision {
    /// Serve the cached document; it is inside its TTL.
    UseCached,
    /// Fetch fresh; the cache is empty or expired.
    Fetch,
}

/// The per-isolate JWKS cache policy: fresh inside the TTL, fetch
/// otherwise. Stale-fallback on a failed fetch is the edge's choice, not
/// this policy's: a stale document is better than an outage, and the TTL
/// still bounds acceptance of rotated-away keys.
pub fn decide_jwks(fetched_at_ms: Option<u64>, now: UnixMillis) -> JwksDecision {
    match fetched_at_ms {
        Some(at) if now.saturating_ms_since(UnixMillis::new(at)) < JWKS_CACHE_TTL_MS => {
            JwksDecision::UseCached
        }
        _ => JwksDecision::Fetch,
    }
}

/// The unknown-kid rule: one refetch per request, never a loop.
pub fn refetch_once_allowed(already_refetched: bool) -> bool {
    !already_refetched
}

/// The verdict outcome for one GitHub user token.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Verdict {
    /// The GitHub login the token authenticates as.
    pub login: String,
    /// Whether the login is a member of an accepted org.
    pub org_member: bool,
    /// When the verdict was produced, in epoch milliseconds.
    #[serde(rename = "checkedAtMs")]
    pub checked_at_ms: u64,
}

/// How long a verdict stays believed: admits expire slower than denials,
/// because revocation of a laptop credential must converge in minutes
/// while a fresh org member must stop bouncing on the API.
pub fn verdict_ttl_ms(allowed: bool) -> u64 {
    if allowed {
        VERDICT_ALLOW_TTL_MS
    } else {
        VERDICT_DENY_TTL_MS
    }
}

/// Whether a verdict remains believable at `now`.
pub fn verdict_fresh(verdict: &Verdict, now: UnixMillis) -> bool {
    now.saturating_ms_since(UnixMillis::new(verdict.checked_at_ms))
        < verdict_ttl_ms(verdict.org_member)
}

/// The session document at `sess/{id}`: minted only after a browser OAuth
/// login proved org membership, so the record names the login and its
/// issue time. Membership re-checks happen at mint; revocation after mint
/// waits for the session's absolute expiry — the 14-day trade a 10-minute
/// verdict TTL does not grant.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionRecord {
    /// The GitHub login the session authenticates as.
    pub login: String,
    /// When the session was created, in epoch milliseconds.
    #[serde(rename = "createdAtMs")]
    pub created_at_ms: u64,
}

/// Whether a session created at `created_at_ms` is live at `now`. Absolute
/// expiry: sessions do not roll, because a quiet laptop should not extend
/// a credential horizon indefinitely.
pub fn session_live(created_at_ms: u64, now: UnixMillis) -> bool {
    now.saturating_ms_since(UnixMillis::new(created_at_ms)) < SESSION_TTL_MS
}

/// Whether an OAuth state token issued at `issued_at_ms` is valid at
/// `now`; consumed on use by the edge regardless.
pub fn oauth_state_valid(issued_at_ms: u64, now: UnixMillis) -> bool {
    now.saturating_ms_since(UnixMillis::new(issued_at_ms)) < OAUTH_STATE_TTL_MS
}

/// Whether a presented read credential is an OIDC token rather than a
/// GitHub user token: three nonempty base64url segments. The gateway is
/// shape-only — everything cryptographic happens downstream, where a
/// GitHub token that happens to look like one still gets the full
/// verification path and fails it.
pub fn looks_like_oidc_token(token: &str) -> bool {
    let segments: Vec<&str> = token.split('.').collect();
    segments.len() == 3
        && segments.iter().all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_oidc_shape_gate_is_three_base64url_segments() {
        assert!(looks_like_oidc_token("aGVhZA.cGF5bG9hZA.c2ln"));
        for bad in [
            "",
            "aGVhZA.cGF5bG9hZA",
            "aGVhZA.cGF5bG9hZA.c2ln.extra",
            "ghp_plainmetoken",
            "a..b",
            "aGVhZA.c G.c2ln",
            "aGVhZA.cGF5bG9hZA.c2ln+",
            "github_pat_11ABCD",
        ] {
            assert!(!looks_like_oidc_token(bad), "{bad} refused");
        }
    }

    fn config() -> OidcConfig {
        OidcConfig {
            orgs: vec!["my-org".to_string(), "sibling".to_string()],
            audience: "cachet".to_string(),
            default_branch_ref: "refs/heads/main".to_string(),
        }
    }

    fn claims() -> serde_json::Value {
        serde_json::json!({
            "iss": OIDC_ISSUER,
            "aud": "cachet",
            "exp": 1_780_000_600_u64,
            "iat": 1_780_000_000_u64,
            "repository": "my-org/my-repo",
            "repository_owner": "my-org",
            "ref": "refs/heads/main",
            "run_id": "42",
            "sha": "abc",
        })
    }

    #[test]
    fn the_happy_token() {
        let identity = verify_claims(&claims(), &config(), UnixMillis::new(1_780_000_000_000))
            .expect("the contract passes");
        assert_eq!(identity.repository_owner, "my-org");
        assert_eq!(identity.run_id, "42");
    }

    #[test]
    fn every_shape_failure_is_unauthorized() {
        let now = UnixMillis::new(1_780_000_000_000);
        let mutations: Vec<serde_json::Value> = vec![
            serde_json::json!({"iss": "https://example.com", "aud": "cachet", "exp": 1_780_000_600_u64, "iat": 1_u64, "repository": "my-org/my-repo", "repository_owner": "my-org", "ref": "refs/heads/main"}),
            serde_json::json!({"iss": OIDC_ISSUER, "aud": ["cachet", "other"], "exp": 1_780_000_600_u64, "iat": 1_u64, "repository": "my-org/my-repo", "repository_owner": "my-org", "ref": "refs/heads/main"}),
            serde_json::json!({"iss": OIDC_ISSUER, "aud": "other", "exp": 1_780_000_600_u64, "iat": 1_u64, "repository": "my-org/my-repo", "repository_owner": "my-org", "ref": "refs/heads/main"}),
            serde_json::json!({"iss": OIDC_ISSUER, "aud": "cachet", "exp": 1_776_000_000_u64, "iat": 1_u64, "repository": "my-org/my-repo", "repository_owner": "my-org", "ref": "refs/heads/main"}),
            serde_json::json!({"iss": OIDC_ISSUER, "aud": "cachet", "exp": "1780000600", "iat": 1_u64, "repository": "my-org/my-repo", "repository_owner": "my-org", "ref": "refs/heads/main"}),
        ];
        for raw in mutations {
            assert_eq!(
                verify_claims(&raw, &config(), now),
                Err(ClientError::Unauthorized),
                "{raw} refused"
            );
        }
    }

    #[test]
    fn out_of_org_is_forbidden() {
        let mut raw = claims();
        raw["repository_owner"] = serde_json::Value::String("elsewhere".into());
        assert_eq!(
            verify_claims(&raw, &config(), UnixMillis::new(1_780_000_000_000)),
            Err(ClientError::ForbiddenOrg)
        );
        // why: suffix similarity is not membership.
        raw["repository_owner"] = serde_json::Value::String("my-org-extended".into());
        assert!(matches!(
            verify_claims(&raw, &config(), UnixMillis::new(1_780_000_000_000)),
            Err(ClientError::ForbiddenOrg | ClientError::Unauthorized)
        ));
    }

    #[test]
    fn project_binding_refuses_cross_repo_leases() {
        let identity =
            verify_claims(&claims(), &config(), UnixMillis::new(1_780_000_000_000)).expect("valid");
        assert!(
            require_project_binding(
                &identity,
                &ProjectName::parse("my-org-my-repo").expect("valid name")
            )
            .is_ok()
        );
        assert_eq!(
            require_project_binding(
                &identity,
                &ProjectName::parse("my-org-other-repo").expect("valid name")
            ),
            Err(ClientError::ForbiddenProject)
        );
    }

    #[test]
    fn branch_gate_uses_exact_match() {
        let identity =
            verify_claims(&claims(), &config(), UnixMillis::new(1_780_000_000_000)).expect("valid");
        assert!(require_default_branch(&identity, &config()).is_ok());
        let mut misbranch = identity.clone();
        misbranch.ref_ = "refs/heads/feature".to_string();
        assert_eq!(
            require_default_branch(&misbranch, &config()),
            Err(ClientError::ForbiddenRef)
        );
    }

    #[test]
    fn verdict_and_session_clocks() {
        let verdict = Verdict {
            login: "someone".to_string(),
            org_member: true,
            checked_at_ms: 1_000,
        };
        assert!(verdict_fresh(
            &verdict,
            UnixMillis::new(1_000 + VERDICT_ALLOW_TTL_MS - 1)
        ));
        assert!(!verdict_fresh(
            &verdict,
            UnixMillis::new(1_000 + VERDICT_ALLOW_TTL_MS)
        ));
        let denied = Verdict {
            org_member: false,
            ..verdict
        };
        assert!(!verdict_fresh(
            &denied,
            UnixMillis::new(1_000 + VERDICT_DENY_TTL_MS)
        ));
        assert!(session_live(0, UnixMillis::new(SESSION_TTL_MS - 1)));
        assert!(!session_live(0, UnixMillis::new(SESSION_TTL_MS)));
        assert!(oauth_state_valid(
            0,
            UnixMillis::new(OAUTH_STATE_TTL_MS - 1)
        ));
    }

    #[test]
    fn jwks_policy_is_ttl_bound() {
        assert_eq!(
            decide_jwks(
                Some(10_000),
                UnixMillis::new(10_000 + JWKS_CACHE_TTL_MS - 1)
            ),
            JwksDecision::UseCached
        );
        assert_eq!(
            decide_jwks(Some(10_000), UnixMillis::new(10_000 + JWKS_CACHE_TTL_MS)),
            JwksDecision::Fetch
        );
        assert_eq!(decide_jwks(None, UnixMillis::new(0)), JwksDecision::Fetch);
        assert!(refetch_once_allowed(false));
        assert!(!refetch_once_allowed(true));
    }
}
