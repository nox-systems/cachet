//! GitHub OIDC at the edge (CLAUDE.md §1): the only write credential. The
//! decision policy is cachet-core's; this module does the isolate-local
//! JWKS caching, the single refetch on an unknown kid, the stale fallback,
//! and the environment plumbing. The JWKS document itself is not trusted
//! state: it is transport for public keys, refetched from the issuer's URL
//! (a deployment variable, so the workerd lane can substitute a stub).

use std::cell::RefCell;

use cachet_core::auth::{
    OidcConfig, OidcIdentity, decide_jwks, refetch_once_allowed, verify_claims,
};
use cachet_core::error::ClientError;
use cachet_core::types::UnixMillis;
use cachet_crypto::rs256::{RsaJwk, decode_jwt, verify_rs256};
use worker::{Env, Fetch, Method, Request};

use crate::log;

/// The JWKS document's shape.
#[derive(Debug, serde::Deserialize)]
struct JwksDocument {
    keys: Vec<RsaJwk>,
}

// why: workerd runs one worker per isolate on one thread, so a
// thread_local cache is exactly the per-isolate cache the design calls
// for (10-minute TTL, one refetch on unknown kid, stale fallback).
thread_local! {
    static JWKS_CACHE: RefCell<Option<(u64, Vec<RsaJwk>)>> = const { RefCell::new(None) };
}

/// The deployment's OIDC configuration, read from its variables. Missing
/// configuration cannot verify anything, so it answers auth_unavailable
/// rather than fail open.
pub(crate) fn oidc_config(env: &Env) -> cachet_core::error::Result<OidcConfig> {
    let orgs = env
        .var("CACHET_ORGS")
        .map_err(|_| ClientError::AuthUnavailable)?
        .to_string();
    let audience = env
        .var("CACHET_AUDIENCE")
        .map_err(|_| ClientError::AuthUnavailable)?
        .to_string();
    let default_branch_ref = env
        .var("CACHET_DEFAULT_BRANCH_REF")
        .map_err(|_| ClientError::AuthUnavailable)?
        .to_string();
    let config = OidcConfig {
        orgs: orgs
            .split(',')
            .map(|org| org.trim().to_string())
            .filter(|org| !org.is_empty())
            .collect(),
        audience,
        default_branch_ref,
    };
    config.validate()?;
    Ok(config)
}

/// Fetch the issuer's JWKS document now.
async fn fetch_jwks(env: &Env) -> cachet_core::error::Result<Vec<RsaJwk>> {
    let url = env
        .var("CACHET_JWKS_URL")
        .map_err(|_| ClientError::AuthUnavailable)?
        .to_string();
    let request = Request::new(&url, Method::Get).map_err(|_| ClientError::AuthUnavailable)?;
    let mut response = Fetch::Request(request)
        .send()
        .await
        .map_err(|_| ClientError::AuthUnavailable)?;
    if response.status_code() != 200 {
        return Err(ClientError::AuthUnavailable);
    }
    let document: JwksDocument = response
        .json()
        .await
        .map_err(|_| ClientError::AuthUnavailable)?;
    Ok(document.keys)
}

/// Resolve the signing key for `kid`, maintaining the isolate's cache
/// policy. Stale fallback is deliberate: a cached document older than its
/// TTL is better than an outage ("auth_unavailable" is only for having no
/// trustworthy document at all), and the TTL still bounds how long a
/// rotated-away key keeps working.
async fn jwks_key_for(
    env: &Env,
    now: UnixMillis,
    kid: Option<&str>,
) -> cachet_core::error::Result<RsaJwk> {
    let cached = JWKS_CACHE.with(|slot| slot.borrow().clone());
    let find =
        |keys: &[RsaJwk]| kid.and_then(|kid| keys.iter().find(|key| key.kid == kid).cloned());

    // (document, whether it was fetched for this request): the flag is
    // what the single-refetch-on-unknown-kid rule keys off.
    let (mut keys, fresh) = match cached.as_ref().and_then(|(fetched_at_ms, keys)| {
        (decide_jwks(Some(*fetched_at_ms), now) == cachet_core::auth::JwksDecision::UseCached)
            .then(|| keys.clone())
    }) {
        Some(keys) => (keys, false),
        None => match fetch_jwks(env).await {
            Ok(keys) => {
                JWKS_CACHE.with(|slot| {
                    *slot.borrow_mut() = Some((now.as_u64(), keys.clone()));
                });
                (keys, true)
            }
            Err(fresh) => match cached {
                Some((_, keys)) => {
                    log::event(
                        "warn",
                        "auth.jwks_fetch_failed_stale_fallback",
                        &[("error", format!("{fresh:?}"))],
                    );
                    (keys, false)
                }
                None => return Err(fresh),
            },
        },
    };

    if let Some(key) = find(&keys) {
        return Ok(key);
    }
    // An unknown kid means the issuer rotated since the cache entry: one
    // refetch, never a loop.
    if refetch_once_allowed(fresh) {
        keys = fetch_jwks(env)
            .await
            .map_err(|_| ClientError::Unauthorized)?;
        JWKS_CACHE.with(|slot| {
            *slot.borrow_mut() = Some((now.as_u64(), keys.clone()));
        });
        if let Some(key) = find(&keys) {
            return Ok(key);
        }
    }
    Err(ClientError::Unauthorized)
}

/// The write-path credential check: bearer, RS256 against the JWKS, claim
/// policy. Answers the identity a successful token attests, for the routes
/// that bind it (leases pin the repository claims).
///
/// # Errors
///
/// [`ClientError::Unauthorized`] for a missing, forged, or expired token;
/// [`ClientError::MalformedAuth`] for a credential whose header shape
/// cannot parse (oversized, wrong scheme); [`ClientError::ForbiddenOrg`]
/// for a valid token outside the deployment's orgs;
/// [`ClientError::AuthUnavailable`] when no trustworthy JWKS can be had.
pub async fn authorize_write(
    env: &Env,
    now: UnixMillis,
    authorization: Option<&str>,
) -> cachet_core::error::Result<OidcIdentity> {
    let config = oidc_config(env)?;

    let token = match authorization {
        None => return Err(ClientError::Unauthorized),
        Some(header) if header.len() > cachet_core::constants::AUTH_HEADER_BYTES_MAX => {
            return Err(ClientError::MalformedAuth);
        }
        Some(header) => header
            .strip_prefix("Bearer ")
            .ok_or(ClientError::MalformedAuth)?,
    };
    let decoded = decode_jwt(token).map_err(|_| ClientError::Unauthorized)?;
    // why: algorithm selection is policy, and it happens before any
    // signature work: a stripped or downgraded header must never negotiate
    // anything; RS256 is the only scheme this deployment accepts.
    if decoded
        .header
        .get("alg")
        .and_then(serde_json::Value::as_str)
        != Some("RS256")
    {
        return Err(ClientError::Unauthorized);
    }

    let key = jwks_key_for(env, now, decoded.kid.as_deref()).await?;
    let verified = verify_rs256(&key, &decoded.signing_input, &decoded.signature)
        .map_err(|_| ClientError::Unauthorized)?;
    if !verified {
        return Err(ClientError::Unauthorized);
    }

    verify_claims(&decoded.claims, &config, now)
}
