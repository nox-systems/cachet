//! The pipeline itself: `main_snapshot` for the composite's first step,
//! `push` for the post step. Sequencing, retries, and request shapes
//! compose the decision layer over the adapters; nothing here knows nix
//! argv or reqwest.

use crate::adapters::{Adapters, Commands, Http, TokenSource, WireAnswer};
use crate::error::PushError;
use crate::filter::drop_already_cached;
use crate::plan::{
    StagedObject, UploadMechanics, object_url, owned_object_keys, plan_mechanics,
    read_staging_layout, upload_order,
};
use crate::retry::{RETRY_MAX, delay_after};
use crate::snapshot::{bound_candidates, parse_snapshot, store_diff};

/// One wave's upper membership: sixteen object uploads in flight, the
/// previous pipeline's tuning carried over.
const UPLOAD_CONCURRENCY: usize = 16;

/// why: the budget bounds resident bodies at four large ones; waves
/// partition by count first and bytes second, and the budget must always
/// admit the largest legal single body.
const UPLOAD_WAVE_BYTES: u64 = 4 * cachet_core::constants::UPLOAD_SINGLE_MAX_BYTES;

/// The part fan-out inside one multipart upload. Its byte budget matches
/// four whole parts, which is also all the parts one wave may hold.
const PART_CONCURRENCY: usize = 4;

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
    /// Paths cachet already held.
    pub cache_hits: usize,
    /// Candidates that did not parse as store paths; kept for upload.
    pub unparseable_paths: usize,
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
    /// The probe's tally.
    CacheTally {
        /// Paths still bound for upload.
        to_upload: usize,
        /// Paths cachet already held.
        cache_hits: usize,
        /// Candidates that did not parse as store paths; kept for upload.
        unparseable_paths: usize,
    },
    /// The bulk probe could not answer; every candidate counts as absent,
    /// because re-uploading is the safe side of the error.
    ProbeBulkFailed {
        /// The probe's complaint.
        message: String,
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

    // The presence question goes to cachet in one bulk request. There is
    // no upstream pass: the cache stores what the org pushes, the way
    // cachix and flakehub do, because a store is its own dedup and a
    // foreign cache's holdings are none of this pipeline's business. A
    // probe that cannot answer treats every candidate as absent: re-upload
    // is re-signed identical bytes, but a false "held" would strand
    // clients on a 404.
    let present = match probe_present_set(a, inputs, &candidates).await {
        Ok(present) => present,
        Err(failure) => {
            events(PushEvent::ProbeBulkFailed {
                message: failure.to_string(),
            });
            std::collections::BTreeSet::new()
        }
    };
    let cached = drop_already_cached(&candidates, &present);
    outcome.cache_hits = cached.cache_hits;
    outcome.unparseable_paths = cached.unparseable_paths;
    events(PushEvent::CacheTally {
        to_upload: cached.to_upload.len(),
        cache_hits: cached.cache_hits,
        unparseable_paths: cached.unparseable_paths,
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
    // why: `nix copy` stages the survivors' closures, but the probe
    // already priced every path. The wire set is the survivors' own
    // pairs: their narinfos plus exactly the NARs those narinfos name.
    // Closure members predate this job — pushed when they entered the
    // store, or foreign and fetched from the next substituter — so
    // re-staging them is the thousands-of-objects-for-a-handful-of-paths
    // class this filter exists to stop. GC reads are safe by the same
    // construction: a deep reference without a pushed narinfo marks
    // without descent.
    let mut survivor_bodies = std::collections::BTreeMap::new();
    for path in &cached.to_upload {
        let hash = cachet_core::keys::parse_store_path(path)
            .map_err(|_| PushError::Detail {
                message: format!("a survivor outside the store-path grammar: {path}"),
            })?
            .hash
            .as_str()
            .to_string();
        let key = format!("{hash}{}", cachet_core::constants::NARINFO_KEY_SUFFIX);
        let body = a.commands.read_file(&staging_dir.join(&key)).await?;
        survivor_bodies.insert(
            hash,
            String::from_utf8(body).map_err(|_| PushError::Detail {
                message: format!("the staged narinfo for {path} is not text"),
            })?,
        );
    }
    let owned = owned_object_keys(&survivor_bodies)?;
    objects.retain(|object| owned.contains(&object.key));
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
            if answer.status == 401 {
                a.tokens.invalidate(&inputs.audience).await;
            }
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

/// The presence set from the bulk probe: one authorized `POST
/// /api/probe` answers the whole candidate set at once. Candidates that
/// fail the store-path grammar never join the body; the filter keeps
/// them for upload the same way the old per-path probes did, fail-toward
/// rebuild. The mint lives inside the retry closure so a retried attempt
/// re-reads the memo (and a 401 clears it) rather than replaying a token
/// seconds old.
async fn probe_present_set<C: Commands, H: Http, T: TokenSource>(
    a: &Adapters<'_, C, H, T>,
    inputs: &PushInputs,
    candidates: &[String],
) -> Result<std::collections::BTreeSet<String>, PushError> {
    let body_bytes = serde_json::to_vec(&cachet_api::ProbeBody {
        paths: candidates
            .iter()
            .filter_map(|path| {
                cachet_core::keys::parse_store_path(path)
                    .ok()
                    .map(|parts| parts.hash.as_str().to_string())
            })
            .collect(),
    })
    .expect("the probe body serializes");
    let url = format!("{}/api/probe", inputs.cache_url.trim_end_matches('/'));
    let answer = with_retries("the presence probe", a.sleep, || {
        let url = url.clone();
        let body_bytes = body_bytes.clone();
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
            if answer.status == 401 {
                a.tokens.invalidate(&inputs.audience).await;
            }
            require_2xx("the presence probe", answer)
        }
    })
    .await?;
    let answer: cachet_api::ProbeAnswer =
        serde_json::from_slice(&answer.body).map_err(|_| PushError::Detail {
            message: "the probe answered with an undecodable body".to_string(),
        })?;
    Ok(std::collections::BTreeSet::from_iter(answer.present))
}

/// Run an item list through the wave plan: each wave's items join_all,
/// and a failing wave ends the run with the plan-order-first error
/// before any later wave starts. Waves are how the fan-out stays
/// testable: the call traffic for one wave is a fixed set, and result
/// mapping preserves item order by construction.
async fn wave_run<'x, T, R>(
    items: &'x [T],
    sizes: &[u64],
    max_count: usize,
    byte_budget: u64,
    run: &(dyn Fn(&'x T) -> futures_util::future::BoxFuture<'x, Result<R, PushError>> + Sync + 'x),
) -> Result<Vec<R>, PushError> {
    debug_assert_eq!(items.len(), sizes.len());
    let mut done = Vec::with_capacity(items.len());
    for (start, end) in crate::plan::wave_plan(sizes, max_count, byte_budget) {
        let wave = futures_util::future::join_all(items[start..end].iter().map(run)).await;
        // join_all preserves item order, so the first Err here is the
        // plan-order-first failure of the whole list.
        match wave.into_iter().collect::<Result<Vec<_>, _>>() {
            Ok(batch) => done.extend(batch),
            Err(first) => return Err(first),
        }
    }
    Ok(done)
}

/// Upload every object: single NARs in waves, multipart quartets one at
/// a time with their parts waved inside, then every narinfo in waves.
/// why: the worker verifies the NAR-before-narinfo ordering rather than
/// trusting it (NEVER-DANGLE), but the client keeps its own promise —
/// every NAR completes before any narinfo begins.
async fn upload_objects<C: Commands, H: Http, T: TokenSource>(
    a: &Adapters<'_, C, H, T>,
    inputs: &PushInputs,
    staging_dir: &std::path::Path,
    objects: &[StagedObject],
) -> Result<usize, PushError> {
    let mut singles = Vec::new();
    let mut single_sizes = Vec::new();
    let mut multiparts = Vec::new();
    let mut narinfos = Vec::new();
    let mut narinfo_sizes = Vec::new();
    for object in objects {
        match plan_mechanics(&object.key, object.size_bytes)? {
            UploadMechanics::Single => {
                if object.is_narinfo() {
                    narinfos.push(object);
                    narinfo_sizes.push(object.size_bytes);
                } else {
                    singles.push(object);
                    single_sizes.push(object.size_bytes);
                }
            }
            UploadMechanics::Multipart(shape) => multiparts.push((object, shape)),
        }
    }
    let run_single =
        |object: &&StagedObject| -> futures_util::future::BoxFuture<'_, Result<(), PushError>> {
            // The future owns its object: a wave outlives the caller's stack,
            // so nothing the wave runs may borrow a parameter frame.
            let object = (*object).clone();
            Box::pin(async move { upload_single(a, inputs, staging_dir, &object).await })
        };
    wave_run(
        &singles,
        &single_sizes,
        UPLOAD_CONCURRENCY,
        UPLOAD_WAVE_BYTES,
        &run_single,
    )
    .await?;
    for (object, shape) in multiparts {
        upload_multipart(a, inputs, staging_dir, object, shape).await?;
    }
    wave_run(
        &narinfos,
        &narinfo_sizes,
        UPLOAD_CONCURRENCY,
        UPLOAD_WAVE_BYTES,
        &run_single,
    )
    .await?;
    Ok(objects.len())
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
        // why: the object's key IS its staging-relative path; reading by
        // anything narrower (a basename) misses nar/'s level of the tree.
        .read_file(&staging_dir.join(&object.key))
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
            // why: a 401 is the only truthful signal a run-scoped token
            // has aged out; telling the source to refill it is cheaper
            // than minting for every attempt.
            if answer.status == 401 {
                a.tokens.invalidate(&inputs.audience).await;
            }
            require_2xx(&label, answer)
        }
    })
    .await?;
    Ok(())
}

