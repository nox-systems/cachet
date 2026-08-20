//! Lease renewal and lease reads: the routes under `/roots`. I/O only
//! here: who may renew, what the document contains, and how the listing
//! is bounded are cachet-core's decisions; this module performs the bucket
//! operations they authorize.

use cachet_core::auth::OidcConfig;
use cachet_core::constants::{BUCKET_LIST_PAGE_LIMIT, ROOTS_BODY_BYTES_MAX, ROOTS_PROJECTS_MAX};
use cachet_core::error::ClientError;
use cachet_core::keys::{lease_key_for_project, project_from_lease_key};
use cachet_core::roots::{bound_project_list, build_lease_renewal, serialize_project_list};
use cachet_core::types::{ProjectName, UnixMillis};
use cachet_core::write::require_content_length;
use worker::{Env, Request, Response, Result};

use crate::{error, log};

/// JSON, uncacheable: a lease changes on every default-branch build.
fn lease_headers() -> Result<worker::Headers> {
    let headers = worker::Headers::new();
    headers.set("content-type", "application/json")?;
    headers.set("cache-control", "no-store")?;
    Ok(headers)
}

/// Renew a project's lease: a typed answer for every refusal, a 204 for a
/// stored renewal. Last write wins by design — a lease asserts what a
/// project needs now, and a path dropping out of the closure is the only
/// reason the cache stays bounded.
pub async fn renew_lease(
    env: &Env,
    config: &OidcConfig,
    identity: &cachet_core::auth::OidcIdentity,
    project: &ProjectName,
    now: UnixMillis,
    mut req: Request,
) -> Result<Response> {
    let _length = match require_content_length(
        req.headers().get("content-length")?.as_deref(),
        ROOTS_BODY_BYTES_MAX,
    ) {
        Ok(length) => length,
        Err(code) => return error::problem_response(code),
    };
    let body_text = req.text().await.map_err(|_| ClientError::MalformedRoots);
    let body_text = match body_text {
        Ok(text) => text,
        Err(code) => return error::problem_response(code),
    };
    let lease = match build_lease_renewal(project, &body_text, identity, config, now) {
        Ok(lease) => lease,
        Err(code) => {
            log::event(
                "warn",
                "roots.renewal_rejected",
                &[
                    ("project", project.as_str().to_string()),
                    ("code", code.code().to_string()),
                ],
            );
            return error::problem_response(code);
        }
    };
    match env
        .bucket("CACHE_BUCKET")?
        .put(lease_key_for_project(project), lease.serialize())
        .execute()
        .await
    {
        Ok(_) => {
            log::event(
                "info",
                "roots.renewed",
                &[
                    ("project", project.as_str().to_string()),
                    ("storePaths", lease.store_paths.len().to_string()),
                    ("commitSha", lease.commit_sha.clone()),
                ],
            );
            Ok(Response::empty()?.with_status(204))
        }
        Err(failure) => {
            log::event(
                "error",
                "roots.store_failed",
                &[("error", failure.to_string())],
            );
            error::problem_response(ClientError::StorageUnavailable)
        }
    }
}

/// List the projects holding leases, bounded and complete or refused.
/// The listing paginates with a bounded page count: a bucket past the cap
/// is reported as an overflow rather than silently truncated, because a
/// partial list is a wrong answer acting like a right one.
pub async fn list_projects(env: &Env) -> Result<Response> {
    let bucket = env.bucket("CACHE_BUCKET")?;
    let mut projects: Vec<ProjectName> = Vec::new();
    let mut cursor: Option<String> = None;
    // One page of slack past the cap: the bound below, not this loop, is
    // the final word on "too many".
    let pages = ROOTS_PROJECTS_MAX / BUCKET_LIST_PAGE_LIMIT + 1;
    for _page in 0..=pages {
        let mut builder = bucket
            .list()
            .prefix(cachet_core::constants::ROOTS_KEY_PREFIX)
            .limit(u32::try_from(BUCKET_LIST_PAGE_LIMIT).expect("the page limit fits u32"));
        if let Some(cursor) = &cursor {
            builder = builder.cursor(cursor);
        }
        let listed = match builder.execute().await {
            Ok(listed) => listed,
            Err(failure) => {
                log::event(
                    "error",
                    "roots.list_failed",
                    &[("error", failure.to_string())],
                );
                return error::problem_response(ClientError::StorageUnavailable);
            }
        };
        for object in listed.objects() {
            if let Ok(project) = project_from_lease_key(&object.key()) {
                projects.push(project);
            }
        }
        if !listed.truncated() {
            break;
        }
        cursor = listed.cursor();
    }
    if let Err(code) = bound_project_list(&projects) {
        log::alert("roots.too_many_projects");
        return error::problem_response(code);
    }
    let body = serialize_project_list(&projects);
    Ok(Response::ok(body)?.with_headers(lease_headers()?))
}

/// Serve one lease document verbatim.
pub async fn read_lease(env: &Env, project: &ProjectName) -> Result<Response> {
    let bucket = env.bucket("CACHE_BUCKET")?;
    match bucket.get(lease_key_for_project(project)).execute().await? {
        None => error::problem_response(ClientError::NotFound),
        Some(object) => {
            let Some(body) = object.body() else {
                return error::problem_response(ClientError::StorageUnavailable);
            };
            let text = body.text().await?;
            Ok(Response::ok(text)?.with_headers(lease_headers()?))
        }
    }
}
