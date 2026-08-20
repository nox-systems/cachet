//! The browser OAuth endpoints: `/_auth/login`, `/_auth/callback`, and
//! `/logout`. The flow's decisions (URL construction, query grammar,
//! cookie strings, target choice) are cachet-core's; this module samples
//! the entropy in, performs the KV and GitHub I/O, and renders. The
//! client secret enters only as the `GITHUB_OAUTH_CLIENT_SECRET` binding
//! and is never logged or returned, and a consumed state is deleted
//! before its validity is judged so a callback can never be replayed.

use cachet_core::auth::SessionRecord;
use cachet_core::constants::{OAUTH_STATE_KEY_PREFIX, SESSION_KEY_PREFIX};
use cachet_core::error::ClientError;
use cachet_core::oauth::{self, CallbackTarget, OAuthStateRecord};
use cachet_core::types::UnixMillis;
use worker::{Env, Fetch, Headers, Method, Request, RequestInit, Response, Result, Url};

use crate::{error, log, verdict};

/// The binding name of the OAuth client secret.
const CLIENT_SECRET_BINDING: &str = "GITHUB_OAUTH_CLIENT_SECRET";

/// The deployment's OAuth configuration. Missing pieces cannot run the
/// flow, so they answer auth_unavailable like every other auth backend.
struct OAuthConfig {
    web_base: String,
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    ui_origin: Option<String>,
}

fn oauth_config(env: &Env) -> cachet_core::error::Result<OAuthConfig> {
    let web_base = env
        .var("CACHET_GITHUB_WEB_URL")
        .map_err(|_| ClientError::AuthUnavailable)?
        .to_string();
    let client_id = env
        .var("CACHET_OAUTH_CLIENT_ID")
        .map_err(|_| ClientError::AuthUnavailable)?
        .to_string();
    let host = env
        .var("CACHET_HOST")
        .map_err(|_| ClientError::AuthUnavailable)?
        .to_string();
    let client_secret = env
        .secret(CLIENT_SECRET_BINDING)
        .map_err(|_| ClientError::AuthUnavailable)?
        .to_string();
    // The one optional piece: absent means the callback answers a bare 204
    // instead of redirecting to a UI.
    let ui_origin = env.var("CACHET_UI_ORIGIN").ok().map(|v| v.to_string());
    Ok(OAuthConfig {
        web_base,
        client_id,
        client_secret,
        redirect_uri: format!("https://{host}{}", oauth::CALLBACK_PATH),
        ui_origin,
    })
}

/// The entropy seam (CLAUDE.md §3): sampled at the edge, formatted by the
/// core.
fn sample_identifier() -> String {
    let mut random = [0_u8; 16];
    getrandom::getrandom(&mut random).expect("getrandom cannot fail on workers");
    oauth::id_from_random(&random)
}

/// The KV handle, or the outage answer.
fn kv(env: &Env) -> cachet_core::error::Result<worker::kv::KvStore> {
    env.kv(verdict::KV_BINDING)
        .map_err(|_| ClientError::AuthUnavailable)
}

/// Write one JSON record under `key` with `ttl_seconds`; failures log and
/// return the outage code. OAuth state and sessions share the shape.
async fn store_record(
    kv: &worker::kv::KvStore,
    key: &str,
    text: String,
    ttl_seconds: u64,
    failure_event: &'static str,
) -> cachet_core::error::Result<()> {
    let builder = kv
        .put(key, text)
        .map(|builder| builder.expiration_ttl(ttl_seconds))
        .map_err(|failure| {
            log::event("error", failure_event, &[("error", failure.to_string())]);
            ClientError::AuthUnavailable
        })?;
    builder.execute().await.map_err(|failure| {
        log::event("error", failure_event, &[("error", failure.to_string())]);
        ClientError::AuthUnavailable
    })
}

/// Responses with credential material never touch any cache.
fn no_store_headers() -> Result<Headers> {
    let headers = Headers::new();
    headers.set("cache-control", "no-store")?;
    Ok(headers)
}

