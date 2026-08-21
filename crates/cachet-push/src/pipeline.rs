//! The pipeline itself: `main_snapshot` for the composite's first step,
//! `push` for the post step. Sequencing, retries, and request shapes
//! compose the decision layer over the adapters; nothing here knows nix
//! argv or reqwest.

use futures_util::StreamExt as _;

use crate::adapters::{Adapters, Commands, Http, TokenSource, WireAnswer};
use crate::error::PushError;
use crate::filter::{drop_already_cached, filter_against_upstream};
use crate::plan::{
    StagedObject, UploadMechanics, object_url, plan_mechanics, read_staging_layout, upload_order,
};
use crate::retry::{RETRY_MAX, delay_after};
use crate::snapshot::{bound_candidates, parse_snapshot, store_diff};

/// The probe pool's width: bounded fan-out for presence checks, matching
/// the previous pipeline's tuning.
const PROBE_CONCURRENCY: usize = 16;

/// One sleep, injected so the unit lane never waits.
pub type Sleeper = dyn Fn(u64) -> futures_util::future::BoxFuture<'static, ()> + Send + Sync;

/// The values the composite resolves once into the job environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushInputs {
    /// The cache's base URL.
    pub cache_url: String,
    /// The OIDC audience.
    pub audience: String,
    /// The lease name: `owner-repo` hyphenated.
    pub project: String,
    /// The flake installables whose closures root this push.
    pub installables: Vec<String>,
    /// The upstream substituter to filter against.
    pub upstream_url: String,
    /// Whether this run answers for the configured default branch.
    pub is_default_branch: bool,
}

/// What the push did, for the caller's log lines.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PushOutcome {
    /// Candidates from the diff.
    pub added_paths: usize,
    /// Paths kept because the roots named them.
    pub roots_kept: usize,
    /// Paths the upstream already serves.
    pub upstream_hits: usize,
    /// Paths cachet already held.
    pub cache_hits: usize,
    /// Probes that answered nothing.
    pub probe_failures: usize,
    /// Objects (NAR plus narinfo files) uploaded.
    pub uploaded_objects: usize,
    /// Whether the lease renewed on this run.
    pub lease_renewed: bool,
}

/// Progress the caller renders. Each variant names one past-tense fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushEvent {
    /// The before-snapshot wrote.
    SnapshotTaken,
    /// The before-snapshot could not be taken; the push declines.
    MainSnapshotFailed {
        /// The command's complaint.
        message: String,
    },
    /// The job added nothing to the store.
    NothingAdded,
    /// One installable would not resolve and rides the lease unnamed.
    InstallableUnresolved {
        /// The installable as given.
        installable: String,
    },
    /// Pass one's tally.
    UpstreamTally {
        /// Paths still bound for pushing.
        to_push: usize,
        /// Paths the upstream already serves.
        upstream_hits: usize,
        /// Probes with no answer.
        probe_failures: usize,
    },
    /// Pass two's tally.
    CacheTally {
        /// Paths still bound for upload.
        to_upload: usize,
        /// Paths cachet already held.
        cache_hits: usize,
        /// Probes with no answer.
        probe_failures: usize,
    },
    /// All surviving objects landed.
    UploadedObjects {
        /// Narinfo plus NAR file count.
        count: usize,
    },
    /// The renewal's branch gate refused.
    LeaseSkippedNotDefaultBranch,
    /// The lease renewed.
    LeaseRenewed {
        /// The project it covers.
        project: String,
    },
}

/// The composite's main step: snapshot, or decline gracefully. A command
/// failure answers `None` with its event — the job must finish green.
pub async fn main_snapshot<C: Commands>(
    commands: &C,
    events: &mut dyn FnMut(PushEvent),
) -> Result<Option<String>, PushError> {
    match commands.path_info_all().await {
        Ok(text) => {
            events(PushEvent::SnapshotTaken);
            Ok(Some(text))
        }
        Err(failure) => {
            events(PushEvent::MainSnapshotFailed {
                message: failure.to_string(),
            });
            Ok(None)
        }
    }
}

