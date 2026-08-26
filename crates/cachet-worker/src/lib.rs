//! cachet-worker is the deployable leaf (CLAUDE.md §4). It adapts the
//! Cloudflare request, environment, R2 bucket, KV namespace, Cache API, and
//! outbound GitHub calls into cachet-core decisions and renders results.
//! All ambient clock, entropy, and I/O live here behind the seams; the
//! signing key enters only as a Workers secret binding and is never logged
//! or returned.

#![forbid(unsafe_code)]

mod api;
mod auth;
mod error;
mod gc;
mod log;
mod oauth;
mod probe;
mod read;
mod roots;
mod stats;
mod verdict;
mod write;

use cachet_core::constants::{NARINFO_KEY_SUFFIX, ROOTS_KEY_PREFIX};
use cachet_core::error::ClientError;
use cachet_core::keys::{parse_nar_request_path, parse_narinfo_request_path};
use cachet_core::read::ObjectKind;
use cachet_core::types::{ProjectName, UnixMillis};
use worker::{Context, Env, Method, Request, Response, Result, event};

/// The fetch entry point. The check order is the contract: key grammar
/// first (a malformed path is a 400 before any credential is read),
/// authorization second (an unauthenticated write is a 401 before any
/// 411), then the route's own guards. Everything outside the cache-object
/// grammars and the handshake is a 404.
#[event(fetch)]
async fn fetch(req: Request, env: Env, ctx: Context) -> Result<Response> {
    let method = req.method();
    // why: the Clock seam (CLAUDE.md §3): the one ambient sample per
    // request, injected everywhere below.
    let now = UnixMillis::new(worker::Date::now().as_millis());
    let path = req.path();

    if method == Method::Get || method == Method::Head {
        if let Some(response) = fixed_get_routes(&env, now, &req, &path, &method).await {
            return response;
        }
        if path == "/roots" || path == "/roots/" {
            let authorized = verdict::authorize_read(&env, now, &req).await;
            if let Err(code) = authorized {
                return error::problem_response(code);
            }
            return roots::list_projects(&env).await;
        }
        if let Some(name) = path.strip_prefix(&format!("/{ROOTS_KEY_PREFIX}")) {
            let project = match ProjectName::parse(name) {
                Ok(project) => project,
                Err(code) => return error::problem_response(code),
            };
            let authorized = verdict::authorize_read(&env, now, &req).await;
            if let Err(code) = authorized {
                return error::problem_response(code);
            }
            return roots::read_lease(&env, &project).await;
        }
        if let Some(response) = read_routes(&env, &ctx, now, &req, &method, &path).await {
            return response;
        }
        return error::problem_response(ClientError::NotFound);
    }

    if let Some(name) = path.strip_prefix(&format!("/{ROOTS_KEY_PREFIX}")) {
        if method == Method::Post {
            let project = match ProjectName::parse(name) {
                Ok(project) => project,
                Err(code) => return error::problem_response(code),
            };
            let identity = match authorize_write(&env, now, &req).await {
                Ok(identity) => identity,
                Err(code) => return error::problem_response(code),
            };
            // The claims that shape the document come from the token, and
            // the branch the token ran on decides whether it may renew at
            // all; renewing is the one route where ref matters.
            let config = match auth::oidc_config(&env) {
                Ok(config) => config,
                Err(code) => return error::problem_response(code),
            };
            return roots::renew_lease(&env, &config, &identity, &project, now, req).await;
        }
    }

    if path == "/logout" && method == Method::Post {
        return oauth::logout(&env, &req).await;
    }

    if path == "/api/probe" && method == Method::Post {
        return probe::answer_probe(&env, now, req).await;
    }

    // The credential exchange: a GitHub token in, this deployment's own
    // read token out. The GitHub token is checked once, here, and never
    // reaches the bucket, the netrc, or any later request (ADR 0002).
    if path == "/api/login/exchange" && method == Method::Post {
        let Some(github_token) = verdict::presented_token(&req) else {
            return error::problem_response(ClientError::Unauthorized);
        };
        // An absent or unreadable body is a login with nothing to renew
        // from, which is exactly what a non-expiring OAuth App produces.
        let mut req = req;
        let grant: cachet_api::LoginExchangeBody = req.json().await.unwrap_or_default();
        return match verdict::issue_read_token(&env, now, &github_token, &grant).await {
            Ok(issued) => api::json_no_store(&issued),
            Err(code) => error::problem_response(code),
        };
    }

    // Logging out: the holder presents the token, and the deployment
    // forgets it. Nobody else can name the record, because it is keyed
    // by the token's own hash.
    if path == "/api/login/revoke" && method == Method::Post {
        let Some(token) = verdict::presented_token(&req) else {
            return error::problem_response(ClientError::Unauthorized);
        };
        return match verdict::revoke_read_token(&env, &token).await {
            Ok(()) => Response::empty().map(|response| response.with_status(204)),
            Err(code) => error::problem_response(code),
        };
    }

    if path.ends_with(NARINFO_KEY_SUFFIX) && method == Method::Put {
        let hash = match parse_narinfo_request_path(&path) {
            Ok(hash) => hash,
            Err(code) => return error::problem_response(code),
        };
        let authorized = authorize_write(&env, now, &req).await;
        let caller = write_caller(&authorized);
        if let Err(code) = authorized {
            return error::problem_response(code);
        }
        let bytes = uploaded_bytes(&req);
        let answered = write::put_narinfo(&env, req, &hash).await;
        return count_write(
            &env,
            cachet_core::stats::StatKind::Narinfo,
            bytes,
            &caller,
            answered,
        );
    }

    if path.starts_with("/nar/") {
        return write_routes(req, env, method, now, &path).await;
    }

    error::problem_response(ClientError::NotFound)
}

