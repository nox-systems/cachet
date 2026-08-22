//! Read authorization at the edge: a device-flow GitHub token validated
//! against the GitHub API and verdict-cached in KV, or a browser session
//! resolved from its cookie. Two facts guide the cut. Verdicts cache by
//! the token's sha256, so the token itself never persists, and admits
//! outlast denials (600s to 60s) because credential revocation must
//! converge in minutes while a fresh member must not bounce on the API.
//! And the API itself is a deployment variable: transport configuration,
//! so the workerd lane can substitute a stub without weakening any check.

use base64ct::Encoding as _;
use cachet_core::auth::{SessionRecord, Verdict, session_live, verdict_fresh, verdict_ttl_ms};
use cachet_core::constants::{SESSION_COOKIE_NAME, SESSION_KEY_PREFIX, VERDICT_KEY_PREFIX};
use cachet_core::error::{ClientError, Result};
use cachet_core::types::UnixMillis;
use cachet_crypto::sha256::{hex_digest, sha256};
use worker::{Env, Fetch, Headers, Method, Request};

use crate::log;

/// The KV binding for verdicts, sessions, and OAuth state.
pub(crate) const KV_BINDING: &str = "CACHET_KV";

/// The read-time identity: who the request authenticates as, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadIdentity {
    /// A validated device-flow GitHub token.
    Token { login: String },
    /// A live browser session. The session id is carried for admin routes
    /// to bind without re-reading it.
    Session { login: String },
}

/// A GitHub API answer for one membership query.
#[derive(Debug, serde::Deserialize)]
struct Membership {
    state: String,
}

/// The /user answer's login.
#[derive(Debug, serde::Deserialize)]
struct GitHubUser {
    login: String,
}

fn github_api_url(env: &Env) -> String {
    // why: overrides exist for the lane's stub; production reads the
    // default and needs no config.
    cachet_core::constants::override_or(
        env.var("CACHET_GITHUB_API_URL")
            .ok()
            .map(|value| value.to_string())
            .as_deref(),
        cachet_core::constants::GITHUB_API_URL_DEFAULT,
    )
    .to_string()
}

/// One GET to the GitHub API with the lane-wide headers.
async fn github_get(env: &Env, token: &str, path: &str) -> Result<(u16, String)> {
    let base = github_api_url(env);
    let headers = Headers::new();
    let _ = headers.set("authorization", &format!("Bearer {token}"));
    let _ = headers.set("accept", "application/vnd.github+json");
    let _ = headers.set("user-agent", "cachet-worker/0.0.1");
    let _ = headers.set("x-github-api-version", "2022-11-28");
    let mut request_init = worker::RequestInit::new();
    request_init.with_method(Method::Get);
    request_init.headers.clone_from(&headers);
    let request =
        Request::new_with_init(&format!("{base}{path}"), &request_init).map_err(|failure| {
            log::event(
                "error",
                "verdict.github_fetch_failed",
                &[
                    ("where", "request_build".to_string()),
                    ("error", failure.to_string()),
                ],
            );
            ClientError::AuthUnavailable
        })?;
    let mut response = Fetch::Request(request).send().await.map_err(|failure| {
        log::event(
            "error",
            "verdict.github_fetch_failed",
            &[
                ("where", "send".to_string()),
                ("error", failure.to_string()),
            ],
        );
        ClientError::AuthUnavailable
    })?;
    let status = response.status_code();
    let text = response.text().await.map_err(|failure| {
        log::event(
            "error",
            "verdict.github_fetch_failed",
            &[
                ("where", "body".to_string()),
                ("error", failure.to_string()),
            ],
        );
        ClientError::AuthUnavailable
    })?;
    Ok((status, text))
}