/// `GET /_auth/login`: mint a state, remember it briefly, and send the
/// browser to GitHub with exactly the scope the verdict path needs.
pub async fn login(env: &Env, now: UnixMillis) -> Result<Response> {
    let config = match oauth_config(env) {
        Ok(config) => config,
        Err(code) => return error::problem_response(code),
    };
    let kv = match kv(env) {
        Ok(kv) => kv,
        Err(code) => return error::problem_response(code),
    };
    let state = sample_identifier();
    let record = OAuthStateRecord {
        issued_at_ms: now.as_u64(),
    };
    let Ok(record_text) = serde_json::to_string(&record) else {
        return error::problem_response(ClientError::AuthUnavailable);
    };
    if let Err(code) = store_record(
        &kv,
        &format!("{OAUTH_STATE_KEY_PREFIX}{state}"),
        record_text,
        oauth::state_ttl_seconds(),
        "oauth.state_store_failed",
    )
    .await
    {
        return error::problem_response(code);
    }
    let location = oauth::authorize_url(
        &config.web_base,
        &config.client_id,
        &config.redirect_uri,
        &state,
    );
    log::event("info", "oauth.login_started", &[]);
    redirect_response(&location)
}

/// `GET /_auth/callback`: consume the state, exchange the code, enforce
/// the org gate, and mint the session. The exchange's token rides the
/// same GitHub validation and org check as every other read credential.
pub async fn callback(env: &Env, now: UnixMillis, req: &Request) -> Result<Response> {
    let query = req.url()?.query().unwrap_or("").to_string();
    let params = match oauth::parse_callback_query(&query) {
        Ok(params) => params,
        Err(code) => return error::problem_response(code),
    };
    let kv = match kv(env) {
        Ok(kv) => kv,
        Err(code) => return error::problem_response(code),
    };
    if let Err(code) = consume_state(&kv, &params.state, now).await {
        return error::problem_response(code);
    }
    let config = match oauth_config(env) {
        Ok(config) => config,
        Err(code) => return error::problem_response(code),
    };
    let token = match exchange_code(&config, &params.code).await {
        Ok(token) => token,
        Err(code) => return error::problem_response(code),
    };
    let decision = match verdict::check_github_token(env, now, &token).await {
        Ok(decision) => decision,
        Err(code) => return error::problem_response(code),
    };
    if !decision.org_member {
        log::event(
            "info",
            "oauth.login_refused",
            &[("login", decision.login.clone())],
        );
        return error::problem_response(ClientError::ForbiddenOrg);
    }
    let session_id = sample_identifier();
    let record = SessionRecord {
        login: decision.login.clone(),
        created_at_ms: now.as_u64(),
    };
    let Ok(record_text) = serde_json::to_string(&record) else {
        return error::problem_response(ClientError::AuthUnavailable);
    };
    if let Err(code) = store_record(
        &kv,
        &format!("{SESSION_KEY_PREFIX}{session_id}"),
        record_text,
        oauth::session_ttl_seconds(),
        "oauth.session_store_failed",
    )
    .await
    {
        return error::problem_response(code);
    }

    log::event(
        "info",
        "oauth.login_completed",
        &[("login", decision.login)],
    );
    let headers = no_store_headers()?;
    headers.set("set-cookie", &oauth::session_cookie(&session_id))?;
    // The Location joins the cookie's header set directly: `with_headers`
    // replaces the whole set, so a redirect built first and given these
    // headers would lose the very header that makes it a redirect.
    match oauth::callback_target(config.ui_origin.as_deref()) {
        CallbackTarget::Redirect(origin) => {
            if Url::parse(&origin).is_err() {
                log::event("error", "oauth.ui_origin_invalid", &[]);
                return error::problem_response(ClientError::AuthUnavailable);
            }
            headers.set("location", &origin)?;
            Ok(Response::empty()?.with_status(302).with_headers(headers))
        }
        CallbackTarget::Empty => Ok(Response::empty()?.with_status(204).with_headers(headers)),
    }
}

