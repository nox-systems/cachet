//! `/api/public/config`: the one unauthenticated JSON document. The CLI
//! and the later SPA discover the deployment's OAuth client id, orgs,
//! host, and public signing key here, so nothing the client needs ever
//! rides in a secret channel it does not yet have.

use cachet_core::constants::{GC_LATEST_REPORT_KEY, GC_REPORTS_KEY_PREFIX, GC_RUNS_PAGE_LIMIT};
use cachet_core::error::ClientError;
use cachet_core::gc::{GcReport, parse_run_id};
use cachet_core::types::UnixMillis;
use cachet_crypto::ed25519::NixSecretKey;
use worker::{Env, Request, Response};

use crate::{auth, log, verdict};

/// Serve the public configuration. The signing key's public half is
/// computed from the secret binding: the deployment answers from its own
/// custody rather than from a second copy that could drift.
pub fn public_config(env: &Env) -> worker::Result<Response> {
    let config = match auth::oidc_config(env) {
        Ok(config) => config,
        Err(code) => return crate::error::problem_response(code),
    };
    let client_id = match env.var("CACHET_OAUTH_CLIENT_ID") {
        Ok(client_id) => client_id.to_string(),
        Err(_) => return crate::error::problem_response(ClientError::AuthUnavailable),
    };
    let host = match env.var("CACHET_HOST") {
        Ok(host) => host.to_string(),
        Err(_) => return crate::error::problem_response(ClientError::AuthUnavailable),
    };
    let public_key = match env.secret("CACHET_SIGNING_KEY") {
        Ok(secret) => match NixSecretKey::parse(&secret.to_string()) {
            Ok(key) => key.public_key_text(),
            Err(failure) => {
                crate::log::event(
                    "error",
                    "api.signing_key_corrupt",
                    &[("error", format!("{failure:?}"))],
                );
                return crate::error::problem_response(ClientError::AuthUnavailable);
            }
        },
        Err(_) => return crate::error::problem_response(ClientError::AuthUnavailable),
    };
    log::event("info", "api.public_config", &[]);
    // The body is cachet-api's shared type: the served wire and the
    // published OpenAPI schema cannot drift apart here.
    let body = serde_json::to_string(&cachet_api::PublicConfig {
        oauth_client_id: client_id,
        orgs: config.orgs,
        host,
        public_key,
    })
    .expect("the config fields serialize");
    let headers = worker::Headers::new();
    headers.set("content-type", "application/json")?;
    headers.set("cache-control", "no-store")?;
    Ok(Response::ok(body)?.with_headers(headers))
}

/// `/api/openapi.json`: the committed generated document, served verbatim
/// so the drift gate and the served bytes check the same artifact.
pub fn openapi_document() -> worker::Result<Response> {
    let headers = worker::Headers::new();
    headers.set("content-type", "application/yaml")?;
    headers.set("cache-control", "public, max-age=300")?;
    Ok(Response::ok(OPENAPI_YAML)?.with_headers(headers))
}

// why: serving reads the same committed file the drift gate regenerates,
// so a spec change that forgets to regenerate fails CI, not clients.
const OPENAPI_YAML: &str = include_str!("../../../docs/openapi.yaml");

/// JSON with no caching: every `/api/self` answer rides live state.
fn self_headers() -> worker::Result<worker::Headers> {
    let headers = worker::Headers::new();
    headers.set("content-type", "application/json")?;
    headers.set("cache-control", "no-store")?;
    Ok(headers)
}