/// The GETs that match whole fixed paths, matched before any key grammar
/// touches the request: the handshake, the two discovery documents, and
/// the browser login's first leg.
async fn fixed_get_routes(
    env: &Env,
    now: UnixMillis,
    req: &Request,
    path: &str,
    method: &Method,
) -> Option<Result<Response>> {
    if path == "/nix-cache-info" {
        return Some(read::serve_cache_info());
    }
    if *method != Method::Get {
        return None;
    }
    if path == "/api/public/config" {
        return Some(api::public_config(env));
    }
    if path == "/api/self/health" {
        return Some(api::health(env, now, req).await);
    }
    if path == "/api/whoami" {
        return Some(api::whoami(env, now, req).await);
    }
    if path == "/api/openapi.json" {
        return Some(api::openapi_document());
    }
    if path == "/api/self/gc-runs" {
        return Some(api::gc_runs_list(env, now, req).await);
    }
    if let Some(run_id) = path.strip_prefix("/api/self/gc-runs/") {
        return Some(api::gc_run_read(env, now, req, run_id).await);
    }
    if path == "/api/self/events" {
        return Some(api::stats_events(env, now, req).await);
    }
    if path == "/api/self/stats" {
        return Some(api::stats(env, now, req).await);
    }
    if path == "/_auth/login" {
        return Some(oauth::login(env, now).await);
    }
    if path == "/_auth/callback" {
        return Some(oauth::callback(env, now, req).await);
    }
    None
}