/// Ask the GitHub API for the login and org membership of `token`.
/// Returns the verdict to cache. API outage is never a verdict: it maps
/// to auth_unavailable rather than a denial, so a client is told to
/// retry, not that it is forbidden.
pub(crate) async fn check_github_token(env: &Env, now: UnixMillis, token: &str) -> Result<Verdict> {
    let (status, body) = github_get(env, token, "/user").await?;
    if status != 200 {
        return Ok(Verdict {
            login: String::new(),
            org_member: false,
            checked_at_ms: now.as_u64(),
        });
    }
    let user: GitHubUser = serde_json::from_str(&body).map_err(|_| ClientError::AuthUnavailable)?;
    let config = crate::auth::oidc_config(env)?;
    for org in &config.orgs {
        let (membership_status, membership_body) = github_get(
            env,
            token,
            &format!("/orgs/{org}/memberships/{}", user.login),
        )
        .await?;
        if membership_status == 200 {
            let membership: Membership =
                serde_json::from_str(&membership_body).map_err(|_| ClientError::AuthUnavailable)?;
            if membership.state == "active" {
                return Ok(Verdict {
                    login: user.login,
                    org_member: true,
                    checked_at_ms: now.as_u64(),
                });
            }
        }
    }
    Ok(Verdict {
        login: user.login,
        org_member: false,
        checked_at_ms: now.as_u64(),
    })
}

/// Cache-write one verdict, with the admission or denial TTL.
async fn cache_verdict(kv: &worker::kv::KvStore, digest_hex: &str, verdict: &Verdict) {
    let ttl_seconds = verdict_ttl_ms(verdict.org_member) / 1_000;
    let text = match serde_json::to_string(verdict) {
        Ok(text) => text,
        Err(failure) => {
            log::event(
                "warn",
                "verdict.serialize_failed",
                &[("error", failure.to_string())],
            );
            return;
        }
    };
    if let Ok(builder) = kv
        .put(&format!("{VERDICT_KEY_PREFIX}{digest_hex}"), text)
        .map(|builder| builder.expiration_ttl(ttl_seconds))
    {
        if let Err(failure) = builder.execute().await {
            log::event(
                "warn",
                "verdict.cache_write_failed",
                &[("error", failure.to_string())],
            );
        }
    }
}

/// Resolve a device-flow token: sha256 → KV verdict → GitHub on a miss.
/// An OIDC-shaped token takes the write path's own verification instead:
/// CI runners substitute with the same credential they push with, and the
/// claim check is the org gate either way.
async fn resolve_token(env: &Env, now: UnixMillis, token: &str) -> Result<ReadIdentity> {
    if cachet_core::auth::looks_like_oidc_token(token) {
        let identity =
            crate::auth::authorize_write(env, now, Some(&format!("Bearer {token}"))).await?;
        // why: an OIDC token is never an admin — the admins list names
        // human logins, and this field is what admin gating compares.
        return Ok(ReadIdentity::Token {
            login: identity.repository_owner,
        });
    }
    let digest_hex = hex_digest(&sha256(token.as_bytes()));
    let kv = env
        .kv(KV_BINDING)
        .map_err(|_| ClientError::AuthUnavailable)?;
    if let Ok(Some(verdict)) = kv
        .get(&format!("{VERDICT_KEY_PREFIX}{digest_hex}"))
        .json::<Verdict>()
        .await
    {
        if verdict_fresh(&verdict, now) {
            return if verdict.org_member {
                Ok(ReadIdentity::Token {
                    login: verdict.login,
                })
            } else {
                Err(ClientError::Unauthorized)
            };
        }
    }
    let verdict = check_github_token(env, now, token).await?;
    cache_verdict(&kv, &digest_hex, &verdict).await;
    if verdict.org_member {
        Ok(ReadIdentity::Token {
            login: verdict.login,
        })
    } else {
        Err(ClientError::Unauthorized)
    }
}

