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

thread_local! {
    static READ_DECISION_MEMO: std::cell::RefCell<std::collections::BTreeMap<String, MemoEntry>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}

struct MemoEntry {
    expires_at_ms: u64,
    decision: MemoDecision,
}

#[derive(Clone)]
enum MemoDecision {
    /// The identity this credential resolved to, whole. The caller class
    /// is part of what was decided, and a login alone cannot be turned
    /// back into one.
    Member {
        identity: ReadIdentity,
    },
    Deny,
}

/// The memo's admit TTL: strictly inside the KV verdict's 600s window, so
/// the isolate can abbreviate verification cost but never the revocation
/// bound the threat model documents.
const MEMO_ALLOW_TTL_MS: u64 = 120_000;

/// The denial TTL: strictly inside the KV verdict's 60s window, for the
/// same reason the allow side is bounded.
const MEMO_DENY_TTL_MS: u64 = 45_000;

/// Resident entries before eviction sweeps run: a memo is a bounded
/// convenience, never state.
const MEMO_ENTRY_CAP: usize = 1_024;

/// Read a live memo decision, recording the hit for the event stream.
fn memo_read(key: &str, now: UnixMillis) -> Option<MemoDecision> {
    READ_DECISION_MEMO.with(|memo| {
        let borrowed = memo.borrow();
        match borrowed.get(key) {
            Some(entry) if now.as_u64() < entry.expires_at_ms => Some(entry.decision.clone()),
            _ => None,
        }
    })
}

/// Record a decision. Evict expired entries at the cap, then skip caching
/// if the map stays full: degradation keeps the unmemoized path.
fn memo_write(key: &str, decision: MemoDecision, expires_at_ms: u64, now: UnixMillis) {
    READ_DECISION_MEMO.with(|memo| {
        let mut borrowed = memo.borrow_mut();
        if borrowed.len() >= MEMO_ENTRY_CAP {
            borrowed.retain(|_, entry| entry.expires_at_ms > now.as_u64());
        }
        if borrowed.len() < MEMO_ENTRY_CAP {
            borrowed.insert(
                key.to_string(),
                MemoEntry {
                    expires_at_ms,
                    decision,
                },
            );
        }
    });
}

/// Drop one decision. A revoked credential that the memo keeps answering
/// for is a logout the holder can watch fail, so revocation clears the
/// isolate that served it.
fn memo_forget(key: &str) {
    READ_DECISION_MEMO.with(|memo| {
        memo.borrow_mut().remove(key);
    });
}

/// The read-time identity: who the request authenticates as, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadIdentity {
    /// A person's machine: a credential this deployment issued, or the
    /// GitHub token behind one.
    Token { login: String },
    /// A workflow run, holding the OIDC token it also writes with. Its
    /// own variant because the two answer different questions: a run is
    /// never an admin, and counting it as a laptop would make every
    /// read statistic claim people were at their desks.
    Ci {
        /// The repository owner the run belongs to.
        login: String,
        /// `owner/repo` of the run.
        repository: String,
        /// The ref it ran on.
        reference: String,
    },
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
    let _ = headers.set(
        "user-agent",
        concat!("cachet-worker/", env!("CARGO_PKG_VERSION")),
    );
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

/// Resolve a device-flow token: the isolate memo first, then sha256 → KV
/// verdict → GitHub on a miss. An OIDC-shaped token takes the write
/// path's own verification instead: CI runners substitute with the same
/// credential they push with, and the claim check is the org gate either
/// way. Sessions never ride the memo: logout revokes by KV deletion, and
/// an isolate entry would outlive that promise.
/// What this isolate already decided about a credential, if anything.
/// A hit costs no I/O at all, which is what makes a hot laptop's reads
/// free after the first.
fn memoized_answer(digest_hex: &str, now: UnixMillis) -> Option<Result<ReadIdentity>> {
    let decision = memo_read(digest_hex, now)?;
    let kind = match &decision {
        MemoDecision::Member { .. } => "allow",
        MemoDecision::Deny => "deny",
    };
    log::event("info", "auth.memo_hit", &[("kind", kind.to_string())]);
    Some(match decision {
        MemoDecision::Member { identity } => Ok(identity),
        MemoDecision::Deny => Err(ClientError::Unauthorized),
    })
}