/// The post step: diff, filter, stage, upload, renew. The pipeline's
/// result is the job's diagnosis; the caller decides what green means.
pub async fn push<C: Commands, H: Http, T: TokenSource>(
    a: &Adapters<'_, C, H, T>,
    inputs: &PushInputs,
    before_snapshot: &str,
    staging_dir: &std::path::Path,
    events: &mut dyn FnMut(PushEvent),
) -> Result<PushOutcome, PushError> {
    let mut outcome = PushOutcome::default();

    let after = a.commands.path_info_all().await?;
    let candidates = store_diff(before_snapshot, &after);
    bound_candidates(&candidates)?;
    outcome.added_paths = candidates.len();
    if candidates.is_empty() {
        events(PushEvent::NothingAdded);
        return Ok(outcome);
    }

    // Roots resolve best-effort: an unresolvable installable still rides
    // the lease by name; only its paths are missing.
    let mut resolved_root_paths: Vec<String> = Vec::new();
    for installable in &inputs.installables {
        match a.commands.path_info(installable).await {
            Ok(text) => resolved_root_paths.extend(parse_snapshot(&text)),
            Err(_) => events(PushEvent::InstallableUnresolved {
                installable: installable.clone(),
            }),
        }
    }
    let root_set: std::collections::BTreeSet<String> =
        resolved_root_paths.iter().cloned().collect();
    outcome.roots_kept = candidates
        .iter()
        .filter(|path| root_set.contains(*path))
        .count();

    // Pass one: upstream. Probes fan out sixteen wide, answers ordered.
    let probe_token = a.tokens.mint(&inputs.audience).await?;
    let upstream_answers = probe_pool(a.http, &inputs.upstream_url, None, &candidates).await;
    let upstream = filter_against_upstream(&candidates, &root_set, &|path| {
        upstream_answers
            .iter()
            .find(|(candidate, _)| candidate == path)
            .and_then(|(_, answer)| *answer)
    });
    outcome.upstream_hits = upstream.upstream_hits;
    outcome.probe_failures += upstream.probe_failures;
    events(PushEvent::UpstreamTally {
        to_push: upstream.kept.len(),
        upstream_hits: upstream.upstream_hits,
        probe_failures: upstream.probe_failures,
    });
    if upstream.kept.is_empty() {
        return finish_with_lease(a, inputs, events, outcome, &resolved_root_paths).await;
    }

    // Pass two: cachet itself.
    let cache_answers = probe_pool(
        a.http,
        &inputs.cache_url,
        Some(&probe_token),
        &upstream.kept,
    )
    .await;
    let cached = drop_already_cached(&upstream.kept, &|path| {
        cache_answers
            .iter()
            .find(|(candidate, _)| candidate == path)
            .and_then(|(_, answer)| *answer)
    });
    outcome.cache_hits = cached.cache_hits;
    outcome.probe_failures += cached.probe_failures;
    events(PushEvent::CacheTally {
        to_upload: cached.to_upload.len(),
        cache_hits: cached.cache_hits,
        probe_failures: cached.probe_failures,
    });
    if cached.to_upload.is_empty() {
        return finish_with_lease(a, inputs, events, outcome, &resolved_root_paths).await;
    }

    // Stage through nix: the store's own serialization and compression,
    // unsigned — the cache's pipeline verifies, then signs.
    let destination = format!("file://{}?compression=zstd", staging_dir.display());
    a.commands.copy_to(&destination, &cached.to_upload).await?;
    let entries = a.commands.read_dir(staging_dir).await?;
    let mut objects = read_staging_layout(&entries)?;
    upload_order(&mut objects);
    let uploaded = upload_objects(a, inputs, staging_dir, &objects).await?;
    outcome.uploaded_objects = uploaded;
    events(PushEvent::UploadedObjects { count: uploaded });

    finish_with_lease(a, inputs, events, outcome, &resolved_root_paths).await
}

/// The renewal tail every non-empty candidate set reaches: gated on the
/// default branch, retried in the envelope like everything else.
async fn finish_with_lease<C: Commands, H: Http, T: TokenSource>(
    a: &Adapters<'_, C, H, T>,
    inputs: &PushInputs,
    events: &mut dyn FnMut(PushEvent),
    mut outcome: PushOutcome,
    resolved_root_paths: &[String],
) -> Result<PushOutcome, PushError> {
    if !inputs.is_default_branch {
        events(PushEvent::LeaseSkippedNotDefaultBranch);
        outcome.lease_renewed = false;
        return Ok(outcome);
    }
    let body = cachet_api::RenewalBody {
        installables: inputs.installables.clone(),
        store_paths: resolved_root_paths.to_vec(),
    };
    let body_bytes = serde_json::to_vec(&body).expect("the renewal body serializes");
    let url = object_url(&inputs.cache_url, &format!("roots/{}", inputs.project), "");
    let what = format!("renew the lease for {}", inputs.project);
    with_retries(&what, a.sleep, || {
        let body_bytes = body_bytes.clone();
        let url = url.clone();
        let label = what.clone();
        async move {
            let token = a.tokens.mint(&inputs.audience).await?;
            let answer = a
                .http
                .post(
                    &url,
                    &token,
                    body_bytes,
                    &[("content-type".to_string(), "application/json".to_string())],
                )
                .await?;
            require_2xx(&label, answer)
        }
    })
    .await?;
    events(PushEvent::LeaseRenewed {
        project: inputs.project.clone(),
    });
    outcome.lease_renewed = true;
    Ok(outcome)
}

