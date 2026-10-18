//! The bulk presence probe: `POST /api/probe`. A push run asks one
//! authorized question about its whole candidate set instead of pricing
//! narinfo HEADs one round trip apiece (the per-request cost of a HEAD is
//! a verdict, a generation read, and a bucket head; multiplied by a
//! closure that is the wrong protocol shape — cachix and flakehub answer
//! the same question in one request, and so does this route).
//!
//! The answer is read from the bucket, never from a secondary index: a KV
//! copy of presence would need a second writer on every narinfo PUT and
//! every GC sweep, and a crash between the pair drifts the index. Drift
//! toward "present" would tell a pusher to skip an object clients then
//! 404 on — the bad direction, for 100ms saved.
//!
//! How the bucket is asked follows the question's size (ADR 0024,
//! `cachet_core::probe::plan`). A small question heads each narinfo key it
//! names, so it costs the same however much the bucket holds. A large one
//! lists the narinfos under each first character the question uses, those
//! listings running side by side. The probe used to list every narinfo in
//! the bucket one page after another for every question, which cost a
//! no-op push 10 to 13 seconds once production held 17,398 narinfos.
//!
//! Holding a listing between probes was tried and removed. A memo can
//! only ever be a subset of what the bucket holds, so its drift is in the
//! safe direction, but the thing it drifts away from is exactly the dedup
//! that makes a warm push cheap: a job whose sibling pushed a path a
//! moment ago reads it as absent and uploads it again.

use std::collections::BTreeSet;

use cachet_core::constants::{PROBE_BODY_BYTES_MAX, PUSH_PATHS_MAX};
use cachet_core::error::ClientError;
use cachet_core::probe::{ProbePlan, narinfo_hash, plan};
use cachet_core::types::{StorePathHash, UnixMillis};
use futures_util::StreamExt as _;
use worker::{Bucket, Env, Request, Response, Result};

use cachet_core::write::require_content_length;

use crate::{error, log, verdict};

/// How many bucket reads one probe keeps waiting at once.
///
/// why: six. A Worker invocation can have six connections waiting for
/// response headers, R2's `head` and `list` among them, and a seventh
/// queues until one of them answers (the Workers limits page). A wider
/// window would only queue.
const BUCKET_READS_IN_FLIGHT: usize = 6;

/// Answer the probe: authorize, bound, parse, read the bucket, intersect.
/// The response is JSON, no-store: presence moves with every write.
pub async fn answer_probe(env: &Env, now: UnixMillis, mut req: Request) -> Result<Response> {
    let authorized = verdict::authorize_read(env, now, &req).await;
    if let Err(code) = authorized {
        return error::problem_response(code);
    }

    let length = require_content_length(
        req.headers().get("content-length")?.as_deref(),
        PROBE_BODY_BYTES_MAX,
    );
    if let Err(code) = length {
        return error::problem_response(code);
    }

    let Ok(body) = req.text().await else {
        return error::problem_response(ClientError::MalformedProbe);
    };
    let Ok(probe) = serde_json::from_str::<cachet_api::ProbeBody>(&body) else {
        return error::problem_response(ClientError::MalformedProbe);
    };
    if probe.paths.len() > usize::try_from(PUSH_PATHS_MAX).expect("the cap fits usize") {
        return error::problem_response(ClientError::MalformedProbe);
    }
    let mut asked: BTreeSet<StorePathHash> = BTreeSet::new();
    for path in &probe.paths {
        match StorePathHash::parse(path) {
            Ok(hash) => {
                asked.insert(hash);
            }
            Err(_) => return error::problem_response(ClientError::MalformedProbe),
        }
    }

    let bucket = env.bucket("CACHE_BUCKET")?;
    let read = match held_narinfos(&bucket, &plan(&asked)).await {
        Ok(read) => read,
        Err(code) => return error::problem_response(code),
    };

    // why: from the asked set, which is ascending and unique, so the answer
    // is too, and a listing's other narinfos never reach it.
    let present: Vec<String> = asked
        .iter()
        .map(StorePathHash::as_str)
        .filter(|hash| read.held.contains(*hash))
        .map(str::to_string)
        .collect();
    log::event(
        "info",
        "probe.bulk",
        &[
            ("paths", asked.len().to_string()),
            ("present", present.len().to_string()),
            ("heads", read.heads.to_string()),
            ("pages", read.pages.to_string()),
        ],
    );

    // One point per push, so a UI can show how much a push actually had
    // to upload: asked against present is the cache's hit rate on the
    // write side. The event line above says what the answer cost.
    crate::stats::emit(
        env,
        &cachet_core::stats::StatPoint::new(
            cachet_core::stats::StatEvent::Probe,
            cachet_core::stats::StatKind::Probe,
            cachet_core::stats::StatOutcome::Answered,
        )
        .measuring(asked.len() as u64, present.len() as u64),
    );

    let body = serde_json::to_string(&cachet_api::ProbeAnswer { present })
        .map_err(|_| worker::Error::RustError("the probe answer serializes".to_string()))?;
    let headers = worker::Headers::new();
    headers.set("content-type", "application/json")?;
    headers.set("cache-control", "no-store")?;
    Ok(Response::ok(body)?.with_headers(headers))
}