/// The four write verbs on the NAR key space: the single PUT and the
/// multipart sequence, distinguished by query parameters the S3 style
/// nix tooling expects, so the bucket key stays identical between them.
async fn write_routes(
    mut req: Request,
    env: Env,
    method: Method,
    now: UnixMillis,
    path: &str,
) -> Result<Response> {
    let key = match parse_nar_request_path(path) {
        Ok(key) => key,
        Err(code) => return error::problem_response(code),
    };
    let authorized = authorize_write(&env, now, &req).await;
    let caller = write_caller(&authorized);
    if let Err(code) = authorized {
        return error::problem_response(code);
    }
    let kind = write_kind(&req, &method);
    let bytes = uploaded_bytes(&req);
    let answered = match method {
        Method::Put => {
            match (
                query_value(&req, "uploadId"),
                query_value(&req, "partNumber"),
            ) {
                (None, None) => write::put_nar(&env, req, &key).await,
                (Some(upload_id), Some(part_number)) => {
                    if part_number.is_empty() || !part_number.bytes().all(|b| b.is_ascii_digit()) {
                        return error::problem_response(ClientError::PartNumberInvalid);
                    }
                    match part_number.parse::<u64>() {
                        Ok(part_number) => {
                            write::upload_part(&env, req, &key, &upload_id, part_number).await
                        }
                        Err(_) => error::problem_response(ClientError::PartNumberInvalid),
                    }
                }
                _ => error::problem_response(ClientError::PartNumberInvalid),
            }
        }
        Method::Post => {
            // why: these answer through the counter rather than around
            // it. Returning here left `begin` and `complete` as kinds the
            // router names and no point ever carries, so a multipart push
            // showed up in the counters as parts with no upload around
            // them.
            if let Some(upload_id) = query_value(&req, "uploadId") {
                write::complete_multipart(&env, req, &key, &upload_id).await
            } else if query_value(&req, "uploads").is_some() {
                write::create_multipart(&env, &key, now, &mut req).await
            } else {
                error::problem_response(ClientError::MalformedKey)
            }
        }
        Method::Delete => {
            let Some(upload_id) = query_value(&req, "uploadId") else {
                return error::problem_response(ClientError::UploadUnknown);
            };
            write::abort_multipart(&env, &key, &upload_id).await
        }
        _ => error::problem_response(ClientError::NotFound),
    };
    count_write(&env, kind, bytes, &caller, answered)
}

/// What a write was against, for the statistic. Read off the route
/// rather than the handler, because this is the one place that knows
/// which branch the request took before it takes it.
fn write_kind(req: &Request, method: &Method) -> cachet_core::stats::StatKind {
    use cachet_core::stats::StatKind;
    match method {
        Method::Put if query_value(req, "partNumber").is_some() => StatKind::Part,
        Method::Put => StatKind::Nar,
        Method::Post if query_value(req, "uploadId").is_some() => StatKind::Complete,
        Method::Post => StatKind::Begin,
        Method::Delete => StatKind::Abort,
        _ => StatKind::Unknown,
    }
}

