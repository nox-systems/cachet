//! Serving cache objects: the edge cache, the bucket, and a cacheable
//! miss, in exactly that order. The decisions (headers, cacheability, the
//! miss's shape, the key derivations) are cachet-core's; this module
//! performs the I/O.

use cachet_core::constants::{GENERATION_OBJECT_KEY, NIX_CACHE_INFO};
use cachet_core::error::ClientError;
use cachet_core::generation::{GenerationDocument, generation_cache_key, object_cache_key};
use cachet_core::read::{
    NOT_FOUND_BODY, ObjectKind, cache_info_response_headers, generation_response_headers,
    is_edge_cacheable, not_found_response_headers, object_response_headers,
};
use worker::{Cache, Context, Env, Headers, Response, Result};

use crate::{error, log};

/// The R2 binding the whole cache lives in.
const BUCKET_BINDING: &str = "CACHE_BUCKET";

/// Apply a pure header list to a response.
fn apply(response: Response, headers: &[(&'static str, String)]) -> Result<Response> {
    let out = Headers::new();
    for (name, value) in headers {
        out.set(name, value)?;
    }
    Ok(response.with_headers(out))
}

/// The miss answer: the plain-text body nix never reads on a GET, empty
/// on a HEAD, cacheable briefly in both cases.
fn miss_response(with_body: bool) -> Result<Response> {
    let base = if with_body {
        Response::ok(NOT_FOUND_BODY)?
    } else {
        Response::empty()?
    };
    apply(base.with_status(404), &not_found_response_headers())
}

/// Serve the handshake body, publicly, from the locked constant.
pub fn serve_cache_info() -> Result<Response> {
    apply(
        Response::ok(NIX_CACHE_INFO)?,
        &cache_info_response_headers(),
    )
}

/// Answer a HEAD from the bucket's metadata alone. A HEAD is never
/// edge-cached: nix uses GET for the queries that matter, so HEAD traffic
/// is rare and a second cache-key space for it buys nothing.
pub async fn head_object(env: &Env, bucket_key: &str, kind: ObjectKind) -> Result<Response> {
    let bucket = env.bucket(BUCKET_BINDING)?;
    match bucket.head(bucket_key).await? {
        None => miss_response(false),
        Some(head) => apply(
            Response::empty()?,
            &object_response_headers(kind, head.size()),
        ),
    }
}

/// Resolve the current edge-caching epoch: matched through the edge cache
/// (the common case costs a near-free lookup), falling back to the bucket,
/// its own short TTL bounding stale belief.
///
/// Returns `None` when the generation cannot be established, and the
/// caller then bypasses the edge cache rather than assuming zero. Reusing
/// zero would resurrect entries written before the first sweep: the exact
/// staleness the generation exists to prevent.
async fn resolve_generation(env: &Env, ctx: &Context) -> Result<Option<u64>> {
    let cache_key = generation_cache_key();
    if let Some(mut cached) = Cache::default().get(&cache_key, true).await? {
        let text = cached.text().await?;
        let Ok(document) = GenerationDocument::parse(&text) else {
            log::alert("generation.cached_document_corrupt");
            return Ok(None);
        };
        return Ok(Some(document.generation));
    }

    let bucket = env.bucket(BUCKET_BINDING)?;
    // An absent object means no sweep has ever run, so the generation is
    // zero. That zero is worth caching: a fresh bucket does not pay a
    // bucket read on every request.
    let text = match bucket.get(GENERATION_OBJECT_KEY).execute().await? {
        Some(object) => {
            let Some(body) = object.body() else {
                log::alert("generation.object_bodiless");
                return Ok(None);
            };
            body.text().await?
        }
        None => GenerationDocument::ZERO.serialize(),
    };
    let Ok(document) = GenerationDocument::parse(&text) else {
        log::alert("generation.document_corrupt");
        return Ok(None);
    };
    let cached = apply(
        Response::ok(document.serialize())?,
        &generation_response_headers(),
    )?;
    ctx.wait_until(async move {
        let _ = Cache::default().put(cache_key, cached).await;
    });
    Ok(Some(document.generation))
}

/// Serve one object: edge cache, then bucket, then a cacheable miss. The
/// body is never buffered: the bucket hands its stream to the runtime, and
/// the response streams without wasm CPU, because worker memory is small
/// and NARs are large.
pub async fn serve_object(
    env: &Env,
    ctx: &Context,
    request_path: &str,
    bucket_key: &str,
    kind: ObjectKind,
) -> Result<Response> {
    let generation = resolve_generation(env, ctx).await?;
    let cache_key = generation.map(|generation| object_cache_key(generation, request_path));

    if let Some(cache_key) = &cache_key {
        if let Some(hit) = Cache::default().get(cache_key, true).await? {
            log::event(
                "info",
                "read.edge_hit",
                &[("kind", kind.name().to_string())],
            );
            return Ok(hit);
        }
    }

    let bucket = env.bucket(BUCKET_BINDING)?;
    let Some(object) = bucket.get(bucket_key).execute().await? else {
        // An absent object is a 404 rather than a 5xx, because nix reads a
        // 404 as "this cache does not have it" and moves to the next
        // substituter, while a 5xx would make it retry us.
        log::event("info", "read.miss", &[("kind", kind.name().to_string())]);
        let mut missing = miss_response(true)?;
        if let Some(cache_key) = cache_key {
            let cached = missing.cloned()?;
            ctx.wait_until(async move {
                let _ = Cache::default().put(cache_key, cached).await;
            });
        }
        return Ok(missing);
    };

    let size = object.size();
    let Some(body) = object.body() else {
        // A GET whose object has no body is a storage anomaly, not a miss.
        log::alert("read.object_bodiless");
        return error::problem_response(ClientError::StorageUnavailable);
    };
    let mut response = apply(
        Response::from_body(body.response_body()?)?,
        &object_response_headers(kind, size),
    )?;
    if is_edge_cacheable(size) {
        if let Some(cache_key) = cache_key {
            let cached = response.cloned()?;
            ctx.wait_until(async move {
                let _ = Cache::default().put(cache_key, cached).await;
            });
        }
    } else {
        // The read succeeded and the object is only too large to cache, so
        // this line is a diagnostic rather than an error.
        log::event(
            "info",
            "read.too_large_to_cache",
            &[
                ("kind", kind.name().to_string()),
                ("sizeBytes", size.to_string()),
            ],
        );
    }
    log::event(
        "info",
        "read.bucket_hit",
        &[("kind", kind.name().to_string())],
    );
    Ok(response)
}
