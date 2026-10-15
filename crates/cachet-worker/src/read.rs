//! Serving cache objects: the edge cache, the bucket, and a cacheable
//! miss, in exactly that order. The decisions (headers, cacheability, the
//! miss's shape, the key derivations) are cachet-core's; this module
//! performs the I/O.
//!
//! A narinfo is answered only beside a NAR the deployment can serve. Nix
//! fetches the narinfo first and the NAR second, and a narinfo whose NAR
//! then answers 404 fails the substitution outright, where a narinfo that
//! answers 404 sends nix to build the one missing path and substitute the
//! rest (ADR 0023).

use cachet_core::constants::{GENERATION_OBJECT_KEY, NIX_CACHE_INFO};
use cachet_core::error::ClientError;
use cachet_core::generation::{
    GenerationDocument, generation_cache_key, miss_cache_key, object_cache_key,
};
use cachet_core::keys::NarKey;
use cachet_core::narinfo::Narinfo;
use cachet_core::read::{
    ByteRange, NOT_FOUND_BODY, ObjectKind, cache_info_response_headers,
    generation_response_headers, is_edge_cacheable, missing_answer_holds,
    not_found_response_headers, object_response_headers, partial_response_headers, resolve_range,
    unsatisfiable_range_header,
};
use cachet_core::stats::{StatCaller, StatEvent, StatKind, StatOutcome, StatPoint};
use cachet_core::types::UnixMillis;
use worker::{Bucket, Cache, Context, Env, Headers, Response, Result};

use crate::{error, log};

/// The R2 binding the whole cache lives in.
const BUCKET_BINDING: &str = cachet_deploy::roster::names::CACHE_BUCKET;

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

/// Answer a HEAD for a NAR from the bucket's metadata alone. A HEAD is
/// never edge-cached: nix uses GET for the queries that matter, so HEAD
/// traffic is rare and a second cache-key space for it buys nothing.
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

/// Serve one NAR: edge cache, then bucket, then a cacheable miss. The
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
    caller: &StatCaller,
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
        count_read(env, kind, StatOutcome::EdgeHit, caller, 0);
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
        count_read(env, kind, StatOutcome::Miss, caller, 0);
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
    count_read(env, kind, StatOutcome::BucketHit, caller, size);
    Ok(response)
}

/// Serve part of one NAR: the answer to a GET carrying a Range header.
///
/// A client asks for a range when a download broke partway and it is
/// resuming, which is the case this exists for: nix fetches through curl,
/// and curl fails a resumed transfer outright when the answer is the
/// whole object again, so a dropped connection on a large NAR used to
/// fail the substitution for good.
///
/// The edge cache is not consulted. It stores whole objects, a ranged
/// read is rare, and the bucket serves a range natively, so this reads
/// the object's size, decides the range against it in cachet-core, and
/// asks the bucket for exactly those bytes. A range that covers none of
/// the object answers 416 with the object's size.
pub async fn serve_range(
    env: &Env,
    bucket_key: &str,
    kind: ObjectKind,
    range: ByteRange,
    caller: &StatCaller,
) -> Result<Response> {
    let bucket = env.bucket(BUCKET_BINDING)?;
    let Some(head) = bucket.head(bucket_key).await? else {
        log::event("info", "read.miss", &[("kind", kind.name().to_string())]);
        count_read(env, kind, StatOutcome::Miss, caller, 0);
        return miss_response(true);
    };
    let size = head.size();
    let Some((first, last)) = resolve_range(range, size) else {
        return range_unsatisfiable(env, kind, size, caller);
    };
    // why the resolved offsets and never the client's: they lie inside
    // the object, so the bucket is never asked for bytes that are not
    // there, and every number handed to it is one the object's own size
    // bounds.
    let length = last - first + 1;
    let Some(object) = bucket
        .get(bucket_key)
        .range(worker::Range::OffsetWithLength {
            offset: first,
            length,
        })
        .execute()
        .await?
    else {
        // Deleted between the two reads: a miss like any other.
        log::event("info", "read.miss", &[("kind", kind.name().to_string())]);
        count_read(env, kind, StatOutcome::Miss, caller, 0);
        return miss_response(true);
    };
    let Some(body) = object.body() else {
        log::alert("read.object_bodiless");
        return error::problem_response(ClientError::StorageUnavailable);
    };
    let response = apply(
        Response::from_body(body.response_body()?)?.with_status(206),
        &partial_response_headers(kind, first, last, size),
    )?;
    log::event(
        "info",
        "read.bucket_hit",
        &[
            ("kind", kind.name().to_string()),
            ("range", format!("{first}-{last}/{size}")),
        ],
    );
    count_read(env, kind, StatOutcome::BucketHit, caller, length);
    Ok(response)
}

