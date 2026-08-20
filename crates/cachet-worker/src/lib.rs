//! cachet-worker is the deployable leaf (CLAUDE.md §4). It adapts the
//! Cloudflare request, environment, R2 bucket, KV namespace, Cache API, and
//! outbound GitHub calls into cachet-core decisions and renders results.
//! All ambient clock, entropy, and I/O live here behind the seams; the
//! signing key enters only as a Workers secret binding and is never logged
//! or returned.

#![forbid(unsafe_code)]

use cachet_core::constants::NIX_CACHE_INFO;
use worker::{Context, Env, Request, Response, Result, event};

/// The fetch entry point. The handshake route is public and serves the
/// exact wire body from cachet-core; every other path is a miss.
#[event(fetch)]
async fn fetch(req: Request, _env: Env, _ctx: Context) -> Result<Response> {
    if req.method() == worker::Method::Get && req.path() == "/nix-cache-info" {
        return Response::ok(NIX_CACHE_INFO);
    }
    Response::error("not found", 404)
}