/// Resolve the session cookie: `sess/{id}` in KV, live by absolute age.
async fn resolve_session(env: &Env, now: UnixMillis, session_id: &str) -> Result<ReadIdentity> {
    if session_id.len() > 256 {
        return Err(ClientError::Unauthorized);
    }
    let kv = env
        .kv(KV_BINDING)
        .map_err(|_| ClientError::AuthUnavailable)?;
    let Some(record) = kv
        .get(&format!("{SESSION_KEY_PREFIX}{session_id}"))
        .json::<SessionRecord>()
        .await
        .map_err(|_| ClientError::AuthUnavailable)?
    else {
        return Err(ClientError::Unauthorized);
    };
    if !session_live(record.created_at_ms, now) {
        return Err(ClientError::Unauthorized);
    }
    Ok(ReadIdentity::Session {
        login: record.login,
    })
}

/// Pull a session id off a Cookie header, if one carries it.
pub(crate) fn session_id_from(cookie_header: Option<&str>) -> Option<String> {
    let header = cookie_header?;
    for part in header.split(';') {
        let part = part.trim();
        if let Some((name, value)) = part.split_once('=') {
            if name == SESSION_COOKIE_NAME {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Resolve read authorization: token first (it can carry an explicit
/// answer), then the session cookie. A deployment with neither resolves
/// nothing, and the answer is 401.
///
/// # Errors
///
/// [`ClientError::Unauthorized`] for a missing or unusable credential;
/// [`ClientError::MalformedAuth`] for a present header that cannot parse;
/// [`ClientError::AuthUnavailable`] when a backend cannot answer.
pub async fn authorize_read(env: &Env, now: UnixMillis, req: &Request) -> Result<ReadIdentity> {
    let headers = req.headers();
    let authorization = headers
        .get("authorization")
        .map_err(|_| ClientError::MalformedAuth)?;
    if let Some(header) = authorization.as_deref() {
        // A present-but-unparseable credential is malformed_auth (400),
        // not unauthorized (401): the caller sent something and it was
        // broken, which a retry-with-changes can fix while a retry cannot.
        if header.len() > cachet_core::constants::AUTH_HEADER_BYTES_MAX {
            return Err(ClientError::MalformedAuth);
        }
        return match header
            .strip_prefix("Bearer ")
            .map(ToString::to_string)
            .or_else(|| basic_password(header))
        {
            Some(token) => resolve_token(env, now, &token).await,
            None => Err(ClientError::MalformedAuth),
        };
    }
    let cookie = headers
        .get("cookie")
        .map_err(|_| ClientError::MalformedAuth)?;
    if let Some(session_id) = session_id_from(cookie.as_deref()) {
        return resolve_session(env, now, &session_id).await;
    }
    Err(ClientError::Unauthorized)
}

/// The admins list: the CACHET_ADMINS comma list, absent meaning nobody.
/// Comparison is exact: GitHub logins are case-preserving and the config
/// value is operator-written.
pub(crate) fn admins(env: &Env) -> Vec<String> {
    env.var("CACHET_ADMINS")
        .ok()
        .map(|value| {
            value
                .to_string()
                .split(',')
                .map(|part| part.trim().to_string())
                .filter(|part| !part.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve a read credential and require adminship on top: every
/// `/api/self` route's single gate.
///
/// # Errors
///
/// [`ClientError::Unauthorized`] for a missing or unusable credential;
/// [`ClientError::ForbiddenAdmin`] for a working credential whose login
/// the deployment does not list.
pub(crate) async fn require_admin(env: &Env, now: UnixMillis, req: &Request) -> Result<String> {
    let identity = authorize_read(env, now, req).await?;
    let (ReadIdentity::Token { login } | ReadIdentity::Session { login }) = identity;
    if admins(env).iter().any(|admin| admin == &login) {
        Ok(login)
    } else {
        log::event("info", "api.admin_refused", &[("login", login)]);
        Err(ClientError::ForbiddenAdmin)
    }
}

/// The password half of `Authorization: Basic base64(user:password)`: the
/// GitHub token arrives as the password because netrc clients speak basic
/// auth, never as a Bearer scheme.
fn basic_password(header: &str) -> Option<String> {
    let encoded = header.strip_prefix("Basic ")?.trim();
    let decoded = base64ct::Base64::decode_vec(encoded).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    text.split_once(':')
        .map(|(_, password)| password.to_string())
        .filter(|password| !password.is_empty())
}