/// How many bytes this write put on the wire.
///
/// Read from `content-length` before the handler consumes the body,
/// which is the one place the number is available without measuring the
/// stream a second time. It is the compressed size R2 gains rather than
/// the store path's decompressed size: what the bucket grew by, which is
/// what "pushed this week" means. A request with no body, which is every
/// multipart open, completion, and abort, contributes nothing.
fn uploaded_bytes(req: &Request) -> u64 {
    req.headers()
        .get("content-length")
        .ok()
        .flatten()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

/// The caller dimensions a write carries: which run it came from.
fn write_caller(
    authorized: &cachet_core::error::Result<cachet_core::auth::OidcIdentity>,
) -> cachet_core::stats::StatCaller {
    authorized.as_ref().map_or_else(
        |_| cachet_core::stats::StatCaller::anonymous(),
        |identity| cachet_core::stats::StatCaller::ci(&identity.repository, &identity.ref_),
    )
}

/// Count one write, and answer exactly what the handler answered.
///
/// The outcome is the status rather than the error code, because the
/// code lives in a body this layer would have to consume to read, and a
/// rejection rate answers the question either way. A refusal counts: a
/// deployment refusing every push is the thing an operator most wants a
/// number for.
fn count_write(
    env: &Env,
    kind: cachet_core::stats::StatKind,
    bytes: u64,
    caller: &cachet_core::stats::StatCaller,
    answered: Result<Response>,
) -> Result<Response> {
    use cachet_core::stats::{StatEvent, StatOutcome, StatPoint};
    let Ok(response) = answered else {
        return answered;
    };
    let status = response.status_code();
    let outcome = if (200..300).contains(&status) {
        StatOutcome::Stored
    } else {
        StatOutcome::Status(status)
    };
    // why: a refusal's bytes are counted too. A push that uploads a
    // gigabyte and is refused cost the deployment that gigabyte, and an
    // operator reading a bandwidth number wants the bytes that arrived
    // rather than the bytes that were kept.
    stats::emit(
        env,
        &StatPoint::new(StatEvent::Write, kind, outcome)
            .by(caller)
            .measuring(1, bytes),
    );
    Ok(response)
}

/// One query parameter, if present.
fn query_value(req: &Request, name: &str) -> Option<String> {
    req.url()
        .ok()?
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

/// The cache-object GET branches: NAR and narinfo keys in one shape. The
/// credential and the generation join here (independent fetches, so a miss
/// pays one colo hop instead of two), and the HEAD arm stays free of
/// generation work the edge cache never serves.
async fn read_routes(
    env: &Env,
    ctx: &Context,
    now: UnixMillis,
    req: &Request,
    method: &Method,
    path: &str,
) -> Option<Result<Response>> {
    let (bucket_key, kind) = if path.starts_with("/nar/") {
        match parse_nar_request_path(path) {
            Ok(key) => (key.as_str().to_string(), ObjectKind::Nar),
            Err(code) => return Some(error::problem_response(code)),
        }
    } else if path.ends_with(NARINFO_KEY_SUFFIX) {
        match parse_narinfo_request_path(path) {
            Ok(hash) => (format!("{hash}{NARINFO_KEY_SUFFIX}"), ObjectKind::Narinfo),
            Err(code) => return Some(error::problem_response(code)),
        }
    } else {
        return None;
    };
    let (authorized, generation) = match method {
        // why: a HEAD never resolves the generation (the edge cache
        // answers GETs, never HEADs), so nothing joins.
        Method::Head => (verdict::authorize_read(env, now, req).await, Ok(None)),
        // why: the generation fetch is independent of the credential;
        // joining pays one colo hop instead of two on a miss chain, and a
        // denied request racing a generation lookup leaks nothing.
        _ => {
            futures_util::future::join(
                verdict::authorize_read(env, now, req),
                read::resolve_generation(env, ctx, now),
            )
            .await
        }
    };
    // why: the actor is read off the credential that already resolved,
    // so counting costs nothing and cannot disagree with the decision.
    let caller = match &authorized {
        Ok(identity) => stat_caller(identity),
        Err(_) => cachet_core::stats::StatCaller::anonymous(),
    };
    let authorized = match authorized {
        // why: a browser session is see-only. It authenticates the
        // console's own surface and stops at the cache's contents, so a
        // cookie copied out of a browser cannot substitute. The refusal
        // is the anonymous one, because naming the reason would tell a
        // caller which credential class it holds.
        Ok(identity) if !identity.reads_cache_objects() => Err(ClientError::Unauthorized),
        other => other,
    };
    if let Err(code) = authorized {
        return Some(error::problem_response(code));
    }
    let generation = match generation {
        Ok(generation) => generation,
        Err(failure) => return Some(Err(failure)),
    };
    Some(match method {
        Method::Head => read::head_object(env, &bucket_key, kind).await,
        _ => read::serve_object(env, ctx, path, &bucket_key, kind, generation, &caller).await,
    })
}

/// Which caller class a resolved read identity belongs to.
///
/// The identity already knows: the read path told the three credential
/// shapes apart to resolve it, and carries which one won. Re-deriving it
/// from a login would be guesswork, and wrong for a workflow run, whose
/// login is its repository owner rather than a person.
fn stat_caller(identity: &verdict::ReadIdentity) -> cachet_core::stats::StatCaller {
    match identity {
        verdict::ReadIdentity::Session { .. } => cachet_core::stats::StatCaller::browser(),
        verdict::ReadIdentity::Token { .. } => cachet_core::stats::StatCaller::laptop(),
        verdict::ReadIdentity::Ci {
            repository,
            reference,
            ..
        } => cachet_core::stats::StatCaller::ci(repository, reference),
    }
}

/// The write-path credential against the request's Authorization header.
/// The cron entry point: the armed collector. Failures land in the event
/// log; a scheduled tick throwing is how workerd marks invocations failed.
#[event(scheduled)]
async fn scheduled(_event: worker::ScheduledEvent, env: Env, _ctx: worker::ScheduleContext) {
    if let Err(failure) = gc::drive(&env).await {
        log::event("error", "gc.run_failed", &[("error", failure.to_string())]);
    }
}

async fn authorize_write(
    env: &Env,
    now: UnixMillis,
    req: &Request,
) -> cachet_core::error::Result<cachet_core::auth::OidcIdentity> {
    let header = req
        .headers()
        .get("authorization")
        .map_err(|_| ClientError::MalformedAuth)?;
    auth::authorize_write(env, now, header.as_deref()).await
}