/// What the bucket answered, and the reads it took.
struct HeldNarinfos {
    /// Hashes whose narinfo the bucket holds: each one asked about, for a
    /// plan of heads; everything under the listed prefixes, for a listing.
    held: BTreeSet<String>,
    /// Bucket heads made.
    heads: u64,
    /// Listing pages read.
    pages: u64,
}

/// Run one plan against the bucket. A read that fails fails the probe as
/// `storage_unavailable`, and the push then treats every candidate as
/// absent, the safe side of the error.
async fn held_narinfos(
    bucket: &Bucket,
    plan: &ProbePlan,
) -> std::result::Result<HeldNarinfos, ClientError> {
    match plan {
        ProbePlan::Heads(keys) => held_by_head(bucket, keys).await,
        ProbePlan::Listing(prefixes) => held_by_listing(bucket, prefixes).await,
    }
}

/// Head each narinfo key, six at a time.
async fn held_by_head(
    bucket: &Bucket,
    keys: &[String],
) -> std::result::Result<HeldNarinfos, ClientError> {
    let mut answers = futures_util::stream::iter(keys)
        .map(|key| async move { (key, bucket.head(key.as_str()).await) })
        .buffer_unordered(BUCKET_READS_IN_FLIGHT);
    let mut held = BTreeSet::new();
    while let Some((key, answer)) = answers.next().await {
        match answer {
            Ok(Some(_)) => {
                if let Some(hash) = narinfo_hash(key) {
                    held.insert(hash.to_string());
                }
            }
            Ok(None) => {}
            Err(failure) => {
                log::event(
                    "error",
                    "probe.head_failed",
                    &[("error", failure.to_string())],
                );
                return Err(ClientError::StorageUnavailable);
            }
        }
    }
    Ok(HeldNarinfos {
        held,
        heads: keys.len() as u64,
        pages: 0,
    })
}

/// List the narinfos under each prefix, six prefixes at a time.
async fn held_by_listing(
    bucket: &Bucket,
    prefixes: &[String],
) -> std::result::Result<HeldNarinfos, ClientError> {
    let mut listings = futures_util::stream::iter(prefixes)
        .map(|prefix| list_prefix(bucket, prefix))
        .buffer_unordered(BUCKET_READS_IN_FLIGHT);
    let mut held = BTreeSet::new();
    let mut pages = 0;
    while let Some(listing) = listings.next().await {
        match listing {
            Ok((hashes, listed_pages)) => {
                held.extend(hashes);
                pages += listed_pages;
            }
            Err(failure) => {
                log::event("error", "probe.list_failed", &[("error", failure)]);
                return Err(ClientError::StorageUnavailable);
            }
        }
    }
    Ok(HeldNarinfos {
        held,
        heads: 0,
        pages,
    })
}

/// Every narinfo hash under one prefix, one page after another, and how
/// many pages that took.
async fn list_prefix(
    bucket: &Bucket,
    prefix: &str,
) -> std::result::Result<(Vec<String>, u64), String> {
    // why: the delimiter collapses `nar/` and the other directories into
    // common prefixes, so the listing prices root-level objects, which are
    // narinfos, rather than the NAR objects that would double every page.
    let mut hashes = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages: u64 = 0;
    loop {
        let mut builder = bucket.list().prefix(prefix).delimiter("/").limit(
            u32::try_from(cachet_core::constants::BUCKET_LIST_PAGE_LIMIT)
                .expect("the page limit fits u32"),
        );
        if let Some(cursor) = &cursor {
            builder = builder.cursor(cursor);
        }
        let listed = builder
            .execute()
            .await
            .map_err(|failure| failure.to_string())?;
        pages += 1;
        for object in listed.objects() {
            if let Some(hash) = narinfo_hash(&object.key()) {
                hashes.push(hash.to_string());
            }
        }
        if !listed.truncated() {
            return Ok((hashes, pages));
        }
        // why: a truncated page without a cursor would restart the listing
        // from its first page forever. R2 documents the cursor as present
        // whenever the page is truncated; a page that breaks that fails
        // the probe instead.
        let Some(next) = listed.cursor() else {
            return Err(format!(
                "the listing under {prefix} was truncated without a cursor"
            ));
        };
        cursor = Some(next);
    }
}