/// The probe pool: ordered answers, sixteen in flight, each one
/// `Some(present)` / `Some(absent)` / `None` for a request that could not
/// answer.
async fn probe_pool<H: Http>(
    http: &H,
    base_url: &str,
    bearer: Option<&str>,
    paths: &[String],
) -> Vec<(String, Option<bool>)> {
    futures_util::stream::iter(paths.iter().map(|path| {
        let bearer = bearer.map(str::to_string);
        async move {
            let url = probe_url(base_url, path);
            let answer = match http.head(&url, bearer.as_deref()).await {
                Ok(404) => Some(false),
                Ok(status) if (200..300).contains(&status) => Some(true),
                _ => None,
            };
            (path.clone(), answer)
        }
    }))
    .buffered(PROBE_CONCURRENCY)
    .collect()
    .await
}

/// The probe URL for one path: its hash half names the narinfo. A path
/// outside the grammar is unprobeable: its probe answers nothing, and the
/// filter keeps it, fail-toward-rebuild.
fn probe_url(base_url: &str, path: &str) -> String {
    let hash = cachet_core::keys::parse_store_path(path)
        .map(|parts| parts.hash.as_str().to_string())
        .unwrap_or_default();
    object_url(base_url, &format!("{hash}.narinfo"), "")
}

/// Upload every object in plan order: NAR bodies first at all times.
async fn upload_objects<C: Commands, H: Http, T: TokenSource>(
    a: &Adapters<'_, C, H, T>,
    inputs: &PushInputs,
    staging_dir: &std::path::Path,
    objects: &[StagedObject],
) -> Result<usize, PushError> {
    let mut uploaded = 0;
    for object in objects {
        match plan_mechanics(&object.key, object.size_bytes)? {
            UploadMechanics::Single => {
                upload_single(a, inputs, staging_dir, object).await?;
            }
            UploadMechanics::Multipart(shape) => {
                upload_multipart(a, inputs, staging_dir, object, shape).await?;
            }
        }
        uploaded += 1;
    }
    Ok(uploaded)
}

/// The single PUT, retried as a unit. The body loads once; each attempt
/// reuses it with a token seconds old.
async fn upload_single<C: Commands, H: Http, T: TokenSource>(
    a: &Adapters<'_, C, H, T>,
    inputs: &PushInputs,
    staging_dir: &std::path::Path,
    object: &StagedObject,
) -> Result<(), PushError> {
    let body = a
        .commands
        .read_file(&staging_dir.join(&object.file_name))
        .await?;
    let url = object_url(&inputs.cache_url, &object.key, "");
    let what = format!("PUT {}", object.key);
    with_retries(&what, a.sleep, || {
        let url = url.clone();
        let body = body.clone();
        let label = what.clone();
        async move {
            let token = a.tokens.mint(&inputs.audience).await?;
            let answer = a.http.put(&url, &token, body, &[]).await?;
            require_2xx(&label, answer)
        }
    })
    .await?;
    Ok(())
}