/// A 302 with the Location set in one piece: `Response::redirect` builds
/// a web_sys response whose headers a later `with_headers` would replace
/// wholesale, Location included, and the callback's variant carries a
/// cookie header alongside, so neither route goes through it.
fn redirect_response(location: &str) -> Result<Response> {
    let headers = Headers::new();
    headers.set("location", location)?;
    headers.set("cache-control", "no-store")?;
    Ok(Response::empty()?.with_status(302).with_headers(headers))
}

/// Read and delete the state, then judge it. Consumed first: single-use
/// even when the rest of the flow fails, so a leaked callback URL is
/// worth nothing twice. A delete failure only shortens the replay window
/// to the state TTL, which the liveness check still bounds.
async fn consume_state(
    kv: &worker::kv::KvStore,
    state: &str,
    now: UnixMillis,
) -> cachet_core::error::Result<()> {
    let key = format!("{OAUTH_STATE_KEY_PREFIX}{state}");
    let record = kv
        .get(&key)
        .json::<OAuthStateRecord>()
        .await
        .map_err(|failure| {
            log::event(
                "error",
                "oauth.state_read_failed",
                &[("error", failure.to_string())],
            );
            ClientError::AuthUnavailable
        })?;
    if let Err(failure) = kv.delete(&key).await {
        log::event(
            "warn",
            "oauth.state_delete_failed",
            &[("error", failure.to_string())],
        );
    }
    match record {
        Some(record) if oauth::state_live(&record, now) => Ok(()),
        _ => Err(ClientError::OauthStateUnknown),
    }
}

/// `POST /logout`: drop the session if one is named, expire the cookie
/// either way. Idempotent by design: the client is cleaning up state it
/// may only half remember.
pub async fn logout(env: &Env, req: &Request) -> Result<Response> {
    let cookie = req.headers().get("cookie").ok().flatten();
    if let Some(session_id) = verdict::session_id_from(cookie.as_deref()) {
        if session_id.len() <= 256 {
            if let Ok(kv) = env.kv(verdict::KV_BINDING) {
                if let Err(failure) = kv
                    .delete(&format!("{SESSION_KEY_PREFIX}{session_id}"))
                    .await
                {
                    log::event(
                        "warn",
                        "oauth.session_delete_failed",
                        &[("error", failure.to_string())],
                    );
                }
            }
        }
    }
    log::event("info", "oauth.logout", &[]);
    let headers = no_store_headers()?;
    headers.set("set-cookie", &oauth::clear_session_cookie())?;
    Ok(Response::empty()?.with_status(204).with_headers(headers))
}

/// The token-exchange answer. GitHub reports even a refused code as 200
/// with an `error` field, so success is the presence of the token.
#[derive(serde::Deserialize)]
struct ExchangeReply {
    access_token: Option<String>,
}

/// Exchange the callback code for a GitHub token. A transport failure or
/// an unparseable answer is an outage and answers 503; a 200 without a
/// token is GitHub refusing the code, which is the client's flow to
/// restart.
async fn exchange_code(config: &OAuthConfig, code: &str) -> cachet_core::error::Result<String> {
    let headers = Headers::new();
    let _ = headers.set("accept", "application/json");
    let _ = headers.set("content-type", "application/x-www-form-urlencoded");
    let _ = headers.set("user-agent", "cachet-worker/0.0.1");
    let mut init = RequestInit::new();
    init.with_method(Method::Post);
    init.headers.clone_from(&headers);
    init.with_body(Some(
        oauth::exchange_form(&config.client_id, &config.client_secret, code).into(),
    ));
    let request = Request::new_with_init(
        &format!("{}/login/oauth/access_token", config.web_base),
        &init,
    )
    .map_err(|_| ClientError::AuthUnavailable)?;
    let mut response = Fetch::Request(request)
        .send()
        .await
        .map_err(|_| ClientError::AuthUnavailable)?;
    if response.status_code() != 200 {
        return Err(ClientError::AuthUnavailable);
    }
    let reply: ExchangeReply = response
        .json()
        .await
        .map_err(|_| ClientError::AuthUnavailable)?;
    reply
        .access_token
        .filter(|token| !token.is_empty())
        .ok_or(ClientError::Unauthorized)
}