/// The 416 a range that covers none of the object earns, with the
/// object's size beside its problem body.
fn range_unsatisfiable(
    env: &Env,
    kind: ObjectKind,
    size: u64,
    caller: &StatCaller,
) -> Result<Response> {
    log::event(
        "info",
        "read.range_unsatisfiable",
        &[("kind", kind.name().to_string())],
    );
    count_read(env, kind, StatOutcome::Status(416), caller, 0);
    let mut response = error::object_read_problem(ClientError::RangeNotSatisfiable)?;
    let (name, value) = unsatisfiable_range_header(size);
    response.headers_mut().set(name, &value)?;
    Ok(response)
}

/// What the bucket holds under one narinfo key, judged against the NAR the
/// narinfo names.
enum StoredNarinfo {
    /// No narinfo under the key.
    Absent,
    /// A narinfo whose NAR the bucket holds, as its text. A narinfo that
    /// will not parse names no NAR anyone can read, and it serves as it
    /// always has, because nix refuses it on its own.
    Servable(String),
    /// A narinfo whose NAR the bucket does not hold. The read deleted it.
    Dangling,
    /// An object with no body: a storage anomaly, never a miss.
    Bodiless,
}

/// What this colo's edge cache holds for one narinfo.
enum EdgeNarinfo {
    /// No entry.
    Absent,
    /// A narinfo whose NAR this colo can serve, as its text.
    Servable(String),
    /// A narinfo whose NAR neither this colo nor the bucket holds. The entry
    /// is gone from this colo, and the NAR it named rides along so the
    /// bucket read does not ask about the same NAR twice.
    Stale(NarKey),
}

/// Whether the bucket holds a NAR.
async fn nar_in_bucket(bucket: &Bucket, nar: &NarKey) -> Result<bool> {
    Ok(bucket.head(nar.as_str()).await?.is_some())
}

/// Whether this colo can serve a NAR: from its own edge cache, where a NAR
/// read put it, or from the bucket.
async fn nar_servable(bucket: &Bucket, nar: &NarKey) -> Result<bool> {
    // why the edge first: a warm read finds both the narinfo and its NAR in
    // the colo that answers it, and asking the bucket on every one of those
    // would put a round trip to the bucket's region on the path the edge
    // cache exists to keep free of one. An entry here serves the NAR
    // whether or not the bucket still holds it, because a NAR's bytes are
    // named by their own hash and cannot be wrong.
    let edge_key = object_cache_key(nar.as_str());
    if Cache::default().get(edge_key, true).await?.is_some() {
        return Ok(true);
    }
    nar_in_bucket(bucket, nar).await
}

/// Read one narinfo out of the bucket, and keep it only beside its NAR.
///
/// A narinfo the bucket holds without its NAR is deleted on the spot. It
/// describes a path no client can substitute, and the push's probe reads
/// presence from the narinfos alone, so while it stays, every push skips
/// the path as held and the cache never gets it back. Deleted, it reads
/// as absent, and the job that just had to build the path uploads it
/// again. The check asks the bucket, because the deletion repairs the
/// bucket, and an edge entry for the NAR says nothing about what the
/// bucket holds. `known_missing` is a NAR this request has already found
/// missing from the bucket, with the instant the request began.
async fn stored_narinfo(
    bucket: &Bucket,
    bucket_key: &str,
    known_missing: Option<(&NarKey, UnixMillis)>,
) -> Result<StoredNarinfo> {
    let Some(object) = bucket.get(bucket_key).execute().await? else {
        return Ok(StoredNarinfo::Absent);
    };
    let stored_at = UnixMillis::new(object.uploaded().as_millis());
    let Some(body) = object.body() else {
        return Ok(StoredNarinfo::Bodiless);
    };
    let text = body.text().await?;
    let Ok(document) = Narinfo::parse(&text) else {
        log::alert("read.narinfo_unparseable");
        return Ok(StoredNarinfo::Servable(text));
    };
    // why the head is made again for a narinfo stored since the request
    // began: a push stores the NAR before the narinfo, so that narinfo can
    // name a NAR the push stored after this request's first head.
    let already_missing = known_missing
        .is_some_and(|(nar, started)| missing_answer_holds(nar, &document.url, stored_at, started));
    if !already_missing && nar_in_bucket(bucket, &document.url).await? {
        return Ok(StoredNarinfo::Servable(text));
    }
    // why best effort: the answer is a 404 either way, because the NAR is
    // gone, and a deletion that fails leaves the next read to try again.
    if let Err(failure) = bucket.delete(bucket_key).await {
        log::event(
            "error",
            "read.narinfo_delete_failed",
            &[("error", failure.to_string())],
        );
    }
    log::event(
        "warn",
        "read.narinfo_nar_missing",
        &[
            ("source", "bucket".to_string()),
            ("nar", document.url.as_str().to_string()),
        ],
    );
    Ok(StoredNarinfo::Dangling)
}

