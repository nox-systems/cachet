//! The pipeline itself: `main_snapshot` for the composite's first step,
//! `push` for the post step. Sequencing, retries, and request shapes
//! compose the decision layer over the adapters; nothing here knows nix
//! argv or reqwest.

use crate::adapters::{Adapters, Commands, Http, TokenSource, UploadBody, WireAnswer};
use crate::error::PushError;
use crate::filter::drop_already_cached;
use crate::plan::{UploadMechanics, object_url, plan_mechanics};
use crate::retry::{RETRY_MAX, delay_after};
use crate::snapshot::{bound_candidates, parse_snapshot, store_diff};
use crate::stage::PathFacts;
use futures_util::StreamExt as _;

/// How many store paths the pipeline works on at once.
///
/// A path is staged and then uploaded, so a slot alternates between
/// spending CPU on compression and waiting on the network; running
/// sixteen means the two overlap instead of taking turns. The previous
/// pipeline ran the same width but as barriered waves, where every wave
/// cost its slowest member and finished slots idled until it landed.
/// This is a sliding window: a finished path is replaced immediately.
const UPLOAD_CONCURRENCY: usize = 16;

/// The part fan-out inside one multipart upload.
///
/// Multipart covers the objects large enough that one of them can hold
/// the whole window's attention, so its parts go out eight wide. The
/// previous pipeline uploaded multipart objects strictly one at a time,
/// in a sequential loop outside the window entirely, which made the
/// largest NARs in a push serialize against each other.
const PART_CONCURRENCY: usize = 8;

/// How often the pipeline reports progress while a long push runs. The
/// staging phase used to print nothing at all for minutes at a time.
const PROGRESS_EVERY: usize = 25;

/// The header a NAR write declares its decompressed size in, so the
/// worker's decoder has a ceiling before it reads a byte.
const NAR_BYTES_HEADER: &str = "x-cachet-nar-bytes";

