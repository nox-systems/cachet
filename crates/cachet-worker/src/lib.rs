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
mod read;
mod roots;
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
        if path.starts_with("/nar/") {
            let key = match parse_nar_request_path(&path) {
                Ok(key) => key,
                Err(code) => return error::problem_response(code),
            };
            let authorized = verdict::authorize_read(&env, now, &req).await;
            if let Err(code) = authorized {
                return error::problem_response(code);
            }
            return match method {
                Method::Head => read::head_object(&env, key.as_str(), ObjectKind::Nar).await,
                _ => read::serve_object(&env, &ctx, &path, key.as_str(), ObjectKind::Nar).await,
            };
        }
        if path.ends_with(NARINFO_KEY_SUFFIX) {
            let hash = match parse_narinfo_request_path(&path) {
                Ok(hash) => hash,
                Err(code) => return error::problem_response(code),
            };
            let authorized = verdict::authorize_read(&env, now, &req).await;
            if let Err(code) = authorized {
                return error::problem_response(code);
            }
            let bucket_key = format!("{hash}{NARINFO_KEY_SUFFIX}");
            return match method {
                Method::Head => read::head_object(&env, &bucket_key, ObjectKind::Narinfo).await,
                _ => read::serve_object(&env, &ctx, &path, &bucket_key, ObjectKind::Narinfo).await,
            };
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

    if path.ends_with(NARINFO_KEY_SUFFIX) && method == Method::Put {
        let hash = match parse_narinfo_request_path(&path) {
            Ok(hash) => hash,
            Err(code) => return error::problem_response(code),
        };
        let authorized = authorize_write(&env, now, &req).await;
        if let Err(code) = authorized {
            return error::problem_response(code);
        }
        return write::put_narinfo(&env, req, &hash).await;
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
    if path == "/api/openapi.json" {
        return Some(api::openapi_document());
    }
    if path == "/api/self/gc-runs" {
        return Some(api::gc_runs_list(env, now, req).await);
    }
    if let Some(run_id) = path.strip_prefix("/api/self/gc-runs/") {
        return Some(api::gc_run_read(env, now, req, run_id).await);
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
    if let Err(code) = authorized {
        return error::problem_response(code);
    }
    match method {
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
            if let Some(upload_id) = query_value(&req, "uploadId") {
                return write::complete_multipart(&env, req, &key, &upload_id).await;
            }
            if query_value(&req, "uploads").is_some() {
                return write::create_multipart(&env, &key, now, &mut req).await;
            }
            error::problem_response(ClientError::MalformedKey)
        }
        Method::Delete => {
            let Some(upload_id) = query_value(&req, "uploadId") else {
                return error::problem_response(ClientError::UploadUnknown);
            };
            write::abort_multipart(&env, &key, &upload_id).await
        }
        _ => error::problem_response(ClientError::NotFound),
    }
}

/// One query parameter, if present.
fn query_value(req: &Request, name: &str) -> Option<String> {
    req.url()
        .ok()?
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
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