/// Read one narinfo out of this colo's edge cache, and keep it only beside
/// a NAR this colo can serve.
///
/// An entry outlives the collector's sweep of the path it describes, for
/// up to its thirty-day lifetime (ADR 0013). An entry whose NAR is gone is
/// removed from this colo, which is the only colo the Cache API reaches,
/// and the read goes on to the bucket, which may hold the path again under
/// a NAR a later push stored.
async fn edge_narinfo(bucket: &Bucket, object_key: &str) -> Result<EdgeNarinfo> {
    let cache = Cache::default();
    let Some(mut cached) = cache.get(object_key, true).await? else {
        return Ok(EdgeNarinfo::Absent);
    };
    let text = cached.text().await?;
    let Ok(document) = Narinfo::parse(&text) else {
        return Ok(EdgeNarinfo::Servable(text));
    };
    if nar_servable(bucket, &document.url).await? {
        return Ok(EdgeNarinfo::Servable(text));
    }
    let _ = cache.delete(object_key, true).await;
    log::event(
        "warn",
        "read.narinfo_nar_missing",
        &[
            ("source", "edge".to_string()),
            ("nar", document.url.as_str().to_string()),
        ],
    );
    Ok(EdgeNarinfo::Stale(document.url))
}

/// A narinfo answer from its text, with the headers a stored object
/// carries.
fn narinfo_response(text: String) -> Result<Response> {
    let size = text.len() as u64;
    apply(
        Response::ok(text)?,
        &object_response_headers(ObjectKind::Narinfo, size),
    )
}

/// Serve one narinfo: this colo's edge entry when this colo can serve its
/// NAR, then a cached miss, then the bucket's copy when the bucket holds
/// its NAR, then a miss. A narinfo is small and the check needs its `URL`
/// line, so it is read whole rather than streamed.
pub async fn serve_narinfo(
    env: &Env,
    ctx: &Context,
    now: UnixMillis,
    request_path: &str,
    bucket_key: &str,
    generation: Option<u64>,
    caller: &StatCaller,
) -> Result<Response> {
    let kind = ObjectKind::Narinfo;
    let bucket = env.bucket(BUCKET_BINDING)?;
    let object_key = object_cache_key(request_path);
    let known_missing = match edge_narinfo(&bucket, &object_key).await? {
        EdgeNarinfo::Servable(text) => {
            log::event(
                "info",
                "read.edge_hit",
                &[("kind", kind.name().to_string())],
            );
            count_read(env, kind, StatOutcome::EdgeHit, caller, 0);
            return narinfo_response(text);
        }
        EdgeNarinfo::Stale(nar) => Some(nar),
        EdgeNarinfo::Absent => None,
    };
    let known_missing = known_missing.as_ref().map(|nar| (nar, now));
    let miss_key = generation.map(|generation| miss_cache_key(generation, request_path));
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

    match stored_narinfo(&bucket, bucket_key, known_missing).await? {
        StoredNarinfo::Servable(text) => {
            let size = text.len() as u64;
            let mut response = narinfo_response(text)?;
            let cached = response.cloned()?;
            ctx.wait_until(async move {
                let _ = Cache::default().put(object_key, cached).await;
            });
            log::event(
                "info",
                "read.bucket_hit",
                &[("kind", kind.name().to_string())],
            );
            count_read(env, kind, StatOutcome::BucketHit, caller, size);
            Ok(response)
        }
        StoredNarinfo::Absent => {
            log::event("info", "read.miss", &[("kind", kind.name().to_string())]);
            count_read(env, kind, StatOutcome::Miss, caller, 0);
            let mut missing = miss_response(true)?;
            if let Some(miss_key) = miss_key {
                let cached = missing.cloned()?;
                ctx.wait_until(async move {
                    let _ = Cache::default().put(miss_key, cached).await;
                });
            }
            Ok(missing)
        }
        // why this miss is not cached: the read just deleted the narinfo,
        // so the next read finds it absent and caches that miss itself,
        // and a push that repairs the path inside the miss window serves
        // at once instead of after it.
        StoredNarinfo::Dangling => {
            log::event("info", "read.miss", &[("kind", kind.name().to_string())]);
            count_read(env, kind, StatOutcome::Miss, caller, 0);
            miss_response(true)
        }
        StoredNarinfo::Bodiless => {
            log::alert("read.object_bodiless");
            error::problem_response(ClientError::StorageUnavailable)
        }
    }
}

