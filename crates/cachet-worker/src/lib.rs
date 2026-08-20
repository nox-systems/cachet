//! cachet-worker is the deployable leaf (CLAUDE.md §4). It adapts the
//! Cloudflare request, environment, R2 bucket, KV namespace, Cache API, and
//! outbound GitHub calls into cachet-core decisions and renders results.
//! All ambient clock, entropy, and I/O live here behind the seams; the
//! signing key enters only as a Workers secret binding and is never logged
//! or returned.

#![forbid(unsafe_code)]

mod error;
mod log;
mod read;

use cachet_core::constants::NARINFO_KEY_SUFFIX;
use cachet_core::error::ClientError;
use cachet_core::keys::{parse_nar_request_path, parse_narinfo_request_path};
use cachet_core::read::ObjectKind;
use worker::{Context, Env, Method, Request, Response, Result, event};

/// The fetch entry point: key grammar and route shape first, then the
/// read path. Everything outside the two cache-object grammars and the
/// handshake is a 404; a path inside a grammar that fails its rules is a
/// 400, because the caller named something no deployment could ever hold.
#[event(fetch)]
async fn fetch(req: Request, env: Env, ctx: Context) -> Result<Response> {
    let method = req.method();
    if method != Method::Get && method != Method::Head {
        return error::problem_response(ClientError::NotFound);
    }
    let path = req.path();
    if path == "/nix-cache-info" {
        return read::serve_cache_info();
    }

    let parsed = if path.starts_with("/nar/") {
        parse_nar_request_path(&path).map(|key| (key.as_str().to_string(), ObjectKind::Nar))
    } else if path.ends_with(NARINFO_KEY_SUFFIX) {
        parse_narinfo_request_path(&path)
            .map(|hash| (format!("{hash}{NARINFO_KEY_SUFFIX}"), ObjectKind::Narinfo))
    } else {
        return error::problem_response(ClientError::NotFound);
    };
    let (bucket_key, kind) = match parsed {
        Ok(parsed) => parsed,
        Err(failure) => return error::problem_response(failure),
    };

    match method {
        Method::Head => read::head_object(&env, &bucket_key, kind).await,
        _ => read::serve_object(&env, &ctx, &path, &bucket_key, kind).await,
    }
}
