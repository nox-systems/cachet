//! The bulk presence probe: `POST /api/probe`. A push run asks one
//! authorized question about its whole candidate set instead of pricing
//! narinfo HEADs one round trip apiece (the per-request cost of a HEAD is
//! a verdict, a generation read, and a bucket head; multiplied by a
//! closure that is the wrong protocol shape — cachix and flakehub answer
//! the same question in one request, and so does this route).
//!
//! The answer is derived from a bucket enumeration, never from a
//! secondary index: a KV copy of presence would need a second writer on
//! every narinfo PUT and every GC sweep, and a crash between the pair
//! drifts the index. Drift toward "present" would tell a pusher to skip
//! an object clients then 404 on — the bad direction, for 100ms saved.
//! Enumerating the source of truth costs a handful of list pages on any
//! inventory GC's grace and leases permit, which is exactly the operation
//! the collector already pays for its own inventory.

use std::collections::BTreeSet;

use cachet_core::constants::{NARINFO_KEY_SUFFIX, PROBE_BODY_BYTES_MAX, PUSH_PATHS_MAX};
use cachet_core::error::ClientError;
use cachet_core::types::{StorePathHash, UnixMillis};
use worker::{Env, Request, Response, Result};

use cachet_core::write::require_content_length;

use crate::{error, log, verdict};

/// Answer the probe: authorize, bound, parse, enumerate, intersect.
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
    let mut asked: BTreeSet<String> = BTreeSet::new();
    for path in &probe.paths {
        match StorePathHash::parse(path) {
            Ok(hash) => {
                asked.insert(hash.as_str().to_string());
            }
            Err(_) => return error::problem_response(ClientError::MalformedProbe),
        }
    }

    let bucket = env.bucket("CACHE_BUCKET")?;
    // why: the delimiter collapses `nar/` into a common prefix, so the
    // enumeration prices root-level objects — narinfos — rather than the
    // NAR objects that would double every page.
    let mut held: BTreeSet<String> = BTreeSet::new();
    let mut cursor: Option<String> = None;
    let mut pages: u64 = 0;
    loop {
        let mut builder = bucket.list().delimiter("/").limit(
            u32::try_from(cachet_core::constants::BUCKET_LIST_PAGE_LIMIT)
                .expect("the page limit fits u32"),
        );
        if let Some(cursor) = &cursor {
            builder = builder.cursor(cursor);
        }
        let listed = match builder.execute().await {
            Ok(listed) => listed,
            Err(failure) => {
                log::event(
                    "error",
                    "probe.list_failed",
                    &[("error", failure.to_string())],
                );
                return error::problem_response(ClientError::StorageUnavailable);
            }
        };
        pages += 1;
        for object in listed.objects() {
            if let Some(hash) = object.key().strip_suffix(NARINFO_KEY_SUFFIX) {
                held.insert(hash.to_string());
            }
        }
        if !listed.truncated() {
            break;
        }
        cursor = listed.cursor();
    }

    let present: Vec<String> = asked.intersection(&held).cloned().collect();
    log::event(
        "info",
        "probe.bulk",
        &[
            ("paths", asked.len().to_string()),
            ("present", present.len().to_string()),
            ("pages", pages.to_string()),
        ],
    );

    let body = serde_json::to_string(&cachet_api::ProbeAnswer { present })
        .map_err(|_| worker::Error::RustError("the probe answer serializes".to_string()))?;
    let headers = worker::Headers::new();
    headers.set("content-type", "application/json")?;
    headers.set("cache-control", "no-store")?;
    Ok(Response::ok(body)?.with_headers(headers))
}