async fn resolve_token(env: &Env, now: UnixMillis, token: &str) -> Result<ReadIdentity> {
    let digest_hex = hex_digest(&sha256(token.as_bytes()));
    if let Some(answer) = memoized_answer(&digest_hex, now) {
        return answer;
    }
    // A credential this deployment issued answers from its own record:
    // no GitHub call, because nothing here holds a GitHub credential to
    // make one with. The record's lifetime is the revocation window
    // (ADR 0002), and it is checked here rather than left to KV's
    // eventual expiry.
    if cachet_core::read_token::looks_like_read_token(token) {
        return resolve_issued_token_memoized(env, now, &digest_hex).await;
    }
    if cachet_core::auth::looks_like_oidc_token(token) {
        let identity =
            crate::auth::authorize_write(env, now, Some(&format!("Bearer {token}"))).await?;
        // why: an OIDC token is never an admin — the admins list names
        // human logins, and this field is what admin gating compares.
        // And the memo may abbreviate verification cost, never the
        // token's own lifetime: the entry dies at min(TTL, exp minus the
        // clock tolerance), so a replayed token never outlives what the
        // claim check already refused.
        let memo_expiry = cachet_crypto::rs256::decode_jwt(token)
            .ok()
            .and_then(|decoded| {
                decoded
                    .claims
                    .get("exp")
                    .and_then(serde_json::Value::as_u64)
            })
            .map_or(now.as_u64() + MEMO_ALLOW_TTL_MS, |exp_seconds| {
                exp_seconds
                    .saturating_mul(1_000)
                    .saturating_sub(cachet_core::constants::OIDC_CLOCK_TOLERANCE_MS)
                    .min(now.as_u64() + MEMO_ALLOW_TTL_MS)
            });
        let read_identity = ReadIdentity::Ci {
            login: identity.repository_owner,
            repository: identity.repository,
            reference: identity.ref_,
        };
        memo_write(
            &digest_hex,
            MemoDecision::Member {
                identity: read_identity.clone(),
            },
            memo_expiry,
            now,
        );
        return Ok(read_identity);
    }
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
                let identity = ReadIdentity::Token {
                    login: verdict.login,
                };
                memo_write(
                    &digest_hex,
                    MemoDecision::Member {
                        identity: identity.clone(),
                    },
                    now.as_u64() + MEMO_ALLOW_TTL_MS,
                    now,
                );
                Ok(identity)
            } else {
                memo_write(
                    &digest_hex,
                    MemoDecision::Deny,
                    now.as_u64() + MEMO_DENY_TTL_MS,
                    now,
                );
                Err(ClientError::Unauthorized)
            };
        }
    }
    let verdict = check_github_token(env, now, token).await?;
    cache_verdict(&kv, &digest_hex, &verdict).await;
    if verdict.org_member {
        memo_write(
            &digest_hex,
            MemoDecision::Member {
                identity: ReadIdentity::Token {
                    login: verdict.login.clone(),
                },
            },
            now.as_u64() + MEMO_ALLOW_TTL_MS,
            now,
        );
        Ok(ReadIdentity::Token {
            login: verdict.login,
        })
    } else {
        memo_write(
            &digest_hex,
            MemoDecision::Deny,
            now.as_u64() + MEMO_DENY_TTL_MS,
            now,
        );
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
    // why: a workflow run is never an admin. The admins list names human
    // GitHub logins, and a run's login is its repository owner, so an
    // OIDC credential could otherwise be admitted by an org whose slug
    // happened to match a listed name.
    let (ReadIdentity::Token { login } | ReadIdentity::Session { login }) = identity else {
        return Err(ClientError::ForbiddenAdmin);
    };
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

/// The credential a request presents, from either scheme. The exchange
/// and revoke routes take it the same way every read does, so a caller
/// that can already talk to this cache needs no new plumbing.
#[must_use]
pub fn presented_token(req: &Request) -> Option<String> {
    let header = req.headers().get("authorization").ok()??;
    if header.len() > cachet_core::constants::AUTH_HEADER_BYTES_MAX {
        return None;
    }
    header
        .strip_prefix("Bearer ")
        .map(ToString::to_string)
        .or_else(|| basic_password(&header))
}

/// Resolve an issued token and remember the answer for this isolate.
///
/// The memo bounds the KV reads a hot laptop costs: a warm isolate
/// answers a repeat read with no I/O at all. Revocation clears it, so a
/// logout is not something the holder can watch fail.
async fn resolve_issued_token_memoized(
    env: &Env,
    now: UnixMillis,
    digest_hex: &str,
) -> Result<ReadIdentity> {
    let identity = resolve_issued_token(env, now, digest_hex).await;
    match &identity {
        Ok(identity) => memo_write(
            digest_hex,
            MemoDecision::Member {
                identity: identity.clone(),
            },
            now.as_u64() + MEMO_ALLOW_TTL_MS,
            now,
        ),
        Err(ClientError::Unauthorized) => memo_write(
            digest_hex,
            MemoDecision::Deny,
            now.as_u64() + MEMO_DENY_TTL_MS,
            now,
        ),
        // An outage is not a verdict: nothing is remembered, so the next
        // request asks again rather than inheriting a backend's bad
        // minute.
        _ => {}
    }
    identity
}

/// Look one issued read token up, and check the identity behind it.
///
/// The record is a pointer, not a verdict: it names the GitHub token
/// this credential stands for, and membership is re-checked against
/// GitHub through the same verdict cache every other credential uses.
/// That is what keeps the revocation window at one verdict TTL rather
/// than the credential's whole life (ADR 0002). An absent, unreadable,
/// or expired record all answer the same refusal, because a credential
/// this deployment does not recognize is not something to explain.
async fn resolve_issued_token(
    env: &Env,
    now: UnixMillis,
    digest_hex: &str,
) -> Result<ReadIdentity> {
    let kv = env
        .kv(KV_BINDING)
        .map_err(|_| ClientError::AuthUnavailable)?;
    let key = cachet_core::read_token::read_token_key(digest_hex);
    let stored = kv
        .get(&key)
        .text()
        .await
        .map_err(|_| ClientError::AuthUnavailable)?;
    let Some(text) = stored else {
        return Err(ClientError::Unauthorized);
    };
    let Ok(mut record) = cachet_core::read_token::ReadTokenRecord::parse(&text) else {
        // The worker wrote this; an unreadable one is a storage fault,
        // and refusing is still the only safe answer to the caller.
        log::event("error", "auth.read_token_corrupt", &[]);
        return Err(ClientError::Unauthorized);
    };
    if !record.is_live(now) {
        log::event("info", "auth.read_token_expired", &[]);
        return Err(ClientError::Unauthorized);
    }

    // The verdict first, and the verdict is keyed by this credential
    // rather than by the GitHub token behind it: renewing that token
    // rotates its digest, and a verdict that vanished on every renewal
    // would send every read back to GitHub for no reason.
    //
    // why: before the renewal, not after. A fresh verdict means GitHub
    // does not need asking, which means the GitHub token does not need
    // to be usable, which means there is nothing to renew. Renewing
    // first made every read past the eight-hour mark do it, and nix
    // opens a build with a couple of dozen requests at once: they would
    // all have raced, and since GitHub rotates a refresh token on use,
    // one would have won and the rest would have presented a spent one.
    if let Some(verdict) = fresh_verdict(&kv, digest_hex, now).await {
        return decide_membership(&record, verdict);
    }

    // GitHub has to be asked, so the token has to be able to ask.
    if record.github_token_stale(now) {
        record = renew_stored_token(env, &kv, &key, now, record).await?;
    }
    let verdict = check_github_token(env, now, &record.github_token).await?;
    cache_verdict(&kv, digest_hex, &verdict).await;
    decide_membership(&record, verdict)
}

/// The cached verdict for one issued credential, if it is still fresh.
async fn fresh_verdict(
    kv: &worker::kv::KvStore,
    digest_hex: &str,
    now: UnixMillis,
) -> Option<Verdict> {
    let verdict: Verdict = kv
        .get(&format!("{VERDICT_KEY_PREFIX}{digest_hex}"))
        .json()
        .await
        .ok()??;
    (now.saturating_ms_since(UnixMillis::new(verdict.checked_at_ms))
        < verdict_ttl_ms(verdict.org_member))
    .then_some(verdict)
}

/// Turn a membership answer into an identity, or the refusal.
fn decide_membership(
    record: &cachet_core::read_token::ReadTokenRecord,
    verdict: Verdict,
) -> Result<ReadIdentity> {
    if verdict.org_member {
        Ok(ReadIdentity::Token {
            login: verdict.login,
        })
    } else {
        log::event(
            "info",
            "auth.read_token_membership_lapsed",
            &[("login", record.login.clone())],
        );
        Err(ClientError::Unauthorized)
    }
}

/// Renew the stored GitHub token, tolerating a lost race.
///
/// GitHub rotates a refresh token on use, so two requests renewing the
/// same record means one presents a spent token and is refused. That is
/// not an error worth passing to a client: the other request has by then
/// written a working record, so a refusal is answered by re-reading and
/// using what landed. Only a re-read that is also unusable gives up.
async fn renew_stored_token(
    env: &Env,
    kv: &worker::kv::KvStore,
    key: &str,
    now: UnixMillis,
    record: cachet_core::read_token::ReadTokenRecord,
) -> Result<cachet_core::read_token::ReadTokenRecord> {
    if !record.can_renew() {
        log::event("info", "auth.github_token_unrenewable", &[]);
        return Err(ClientError::Unauthorized);
    }
    let Ok(renewed) = renew_github_token(env, now, &record).await else {
        // The lost-race path: another request rotated the refresh token
        // first, so re-read and use whatever it wrote.
        let stored = kv
            .get(key)
            .text()
            .await
            .map_err(|_| ClientError::AuthUnavailable)?;
        let Some(reread) = stored
            .as_deref()
            .and_then(|text| cachet_core::read_token::ReadTokenRecord::parse(text).ok())
            .filter(|reread| !reread.github_token_stale(now))
        else {
            log::event("info", "auth.github_token_renewal_failed", &[]);
            return Err(ClientError::Unauthorized);
        };
        log::event("info", "auth.github_token_renewed_elsewhere", &[]);
        return Ok(reread);
    };
    let ttl = renewed
        .expires_at_ms
        .saturating_sub(now.as_u64())
        .max(1_000)
        / 1_000;
    if let Ok(builder) = kv
        .put(key, renewed.serialize())
        .map(|builder| builder.expiration_ttl(ttl))
    {
        // why: best effort. A renewal that lands at GitHub but not in KV
        // costs the next request another renewal, which is a round trip,
        // not an outage.
        let _ = builder.execute().await;
    }
    log::event("info", "auth.github_token_renewed", &[]);
    Ok(renewed)
}

/// Trade the stored refresh token for a fresh access token.
///
/// GitHub waives the client secret for tokens minted through the device
/// flow, which is every credential this path holds, so the exchange
/// needs only the client id.
async fn renew_github_token(
    env: &Env,
    now: UnixMillis,
    record: &cachet_core::read_token::ReadTokenRecord,
) -> Result<cachet_core::read_token::ReadTokenRecord> {
    let client_id = env
        .var("CACHET_OAUTH_CLIENT_ID")
        .map(|value| value.to_string())
        .map_err(|_| ClientError::AuthUnavailable)?;
    let web_base = env.var("CACHET_GITHUB_WEB_URL").map_or_else(
        |_| "https://github.com".to_string(),
        |value| value.to_string(),
    );
    let reply = crate::oauth::post_token_form(
        &format!("{web_base}/login/oauth/access_token"),
        cachet_core::oauth::refresh_form(&client_id, &record.github_refresh_token),
    )
    .await?;
    // GitHub answers a refused refresh as 200 with no token, the same
    // way it answers a refused code: the absence is the refusal.
    let access = reply
        .access_token
        .filter(|token| !token.is_empty())
        .ok_or(ClientError::Unauthorized)?;
    let expires_in = reply.expires_in.unwrap_or(0);
    Ok(cachet_core::read_token::ReadTokenRecord {
        github_token: access,
        // why: a rotated refresh token replaces the old one, and GitHub
        // rotates on every use. Keeping the old one would make the next
        // renewal fail.
        github_refresh_token: reply
            .refresh_token
            .filter(|token| !token.is_empty())
            .unwrap_or_else(|| record.github_refresh_token.clone()),
        github_expires_at_ms: if expires_in == 0 {
            0
        } else {
            now.as_u64()
                .saturating_add(expires_in.saturating_mul(1_000))
        },
        ..record.clone()
    })
}

/// Issue a read credential to a caller who proved a GitHub identity.
///
/// The GitHub token is used here and nowhere else: it is checked for org
/// membership exactly once, and what the caller keeps afterwards is this
/// deployment's own token. That is the whole point (ADR 0002). A caller
/// whose membership does not hold gets the same refusal the read path
/// would have given them.
///
/// # Errors
///
/// [`ClientError::Unauthorized`] for a missing or unusable GitHub
/// credential, [`ClientError::ForbiddenOrg`] for a valid one outside the
/// deployment's orgs, [`ClientError::AuthUnavailable`] when a backend
/// cannot answer.
pub async fn issue_read_token(
    env: &Env,
    now: UnixMillis,
    github_token: &str,
    grant: &cachet_api::LoginExchangeBody,
) -> Result<cachet_api::ReadTokenIssued> {
    let verdict = check_github_token(env, now, github_token).await?;
    if verdict.login.is_empty() {
        return Err(ClientError::Unauthorized);
    }
    if !verdict.org_member {
        log::event(
            "info",
            "auth.exchange_refused",
            &[("login", verdict.login.clone())],
        );
        return Err(ClientError::ForbiddenOrg);
    }
    let token = cachet_core::read_token::format_read_token(&sample_token_body());
    let digest_hex = hex_digest(&sha256(token.as_bytes()));
    let expires_at_ms = now
        .as_u64()
        .saturating_add(cachet_core::constants::READ_TOKEN_TTL_MS);
    let record = cachet_core::read_token::ReadTokenRecord {
        login: verdict.login.clone(),
        issued_at_ms: now.as_u64(),
        expires_at_ms,
        // The GitHub credentials stay here and only here: this is what
        // keeps membership checkable without the laptop ever sending
        // them again (ADR 0002).
        github_token: github_token.to_string(),
        github_refresh_token: grant.refresh_token.clone(),
        github_expires_at_ms: if grant.expires_in_seconds == 0 {
            0
        } else {
            now.as_u64()
                .saturating_add(grant.expires_in_seconds.saturating_mul(1_000))
        },
    };
    let kv = env
        .kv(KV_BINDING)
        .map_err(|_| ClientError::AuthUnavailable)?;
    let ttl_seconds = cachet_core::constants::READ_TOKEN_TTL_MS / 1_000;
    let key = cachet_core::read_token::read_token_key(&digest_hex);
    let builder = kv
        .put(&key, record.serialize())
        .map(|builder| builder.expiration_ttl(ttl_seconds))
        .map_err(|_| ClientError::AuthUnavailable)?;
    builder
        .execute()
        .await
        .map_err(|_| ClientError::AuthUnavailable)?;
    log::event(
        "info",
        "auth.read_token_issued",
        &[
            ("login", verdict.login.clone()),
            ("renewable", (!grant.refresh_token.is_empty()).to_string()),
        ],
    );
    Ok(cachet_api::ReadTokenIssued {
        token,
        login: verdict.login,
        expires_at_ms,
    })
}

/// Delete one issued token, by the token itself. Logging out is the
/// holder proving they hold it; nobody else can name the record.
///
/// # Errors
///
/// [`ClientError::AuthUnavailable`] when KV cannot answer.
pub async fn revoke_read_token(env: &Env, token: &str) -> Result<()> {
    let digest_hex = hex_digest(&sha256(token.as_bytes()));
    let kv = env
        .kv(KV_BINDING)
        .map_err(|_| ClientError::AuthUnavailable)?;
    kv.delete(&cachet_core::read_token::read_token_key(&digest_hex))
        .await
        .map_err(|_| ClientError::AuthUnavailable)?;
    // The isolate memo would otherwise keep answering for this token for
    // up to its own TTL, which would make a logout look ignored.
    memo_forget(&digest_hex);
    log::event("info", "auth.read_token_revoked", &[]);
    Ok(())
}

/// The entropy seam (CLAUDE.md §3): sampled at the edge, formatted by
/// the core.
fn sample_token_body() -> String {
    let mut random = [0_u8; 32];
    getrandom::getrandom(&mut random).expect("getrandom cannot fail on workers");
    <base64ct::Base64UrlUnpadded as base64ct::Encoding>::encode_string(&random)
}
