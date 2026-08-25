//! Serving cache objects: the edge cache, the bucket, and a cacheable
//! miss, in exactly that order. The decisions (headers, cacheability, the
//! miss's shape, the key derivations) are cachet-core's; this module
//! performs the I/O.

use cachet_core::constants::{GENERATION_OBJECT_KEY, NIX_CACHE_INFO};
use cachet_core::error::ClientError;
use cachet_core::generation::{
    GenerationDocument, generation_cache_key, miss_cache_key, object_cache_key,
};
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

// The isolate's own generation belief: the memo reads where legality
// lives, and the value (gen, expiry) is Copy so no borrow crosses an
// await.
thread_local! {
    static GENERATION_MEMO: std::cell::RefCell<Option<(u64, u64)>> =
        const { std::cell::RefCell::new(None) };
}

/// why: strictly inside the generation entry's own 60-second edge TTL,
/// so a stale isolate belief can resurrect old-generation edge keys for
/// at most ~90 seconds after a sweep bump. Read-time staleness only,
/// inside the sweep's documented convergence window, never a write-path
/// decision.
const GENERATION_MEMO_TTL_MS: u64 = 30_000;

/// Resolve the current edge-caching epoch: isolate memo first, then the
/// edge cache (the common case costs a near-free lookup), falling back to
/// the bucket, its own short TTL bounding stale belief.
///
/// Returns `None` when the generation cannot be established, and the
/// caller then bypasses the edge cache rather than assuming zero. Reusing
/// zero would resurrect entries written before the first sweep: the exact
/// staleness the generation exists to prevent.
pub(crate) async fn resolve_generation(
    env: &Env,
    ctx: &Context,
    now: cachet_core::types::UnixMillis,
) -> Result<Option<u64>> {
    if let Some((generation, expires_at_ms)) = GENERATION_MEMO.with(|memo| *memo.borrow()) {
        if now.as_u64() < expires_at_ms {
            return Ok(Some(generation));
        }
    }
    let cache_key = generation_cache_key();
    if let Some(mut cached) = Cache::default().get(&cache_key, true).await? {
        let text = cached.text().await?;
        let Ok(document) = GenerationDocument::parse(&text) else {
            log::alert("generation.cached_document_corrupt");
            return Ok(None);
        };
        GENERATION_MEMO.with(|memo| {
            *memo.borrow_mut() = Some((document.generation, now.as_u64() + GENERATION_MEMO_TTL_MS));
        });
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
    GENERATION_MEMO.with(|memo| {
        *memo.borrow_mut() = Some((document.generation, now.as_u64() + GENERATION_MEMO_TTL_MS));
    });
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
/// and NARs are large. The generation arrives resolved: the caller joins
/// it against the credential check so a miss pays one colo hop instead of
/// two.
pub async fn serve_object(
    env: &Env,
    ctx: &Context,
    request_path: &str,
    bucket_key: &str,
    kind: ObjectKind,
    generation: Option<u64>,
) -> Result<Response> {
    // Two key spaces, looked up in the order they pay off. A stored
    // object's entry outlives sweeps, so the common warm read answers on
    // the first lookup; a cached absence is generation-scoped and short,
    // and only a request the cache has no object for reaches it.
    let object_key = object_cache_key(request_path);
    let miss_key = generation.map(|generation| miss_cache_key(generation, request_path));

    if let Some(hit) = Cache::default().get(&object_key, true).await? {
        log::event(
            "info",
            "read.edge_hit",
            &[("kind", kind.name().to_string())],
        );
        return Ok(hit);
    }
    if let Some(miss_key) = &miss_key {
        if let Some(hit) = Cache::default().get(miss_key, true).await? {
            log::event(
                "info",
                "read.edge_hit",
                &[
                    ("kind", kind.name().to_string()),
                    ("answer", "miss".to_string()),
                ],
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
        if let Some(miss_key) = miss_key {
            let cached = missing.cloned()?;
            ctx.wait_until(async move {
                let _ = Cache::default().put(miss_key, cached).await;
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
        let cached = response.cloned()?;
        ctx.wait_until(async move {
            let _ = Cache::default().put(object_key, cached).await;
        });
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