/// One multipart upload's parts, waved 4-wide. A failed wave aborts the
/// upload best-effort and the plan-order-first part failure returns;
/// the completion-body order comes from the wave's index alignment, so
/// part numbers ascend without sorting.
async fn upload_parts<C: Commands, H: Http, T: TokenSource>(
    a: &Adapters<'_, C, H, T>,
    inputs: &PushInputs,
    staging_dir: &std::path::Path,
    object: &StagedObject,
    upload_id: &str,
    shape: cachet_core::multipart::PlanShape,
) -> Result<Vec<cachet_api::UploadedPartBody>, PushError> {
    let numbers: Vec<u64> = (1..=shape.count).collect();
    let sizes: Vec<u64> = numbers
        .iter()
        .map(|number| {
            if *number == shape.count {
                shape.last_len
            } else {
                cachet_core::constants::UPLOAD_PART_BYTES
            }
        })
        .collect();
    let run_part = |number: &u64| -> futures_util::future::BoxFuture<
        '_,
        Result<cachet_api::UploadedPartBody, PushError>,
    > {
        Box::pin(upload_one_part(
            a,
            inputs,
            staging_dir,
            object,
            upload_id,
            shape,
            *number,
        ))
    };
    match wave_run(
        &numbers,
        &sizes,
        PART_CONCURRENCY,
        PART_CONCURRENCY as u64 * cachet_core::constants::UPLOAD_PART_BYTES,
        &run_part,
    )
    .await
    {
        Ok(parts) => Ok(parts),
        Err(failure) => {
            // why: canceling sibling PUTs mid-wave would make the abort
            // coverage timing-dependent; a finished wave plus one DELETE
            // is the deterministic version of the same cleanup.
            abort_multipart(a, inputs, object, upload_id).await;
            Err(failure)
        }
    }
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
            if answer.status == 401 {
                a.tokens.invalidate(&inputs.audience).await;
            }
            let answer = require_2xx(&label, answer)?;
            serde_json::from_slice(&answer.body).map_err(|failure| PushError::Detail {
                message: format!("the begin answer did not parse: {failure}"),
            })
        }
    })
    .await?;

    let parts = upload_parts(a, inputs, staging_dir, object, &created.upload_id, shape).await?;

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
            if answer.status == 401 {
                a.tokens.invalidate(&inputs.audience).await;
            }
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
        .read_range(&staging_dir.join(&object.key), offset, length)
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
            if answer.status == 401 {
                a.tokens.invalidate(&inputs.audience).await;
            }
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