/// The multipart quartet, each piece retried alone; a part that dies
/// aborts the upload best-effort and the failure propagates.
async fn upload_multipart<C: Commands, H: Http, T: TokenSource>(
    a: &Adapters<'_, C, H, T>,
    inputs: &PushInputs,
    staging_dir: &std::path::Path,
    object: &StagedObject,
    shape: cachet_core::multipart::PlanShape,
) -> Result<(), PushError> {
    let url = object_url(&inputs.cache_url, &object.key, "?uploads");
    let what = format!("begin {}", object.key);
    let created: cachet_api::UploadCreated = with_retries(&what, a.sleep, || {
        let url = url.clone();
        let label = what.clone();
        async move {
            let token = a.tokens.mint(&inputs.audience).await?;
            let answer = a
                .http
                .post(
                    &url,
                    &token,
                    Vec::new(),
                    &[(
                        "x-cachet-upload-bytes".to_string(),
                        object.size_bytes.to_string(),
                    )],
                )
                .await?;
            let answer = require_2xx(&label, answer)?;
            serde_json::from_slice(&answer.body).map_err(|failure| PushError::Detail {
                message: format!("the begin answer did not parse: {failure}"),
            })
        }
    })
    .await?;

    let mut parts: Vec<cachet_api::UploadedPartBody> = Vec::new();
    for number in 1..=shape.count {
        match upload_one_part(
            a,
            inputs,
            staging_dir,
            object,
            &created.upload_id,
            shape,
            number,
        )
        .await
        {
            Ok(part) => parts.push(part),
            Err(failure) => {
                abort_multipart(a, inputs, object, &created.upload_id).await;
                return Err(failure);
            }
        }
    }

    let url = object_url(
        &inputs.cache_url,
        &object.key,
        &format!("?uploadId={}", created.upload_id),
    );
    let body = serde_json::to_vec(&parts).expect("the completion body serializes");
    let what = format!("complete {}", object.key);
    with_retries(&what, a.sleep, || {
        let url = url.clone();
        let body = body.clone();
        let label = what.clone();
        async move {
            let token = a.tokens.mint(&inputs.audience).await?;
            let answer = a
                .http
                .post(
                    &url,
                    &token,
                    body,
                    &[("content-type".to_string(), "application/json".to_string())],
                )
                .await?;
            require_2xx(&label, answer)
        }
    })
    .await?;
    Ok(())
}

/// One part: a ranged read from the staged file and one PUT.
async fn upload_one_part<C: Commands, H: Http, T: TokenSource>(
    a: &Adapters<'_, C, H, T>,
    inputs: &PushInputs,
    staging_dir: &std::path::Path,
    object: &StagedObject,
    upload_id: &str,
    shape: cachet_core::multipart::PlanShape,
    number: u64,
) -> Result<cachet_api::UploadedPartBody, PushError> {
    let offset = (number - 1) * cachet_core::constants::UPLOAD_PART_BYTES;
    let length = if number == shape.count {
        shape.last_len
    } else {
        cachet_core::constants::UPLOAD_PART_BYTES
    };
    let body = a
        .commands
        .read_range(&staging_dir.join(&object.file_name), offset, length)
        .await?;
    let url = object_url(
        &inputs.cache_url,
        &object.key,
        &format!("?uploadId={upload_id}&partNumber={number}"),
    );
    let what = format!("part {number} of {}", object.key);
    with_retries(&what, a.sleep, || {
        let url = url.clone();
        let body = body.clone();
        let label = what.clone();
        async move {
            let token = a.tokens.mint(&inputs.audience).await?;
            let answer = a.http.put(&url, &token, body, &[]).await?;
            let answer = require_2xx(&label, answer)?;
            serde_json::from_slice(&answer.body).map_err(|failure| PushError::Detail {
                message: format!("the part answer did not parse: {failure}"),
            })
        }
    })
    .await
}

/// The abort: best-effort by contract; a mint failure skips silently and
/// the collector reclaims the upload later.
async fn abort_multipart<C: Commands, H: Http, T: TokenSource>(
    a: &Adapters<'_, C, H, T>,
    inputs: &PushInputs,
    object: &StagedObject,
    upload_id: &str,
) {
    let Ok(token) = a.tokens.mint(&inputs.audience).await else {
        return;
    };
    let url = object_url(
        &inputs.cache_url,
        &object.key,
        &format!("?uploadId={upload_id}"),
    );
    let _ = a.http.delete(&url, &token).await;
}

/// The retry envelope: an operation runs until it works or the envelope
/// closes, naming the label, the count, and the last complaint.
async fn with_retries<T, Fut>(
    what: &str,
    sleep: &Sleeper,
    mut attempt: impl FnMut() -> Fut,
) -> Result<T, PushError>
where
    Fut: std::future::Future<Output = Result<T, PushError>>,
{
    let mut last = String::new();
    for tries in 0..RETRY_MAX {
        match attempt().await {
            Ok(value) => return Ok(value),
            Err(failure) => {
                last = failure.to_string();
                if let Some(delay_ms) = delay_after(tries) {
                    sleep(delay_ms).await;
                }
            }
        }
    }
    Err(PushError::UploadFailed {
        what: what.to_string(),
        attempts: RETRY_MAX,
        last,
    })
}

/// Any 2xx passes as the wire's ok; everything else fails with the body.
fn require_2xx(what: &str, answer: WireAnswer) -> Result<WireAnswer, PushError> {
    if (200..300).contains(&answer.status) {
        return Ok(answer);
    }
    Err(PushError::UploadFailed {
        what: what.to_string(),
        attempts: 1,
        last: format!(
            "{} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body),
        ),
    })
}