/// Answer a HEAD for a narinfo from the bucket's copy, kept only beside its
/// NAR. The edge cache is not consulted, as for every HEAD.
pub async fn head_narinfo(env: &Env, bucket_key: &str) -> Result<Response> {
    let bucket = env.bucket(BUCKET_BINDING)?;
    match stored_narinfo(&bucket, bucket_key, None).await? {
        StoredNarinfo::Servable(text) => apply(
            Response::empty()?,
            &object_response_headers(ObjectKind::Narinfo, text.len() as u64),
        ),
        StoredNarinfo::Absent | StoredNarinfo::Dangling => miss_response(false),
        StoredNarinfo::Bodiless => {
            log::alert("read.object_bodiless");
            error::problem_response(ClientError::StorageUnavailable)
        }
    }
}

/// Serve part of a narinfo: the bucket's copy, kept only beside its NAR,
/// cut to the range. The document is already in hand for the check, so
/// the range is cut from it rather than asked of the bucket again.
pub async fn serve_narinfo_range(
    env: &Env,
    bucket_key: &str,
    range: ByteRange,
    caller: &StatCaller,
) -> Result<Response> {
    let kind = ObjectKind::Narinfo;
    let bucket = env.bucket(BUCKET_BINDING)?;
    let bytes = match stored_narinfo(&bucket, bucket_key, None).await? {
        StoredNarinfo::Servable(text) => text.into_bytes(),
        StoredNarinfo::Absent | StoredNarinfo::Dangling => {
            log::event("info", "read.miss", &[("kind", kind.name().to_string())]);
            count_read(env, kind, StatOutcome::Miss, caller, 0);
            return miss_response(true);
        }
        StoredNarinfo::Bodiless => {
            log::alert("read.object_bodiless");
            return error::problem_response(ClientError::StorageUnavailable);
        }
    };
    let size = bytes.len() as u64;
    let Some((first, last)) = resolve_range(range, size) else {
        return range_unsatisfiable(env, kind, size, caller);
    };
    // why get rather than indexing: the offsets lie inside the document by
    // resolve_range's own contract, and a slice that is not there answers
    // a storage anomaly instead of trapping the isolate.
    let part = usize::try_from(first)
        .ok()
        .zip(usize::try_from(last).ok())
        .and_then(|(first, last)| bytes.get(first..=last));
    let Some(part) = part else {
        log::alert("read.range_outside_document");
        return error::problem_response(ClientError::StorageUnavailable);
    };
    let length = last - first + 1;
    let response = apply(
        Response::from_bytes(part.to_vec())?.with_status(206),
        &partial_response_headers(kind, first, last, size),
    )?;
    log::event(
        "info",
        "read.bucket_hit",
        &[
            ("kind", kind.name().to_string()),
            ("range", format!("{first}-{last}/{size}")),
        ],
    );
    count_read(env, kind, StatOutcome::BucketHit, caller, length);
    Ok(response)
}

/// Count one served object, with who asked for it.
///
/// The caller is the dimension that separates a cache CI hammers from
/// one people substitute from at their desks, and for a workflow run it
/// carries the repository too, so a hit rate can be read per repo.
fn count_read(
    env: &Env,
    kind: ObjectKind,
    outcome: StatOutcome,
    caller: &StatCaller,
    size_bytes: u64,
) {
    crate::stats::emit(
        env,
        &StatPoint::new(StatEvent::Read, StatKind::from(kind), outcome)
            .by(caller)
            .measuring(1, size_bytes),
    );
}