/// `GET /api/self/gc-runs`: one page of run ids, oldest first. Pagination
/// follows the bucket's own cursor, so a deep history page-turns cheaply.
pub async fn gc_runs_list(env: &Env, now: UnixMillis, req: &Request) -> worker::Result<Response> {
    if let Err(code) = verdict::require_admin(env, now, req).await {
        return crate::error::problem_response(code);
    }
    let bucket = env.bucket("CACHE_BUCKET")?;
    let query_cursor = req
        .url()?
        .query_pairs()
        .find(|(key, _)| key == "cursor")
        .map(|(_, value)| value.to_string());
    let page = u32::try_from(GC_RUNS_PAGE_LIMIT).expect("the page limit fits u32");
    let mut builder = bucket
        .list()
        .prefix(GC_REPORTS_KEY_PREFIX.to_string())
        .limit(page);
    if let Some(cursor) = query_cursor {
        builder = builder.cursor(cursor);
    }
    let listed = match builder.execute().await {
        Ok(listed) => listed,
        Err(failure) => {
            log::event(
                "error",
                "api.gc_runs_list_failed",
                &[("error", failure.to_string())],
            );
            return crate::error::problem_response(ClientError::StorageUnavailable);
        }
    };
    let mut runs: Vec<String> = listed
        .objects()
        .iter()
        .filter_map(|object| {
            let key = object.key();
            let suffix = key.strip_prefix(GC_REPORTS_KEY_PREFIX)?;
            let run_id = suffix.strip_suffix(".json")?;
            // The index copy is not a run.
            (run_id != "latest").then(|| run_id.to_string())
        })
        .collect();
    runs.sort();
    let body = cachet_api::GcRunList {
        runs,
        next_cursor: listed.truncated().then(|| listed.cursor()).flatten(),
    };
    let text = serde_json::to_string(&body).expect("run ids serialize");
    Ok(Response::ok(text)?.with_headers(self_headers()?))
}

/// `GET /api/self/gc-runs/{runId}`: one run's report, served as stored.
pub async fn gc_run_read(
    env: &Env,
    now: UnixMillis,
    req: &Request,
    run_id: &str,
) -> worker::Result<Response> {
    if let Err(code) = verdict::require_admin(env, now, req).await {
        return crate::error::problem_response(code);
    }
    if let Err(code) = parse_run_id(run_id) {
        return crate::error::problem_response(code);
    }
    let bucket = env.bucket("CACHE_BUCKET")?;
    let key = format!("{GC_REPORTS_KEY_PREFIX}{run_id}.json");
    let object = match bucket.get(&key).execute().await {
        Ok(object) => object,
        Err(failure) => {
            log::event(
                "error",
                "api.gc_run_read_failed",
                &[("error", failure.to_string())],
            );
            return crate::error::problem_response(ClientError::StorageUnavailable);
        }
    };
    let Some(object) = object else {
        return crate::error::problem_response(ClientError::NotFound);
    };
    let Some(body) = object.body() else {
        return crate::error::problem_response(ClientError::StorageUnavailable);
    };
    let text = body.text().await?;
    Ok(Response::ok(text)?.with_headers(self_headers()?))
}

/// `GET /api/self/stats`: the cache's totals from the newest completed
/// report, written by the collector itself so the answer is one read.
pub async fn stats(env: &Env, now: UnixMillis, req: &Request) -> worker::Result<Response> {
    if let Err(code) = verdict::require_admin(env, now, req).await {
        return crate::error::problem_response(code);
    }
    let bucket = env.bucket("CACHE_BUCKET")?;
    let object = match bucket.get(GC_LATEST_REPORT_KEY).execute().await {
        Ok(object) => object,
        Err(failure) => {
            log::event(
                "error",
                "api.stats_read_failed",
                &[("error", failure.to_string())],
            );
            return crate::error::problem_response(ClientError::StorageUnavailable);
        }
    };
    let Some(object) = object else {
        // why: a fresh deployment has no report yet; the empty answer is a
        // fact, and 404 is its honest shape rather than fabricated zeros.
        return crate::error::problem_response(ClientError::NotFound);
    };
    let Some(body) = object.body() else {
        return crate::error::problem_response(ClientError::StorageUnavailable);
    };
    let text = body.text().await?;
    let report = match GcReport::parse(&text) {
        Ok(report) => report,
        Err(failure) => {
            log::alert("api.latest_report_corrupt");
            log::event(
                "error",
                "api.stats_parse_failed",
                &[("error", format!("{failure:?}"))],
            );
            return crate::error::problem_response(ClientError::StorageUnavailable);
        }
    };
    let body = cachet_api::StatsBody {
        based_on_run_id: report.run_id,
        inventory_paths: report.inventory_paths,
        narinfos_deleted: report.narinfos_deleted,
        nars_deleted: report.nars_deleted,
        bytes_freed: report.bytes_freed,
        gate: report.gate,
        finished_at_ms: report.finished_at_ms,
    };
    let text = serde_json::to_string(&body).expect("the stats fields serialize");
    Ok(Response::ok(text)?.with_headers(self_headers()?))
}