/// The header a multipart upload declares its total compressed size in.
const UPLOAD_BYTES_HEADER: &str = "x-cachet-upload-bytes";

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
    /// A push is partway through its paths. Emitted every so often so a
    /// long upload is legible while it runs rather than only once it
    /// ends.
    UploadProgress {
        /// Paths finished so far.
        done: usize,
        /// Paths the push is working through.
        total: usize,
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

/// The post step: diff, probe, then a window of paths each serialized,
/// compressed, and uploaded on its own, then renew. The pipeline's result
/// is the job's diagnosis; the caller decides what green means.
pub async fn push<C: Commands, H: Http, T: TokenSource>(
    a: &Adapters<'_, C, H, T>,
    inputs: &PushInputs,
    before_snapshot: &str,
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

    // Every surviving path's facts in one nix invocation: NarHash and
    // NarSize come out of nix's own database, so the client never hashes
    // an uncompressed NAR itself.
    let facts = a.commands.path_facts(&cached.to_upload).await?;
    let uploaded = upload_paths(a, inputs, &facts, events).await?;
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

/// Push every surviving path, several at once.
///
/// The window slides: a path that finishes is replaced immediately
/// rather than waiting for its neighbours, and a path is staged and
/// uploaded by the same task, so compression and network overlap by
/// construction. The previous pipeline separated the two into phases with
/// a barrier between them, which meant every NAR in the push had to land
/// before the first narinfo went out.
///
/// A failure ends the push. Dropping the window cancels what is still in
/// flight, so a run that cannot finish stops scheduling instead of
/// working through thousands more paths to reach the same error. What
/// already landed stays landed: objects are content-addressed and each
/// path's NAR precedes its own narinfo, so a partial push leaves the
/// cache consistent and the next run resumes from the probe.
async fn upload_paths<C: Commands, H: Http, T: TokenSource>(
    a: &Adapters<'_, C, H, T>,
    inputs: &PushInputs,
    facts: &[PathFacts],
    events: &mut dyn FnMut(PushEvent),
) -> Result<usize, PushError> {
    let total = facts.len();
    let mut uploaded = 0;
    let mut done = 0;
    let mut failure = None;
    {
        let mut window = futures_util::stream::iter(facts.iter())
            .map(|path_facts| push_one_path(a, inputs, path_facts))
            .buffer_unordered(UPLOAD_CONCURRENCY);
        while let Some(result) = window.next().await {
            match result {
                Ok(objects) => {
                    uploaded += objects;
                    done += 1;
                    if done % PROGRESS_EVERY == 0 || done == total {
                        events(PushEvent::UploadProgress { done, total });
                    }
                }
                Err(refusal) => {
                    failure = Some(refusal);
                    break;
                }
            }
        }
    }
    match failure {
        Some(refusal) => Err(refusal),
        None => Ok(uploaded),
    }
}

/// One path end to end: serialize and compress it, send the NAR, then
/// send the narinfo that names it.
///
/// The ordering promise is per path rather than per push. The worker
/// verifies NAR-before-narinfo itself (NEVER-DANGLE) and refuses a
/// narinfo whose NAR has not been stored and measured, so nothing is lost
/// by letting one path's narinfo go out while another path's NAR is still
/// uploading.
async fn push_one_path<C: Commands, H: Http, T: TokenSource>(
    a: &Adapters<'_, C, H, T>,
    inputs: &PushInputs,
    facts: &PathFacts,
) -> Result<usize, PushError> {
    let staged = a.commands.stage_nar(facts).await?;
    let key = staged.nar_key();
    let body: UploadBody = staged.body.clone().into();
    let nar_bytes = staged.facts.nar_size_bytes;
    match plan_mechanics(&key, staged.file_size_bytes)? {
        UploadMechanics::Single => upload_single(a, inputs, &key, body, nar_bytes).await?,
        UploadMechanics::Multipart(shape) => {
            upload_multipart(a, inputs, &key, body, nar_bytes, shape).await?;
        }
    }
    let document = staged.narinfo()?;
    upload_narinfo(a, inputs, &document).await?;
    // The NAR and its narinfo: what this path added to the cache.
    Ok(2)
}

/// The headers a NAR write carries: the decoder's ceiling, declared
/// before the worker reads a byte.
fn nar_headers(nar_bytes: u64) -> Vec<(String, String)> {
    vec![(NAR_BYTES_HEADER.to_string(), nar_bytes.to_string())]
}

/// The single PUT, retried as a unit. The body is cheap to clone, so an
/// attempt costs a re-send rather than a re-compression or a memcopy.
async fn upload_single<C: Commands, H: Http, T: TokenSource>(
    a: &Adapters<'_, C, H, T>,
    inputs: &PushInputs,
    key: &str,
    body: UploadBody,
    nar_bytes: u64,
) -> Result<(), PushError> {
    let url = object_url(&inputs.cache_url, key, "");
    let what = format!("PUT {key}");
    let headers = nar_headers(nar_bytes);
    with_retries(&what, a.sleep, || {
        let url = url.clone();
        let body = body.clone();
        let headers = headers.clone();
        let label = what.clone();
        async move {
            let token = a.tokens.mint(&inputs.audience).await?;
            let answer = a.http.put(&url, &token, body, &headers).await?;
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

/// The narinfo PUT. This is the request the worker signs on, and it
/// reads the facts its NAR's write already recorded rather than the NAR
/// itself, so it costs a small round trip however large the path is.
async fn upload_narinfo<C: Commands, H: Http, T: TokenSource>(
    a: &Adapters<'_, C, H, T>,
    inputs: &PushInputs,
    document: &cachet_core::narinfo::Narinfo,
) -> Result<(), PushError> {
    let key = format!(
        "{}{}",
        document.store_path_hash,
        cachet_core::constants::NARINFO_KEY_SUFFIX
    );
    let url = object_url(&inputs.cache_url, &key, "");
    let bytes: std::sync::Arc<[u8]> = std::sync::Arc::from(document.serialize().into_bytes());
    let what = format!("PUT {key}");
    with_retries(&what, a.sleep, || {
        let url = url.clone();
        let body = UploadBody::Bytes(std::sync::Arc::clone(&bytes));
        let label = what.clone();
        async move {
            let token = a.tokens.mint(&inputs.audience).await?;
            let answer = a.http.put(&url, &token, body, &[]).await?;
            if answer.status == 401 {
                a.tokens.invalidate(&inputs.audience).await;
            }
            require_2xx(&label, answer)
        }
    })
    .await?;
    Ok(())
}

/// Every part of one multipart upload, eight in flight.
async fn upload_parts<C: Commands, H: Http, T: TokenSource>(
    a: &Adapters<'_, C, H, T>,
    inputs: &PushInputs,
    key: &str,
    body: &UploadBody,
    upload_id: &str,
    shape: cachet_core::multipart::PlanShape,
) -> Result<Vec<cachet_api::UploadedPartBody>, PushError> {
    let numbers: Vec<u64> = (1..=shape.count).collect();
    let mut parts: Vec<cachet_api::UploadedPartBody> = futures_util::stream::iter(numbers)
        .map(|number| upload_one_part(a, inputs, key, body, upload_id, shape, number))
        .buffer_unordered(PART_CONCURRENCY)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    // The completion body names parts in ascending order; the window
    // finishes them in whatever order the wire allows.
    parts.sort_by_key(|part| part.part_number);
    Ok(parts)
}

/// Open the upload, send its parts, and complete it. A failure anywhere
/// aborts the upload so the bucket keeps no half-assembled object.
async fn upload_multipart<C: Commands, H: Http, T: TokenSource>(
    a: &Adapters<'_, C, H, T>,
    inputs: &PushInputs,
    key: &str,
    body: UploadBody,
    nar_bytes: u64,
    shape: cachet_core::multipart::PlanShape,
) -> Result<(), PushError> {
    let url = object_url(&inputs.cache_url, key, "?uploads");
    let what = format!("begin {key}");
    let headers = vec![
        (UPLOAD_BYTES_HEADER.to_string(), body.len().to_string()),
        (NAR_BYTES_HEADER.to_string(), nar_bytes.to_string()),
    ];
    let created: cachet_api::UploadCreated = with_retries(&what, a.sleep, || {
        let url = url.clone();
        let headers = headers.clone();
        let label = what.clone();
        async move {
            let token = a.tokens.mint(&inputs.audience).await?;
            let answer = a.http.post(&url, &token, Vec::new(), &headers).await?;
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

    let parts = match upload_parts(a, inputs, key, &body, &created.upload_id, shape).await {
        Ok(parts) => parts,
        Err(failure) => {
            abort_multipart(a, inputs, key, &created.upload_id).await;
            return Err(failure);
        }
    };

    let url = object_url(
        &inputs.cache_url,
        key,
        &format!("?uploadId={}", created.upload_id),
    );
    let body_bytes = serde_json::to_vec(&parts).expect("the completion body serializes");
    let what = format!("complete {key}");
    let completed = with_retries(&what, a.sleep, || {
        let url = url.clone();
        let body = body_bytes.clone();
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
    .await;
    if let Err(failure) = completed {
        abort_multipart(a, inputs, key, &created.upload_id).await;
        return Err(failure);
    }
    Ok(())
}

/// One part: a slice of the staged body and one PUT. Slicing a file body
/// is two integers, so a part never materializes its bytes in memory
/// before the wire asks for them.
async fn upload_one_part<C: Commands, H: Http, T: TokenSource>(
    a: &Adapters<'_, C, H, T>,
    inputs: &PushInputs,
    key: &str,
    body: &UploadBody,
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
    let part = body.slice(offset, length);
    let url = object_url(
        &inputs.cache_url,
        key,
        &format!("?uploadId={upload_id}&partNumber={number}"),
    );
    let what = format!("PUT part {number} of {key}");
    let answer = with_retries(&what, a.sleep, || {
        let url = url.clone();
        let part = part.clone();
        let label = what.clone();
        async move {
            let token = a.tokens.mint(&inputs.audience).await?;
            let answer = a.http.put(&url, &token, part, &[]).await?;
            if answer.status == 401 {
                a.tokens.invalidate(&inputs.audience).await;
            }
            require_2xx(&label, answer)
        }
    })
    .await?;
    serde_json::from_slice(&answer.body).map_err(|failure| PushError::Detail {
        message: format!("the part answer did not parse: {failure}"),
    })
}

/// Discard an upload the pipeline could not finish, best effort. A
/// failed abort leaves a record the collector reaps once it is stale, so
/// the refusal the caller is already carrying is the one worth reporting.
async fn abort_multipart<C: Commands, H: Http, T: TokenSource>(
    a: &Adapters<'_, C, H, T>,
    inputs: &PushInputs,
    key: &str,
    upload_id: &str,
) {
    let url = object_url(&inputs.cache_url, key, &format!("?uploadId={upload_id}"));
    if let Ok(token) = a.tokens.mint(&inputs.audience).await {
        let _ = a.http.delete(&url, &token).await;
    }
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
