//! The cron-driven collector. cachet-core decides everything destructive
//! as data (the walk, the gates, the plan, the report); this module
//! enumerates, reads, walks, deletes, and reports against the real
//! bucket, persisting `meta/gc-cursor` at each boundary so a cache of any
//! size sweeps across ticks inside one binding's limits.
//!
//! Resume semantics: inventory and leases recompute every tick (their
//! listings are bounded and their outputs are cheap to rebuild), while
//! the mark, collect, and sweep stages hold frozen state in the cursor.
//! Frozen marks and a frozen plan only ever over-protect a path relative
//! to a newer inventory, which is the safe direction for a deletion
//! system; the grace boundary settles at plan time and stays settled for
//! the run.
//!
//! Budget accounting: every binding call spends one operation against
//! [`GC_OP_BUDGET`] (one list page, one read, one put, or one deletion
//! batch counts the same, because the batch is the API unit), and the
//! wall clock answers to [`GC_HEADROOM_MS`]. Either limit hit freezes the
//! run: the cursor already holds what the next tick needs, because
//! deletion is idempotent and ordered narinfos before NARs at every
//! instant.
//!
//! One honest boundary: a NAR no narinfo names anywhere is unreachable
//! for this design and survives. Sweeping it means diffing the whole nar/
//! space against every living narinfo's URL, which is a v2 pass; the
//! safety laws never weaken for it.

use std::collections::BTreeMap;

use cachet_core::constants::{
    BUCKET_LIST_PAGE_LIMIT, GC_CURSOR_OBJECT_KEY, GC_DELETE_BATCH, GC_HEADROOM_MS, GC_OP_BUDGET,
    GC_REPORTS_KEY_PREFIX, GC_RUNS_KEY_PREFIX, GC_RUNS_RETENTION_MS, GENERATION_OBJECT_KEY,
    ROOTS_KEY_PREFIX, UPLOAD_STALE_MAX_MS, UPLOADS_KEY_PREFIX,
};
use cachet_core::gc::{
    ClosureWalker, CollectCursor, CollectedUrl, GateTrip, GcCursor, GcReport, GcStage,
    InventoryItem, SweepCursor, WalkOutcome, WalkReadError, plan_deletions, sweep_candidates,
};
use cachet_core::generation::GenerationDocument;
use cachet_core::keys::{NarKey, parse_nar_key, parse_store_path};
use cachet_core::lease::LeaseDocument;
use cachet_core::narinfo::Narinfo;
use cachet_core::types::{StorePathHash, UnixMillis};
use cachet_core::upload_record::UploadRecord;
use worker::{Bucket, Env, Result};

use crate::log;

/// One invocation's spend: binding calls and wall time, each with its
/// honest ceiling.
struct Budget {
    ops: u64,
    started_at_ms: u64,
}

impl Budget {
    fn new(started_at_ms: u64) -> Self {
        Self {
            ops: 0,
            started_at_ms,
        }
    }

    fn spend(&mut self) {
        self.ops += 1;
    }

    /// Either ceiling hit: time to freeze cleanly.
    fn spent_out(&self, now_ms: u64) -> bool {
        self.ops >= GC_OP_BUDGET || now_ms.saturating_sub(self.started_at_ms) >= GC_HEADROOM_MS
    }
}

/// The invocation's wall clock, sampled at each check: the Clock seam's
/// scheduled-event instance (CLAUDE.md §3).
fn now_ms() -> u64 {
    worker::Date::now().as_millis()
}

/// Whether the collector fires: armed unless the var explicitly says `0`.
fn armed(env: &Env) -> bool {
    env.var("CACHET_GC_ARMED")
        .map_or(true, |value| value.to_string() != "0")
}

/// The grace window in force, overridable per deployment for staging.
fn grace_window_ms(env: &Env) -> u64 {
    env.var("CACHET_GC_GRACE_MS")
        .ok()
        .and_then(|value| value.to_string().parse().ok())
        .unwrap_or(cachet_core::constants::GRACE_WINDOW_MS)
}

/// The run id: start time plus entropy, so artifacts sort in run order
/// and no two runs can collide.
fn run_id(started_at_ms: u64) -> String {
    let mut entropy = [0_u8; 8];
    getrandom::getrandom(&mut entropy).expect("getrandom cannot fail on workers");
    let hex = entropy
        .iter()
        .fold(String::with_capacity(16), |mut acc, b| {
            acc.push(char::from(b"0123456789abcdef"[usize::from(b >> 4)]));
            acc.push(char::from(b"0123456789abcdef"[usize::from(b & 0x0f)]));
            acc
        });
    format!("{started_at_ms}-{hex}")
}

fn stage_name(stage: GcStage) -> &'static str {
    match stage {
        GcStage::Inventory => "inventory",
        GcStage::Leases => "leases",
        GcStage::Mark => "mark",
        GcStage::Candidates => "candidates",
        GcStage::Sweep => "sweep",
        GcStage::Report => "report",
    }
}

fn fresh_cursor(started_at_ms: u64) -> GcCursor {
    GcCursor {
        run_id: run_id(started_at_ms),
        started_at_ms,
        stage: GcStage::Inventory,
        inventory_paths: 0,
        active_leases: 0,
        marked_paths: 0,
        unreadable_deep: 0,
        mark: None,
        collect: None,
        sweep: None,
        uploads_aborted: 0,
    }
}

/// What one narinfo fetch told the walk.
enum WalkFetch {
    Document(Box<Narinfo>),
    Missing,
    Corrupt,
    /// A bucket failure: transient by assumption, and the run parks
    /// unanswered rather than guess.
    Outage(worker::Error),
}

/// Store the cursor and say so. A freeze is quiet in the bucket beyond
/// the cursor itself: the run is parked, not concluded.
async fn freeze(bucket: &Bucket, budget: &mut Budget, cursor: &GcCursor) -> Result<()> {
    budget.spend();
    bucket
        .put(GC_CURSOR_OBJECT_KEY, cursor.serialize())
        .execute()
        .await?;
    log::event(
        "info",
        "gc.paused",
        &[
            ("runId", cursor.run_id.clone()),
            ("stage", stage_name(cursor.stage).to_string()),
            ("ops", budget.ops.to_string()),
        ],
    );
    Ok(())
}

/// Read the frozen run, or report its absence. Both parse failures and
/// bucket failures surface here; the caller starts a fresh run on either
/// and logs the shape it got.
async fn read_cursor(bucket: &Bucket, budget: &mut Budget) -> Result<Option<GcCursor>> {
    budget.spend();
    let Some(object) = bucket.get(GC_CURSOR_OBJECT_KEY).execute().await? else {
        return Ok(None);
    };
    let Some(body) = object.body() else {
        return Err(worker::Error::RustError(
            "cursor object without a body".to_string(),
        ));
    };
    let text = body.text().await?;
    match GcCursor::parse(&text) {
        Ok(cursor) => Ok(Some(cursor)),
        Err(failure) => Err(worker::Error::RustError(format!(
            "cursor unparsable: {failure:?}"
        ))),
    }
}

/// One bucket listing, all pages, as inventory items. A page is one
/// operation; enumerations that cannot fit a tick return Err and the run
/// freezes at its stage to try again next tick, so a too-large listing
/// pauses the collector rather than fooling it.
async fn full_listing(
    bucket: &Bucket,
    budget: &mut Budget,
    prefix: &str,
) -> std::result::Result<Vec<worker::Object>, ()> {
    let mut all = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        if budget.spent_out(now_ms()) {
            return Err(());
        }
        budget.spend();
        let mut builder = bucket
            .list()
            .prefix(prefix.to_string())
            .limit(u32::try_from(BUCKET_LIST_PAGE_LIMIT).expect("the page limit fits u32"));
        if let Some(cursor_value) = &cursor {
            builder = builder.cursor(cursor_value.clone());
        }
        let listed = builder.execute().await.map_err(|_| ())?;
        all.extend(listed.objects());
        if !listed.truncated() {
            return Ok(all);
        }
        cursor = listed.cursor();
    }
}

/// Read every active lease into the walk's root set. Each lease document
/// is one read; a roster that cannot fit the tick's budget trips the
/// truncation gate rather than pausing silently every tick, because the
/// report is the alarm channel the operator reads.
/// The walk's roots plus the count of active leases they came from: the
/// report counts leases, not paths.
async fn stage_leases(
    bucket: &Bucket,
    budget: &mut Budget,
    now: UnixMillis,
) -> std::result::Result<(u64, Vec<StorePathHash>), GateTrip> {
    let objects = full_listing(bucket, budget, ROOTS_KEY_PREFIX)
        .await
        .map_err(|()| GateTrip::LeasesTruncated)?;
    let mut roots = Vec::new();
    let mut active = 0_u64;
    for object in objects {
        if budget.spent_out(now_ms()) {
            return Err(GateTrip::LeasesTruncated);
        }
        // why: listed objects are metadata only; the document needs its
        // own get.
        budget.spend();
        let key = object.key();
        let full = bucket
            .get(&key)
            .execute()
            .await
            .map_err(|_| GateTrip::UnparseableLease { key: key.clone() })?
            .ok_or(GateTrip::UnparseableLease { key: key.clone() })?;
        let Some(body) = full.body() else {
            return Err(GateTrip::UnparseableLease { key });
        };
        let text = body
            .text()
            .await
            .map_err(|_| GateTrip::UnparseableLease { key: key.clone() })?;
        let lease = LeaseDocument::parse(&text)
            .map_err(|_| GateTrip::UnparseableLease { key: key.clone() })?;
        if !lease.is_active(now, cachet_core::constants::LEASE_RETENTION_MS) {
            continue;
        }
        active += 1;
        roots.extend(
            lease
                .store_paths
                .iter()
                .filter_map(|path| parse_store_path(path).ok().map(|parts| parts.hash)),
        );
    }
    Ok((active, roots))
}

/// Fetch the narinfo a walk step needs.
async fn read_walk_narinfo(
    bucket: &Bucket,
    budget: &mut Budget,
    hash: &StorePathHash,
) -> WalkFetch {
    budget.spend();
    let key = format!("{hash}{}", cachet_core::constants::NARINFO_KEY_SUFFIX);
    let object = match bucket.get(&key).execute().await {
        Ok(object) => object,
        Err(failure) => return WalkFetch::Outage(failure),
    };
    let Some(object) = object else {
        return WalkFetch::Missing;
    };
    let Some(body) = object.body() else {
        return WalkFetch::Outage(worker::Error::RustError(
            "narinfo object without a body".to_string(),
        ));
    };
    match body.text().await {
        Ok(text) => match Narinfo::parse(&text) {
            Ok(document) => WalkFetch::Document(Box::new(document)),
            Err(_) => WalkFetch::Corrupt,
        },
        Err(failure) => WalkFetch::Outage(failure),
    }
}

/// Write one stage artifact: a small JSON summary under the run's own
/// prefix, the human-facing record of how far the run got and what it
/// concluded.
async fn write_stage_artifact(
    bucket: &Bucket,
    budget: &mut Budget,
    run_id: &str,
    stage: &str,
    summary: &serde_json::Value,
) -> Result<()> {
    budget.spend();
    let body = format!(
        "{}\n",
        serde_json::to_string_pretty(summary).expect("stage summaries serialize")
    );
    bucket
        .put(format!("{GC_RUNS_KEY_PREFIX}{run_id}/{stage}.json"), body)
        .execute()
        .await?;
    Ok(())
}

/// Aborted uploads: listing-bound reads of each record, and a resume-abort
/// pair per stale one. Leftovers wait for the next run, because the
/// reaper is work-conserving, not all-at-once.
async fn reap_stale_uploads(bucket: &Bucket, budget: &mut Budget, now: UnixMillis) -> u64 {
    let Ok(records) = full_listing(bucket, budget, UPLOADS_KEY_PREFIX).await else {
        return 0;
    };
    let mut aborted = 0_u64;
    for record_object in records {
        if budget.spent_out(now_ms()) {
            break;
        }
        // why: listed objects are metadata only; the record needs its own
        // get.
        budget.spend();
        let Ok(Some(full)) = bucket.get(record_object.key()).execute().await else {
            continue;
        };
        let Some(body) = full.body() else { continue };
        let Ok(text) = body.text().await else {
            continue;
        };
        let Ok(record) = UploadRecord::parse(&text) else {
            continue;
        };
        if now.saturating_ms_since(UnixMillis::new(record.created_at_ms)) < UPLOAD_STALE_MAX_MS {
            continue;
        }
        let upload_id = record_object
            .key()
            .strip_prefix(UPLOADS_KEY_PREFIX)
            .unwrap_or_default()
            .to_string();
        budget.spend();
        if let Ok(upload) = bucket.resume_multipart_upload(&record.key, &upload_id) {
            let _ = upload.abort().await;
        }
        budget.spend();
        let _ = bucket.delete(record_object.key()).await;
        aborted += 1;
    }
    aborted
}

/// Prune finished run artifacts older than their retention: the run id
/// begins with its start time, so age answers from the key alone.
async fn prune_old_runs(bucket: &Bucket, budget: &mut Budget, now: UnixMillis) {
    let Ok(objects) = full_listing(bucket, budget, GC_RUNS_KEY_PREFIX).await else {
        return;
    };
    let mut stale = Vec::new();
    for object in objects {
        let key = object.key();
        let Some(suffix) = key.strip_prefix(GC_RUNS_KEY_PREFIX) else {
            continue;
        };
        let started: u64 = suffix
            .split('-')
            .next()
            .and_then(|digits| digits.parse().ok())
            .unwrap_or(u64::MAX);
        if now.saturating_ms_since(UnixMillis::new(started)) > GC_RUNS_RETENTION_MS {
            stale.push(object.key());
        }
    }
    for batch in stale.chunks(GC_DELETE_BATCH) {
        if budget.spent_out(now_ms()) {
            return;
        }
        budget.spend();
        let _ = bucket.delete_multiple(batch.to_vec()).await;
    }
}

/// End a run, swept or aborted: the report under its reports key, the
/// cursor gone, and the log line that names the conclusion.
async fn finish_run(
    bucket: &Bucket,
    budget: &mut Budget,
    cursor: &GcCursor,
    report: GcReport,
    gate: Option<&GateTrip>,
) -> Result<()> {
    let event = if gate.is_some() {
        "gc.run_aborted"
    } else {
        "gc.run_finished"
    };
    if let Some(trip) = gate {
        log::event(
            "error",
            "gc.gate_tripped",
            &[
                ("runId", cursor.run_id.clone()),
                ("gate", trip.name().to_string()),
                ("detail", trip.to_string()),
            ],
        );
    }
    budget.spend();
    bucket
        .put(
            format!("{GC_REPORTS_KEY_PREFIX}{}.json", cursor.run_id),
            report.serialize(),
        )
        .execute()
        .await?;
    budget.spend();
    let _ = bucket.delete(GC_CURSOR_OBJECT_KEY).await;
    log::event(
        "info",
        event,
        &[
            ("runId", cursor.run_id.clone()),
            ("narinfosDeleted", report.narinfos_deleted.to_string()),
            ("narsDeleted", report.nars_deleted.to_string()),
            ("bytesFreed", report.bytes_freed.to_string()),
        ],
    );
    Ok(())
}

/// Assemble the report from the run's counters.
fn build_report(cursor: &GcCursor, finished_at_ms: u64, gate: Option<&GateTrip>) -> GcReport {
    GcReport {
        run_id: cursor.run_id.clone(),
        started_at_ms: cursor.started_at_ms,
        finished_at_ms,
        inventory_paths: cursor.inventory_paths,
        active_leases: cursor.active_leases,
        marked_paths: cursor.marked_paths,
        unreadable_deep: cursor.unreadable_deep,
        narinfos_deleted: cursor.sweep.as_ref().map_or(0, |s| s.narinfos_deleted),
        nars_deleted: cursor.sweep.as_ref().map_or(0, |s| s.nars_deleted),
        bytes_freed: cursor.sweep.as_ref().map_or(0, |s| s.bytes_freed),
        uploads_aborted: cursor.uploads_aborted,
        gate: gate.map(GateTrip::name).map(str::to_string),
    }
}

/// The scheduled entry point. A run is quiet in every direction: facts
/// live in its artifacts and report, and the event log names only starts,
/// pauses, gates, and finishes.
pub async fn drive(env: &Env) -> Result<()> {
    let start = now_ms();
    if !armed(env) {
        log::event("info", "gc.disarmed", &[]);
        return Ok(());
    }
    let grace_ms = grace_window_ms(env);
    let mut budget = Budget::new(start);
    let bucket = env.bucket("CACHE_BUCKET")?;

    let mut cursor = match read_cursor(&bucket, &mut budget).await {
        Ok(Some(cursor)) => cursor,
        Ok(None) => fresh_cursor(start),
        Err(failure) => {
            // A cursor that will not read must not block the collector
            // forever: stages recompute and deletion ordering is what
            // protects objects, so the run starts over, loudly.
            log::event(
                "error",
                "gc.cursor_corrupt",
                &[("error", failure.to_string())],
            );
            fresh_cursor(start)
        }
    };
    log::event(
        "info",
        "gc.run_started",
        &[
            ("runId", cursor.run_id.clone()),
            ("stage", stage_name(cursor.stage).to_string()),
            ("ops", budget.ops.to_string()),
        ],
    );

    if matches!(
        cursor.stage,
        GcStage::Inventory | GcStage::Leases | GcStage::Mark | GcStage::Candidates
    ) {
        match fresh_stages(&bucket, &mut budget, &mut cursor, grace_ms).await? {
            FreshVerdict::Ready => {}
            FreshVerdict::Frozen | FreshVerdict::Aborted => return Ok(()),
        }
    }

    if cursor.stage == GcStage::Sweep && !run_sweep_stage(&bucket, &mut budget, &mut cursor).await?
    {
        return Ok(());
    }

    if cursor.stage == GcStage::Report {
        finish_report_stage(&bucket, &mut budget, &mut cursor).await?;
    }

    Ok(())
}

/// The fresh half of a tick: inventory, leases, the walk, the collect,
/// and the plan. Extraction from `drive` keeps each half small enough to
/// read as one screen of code.
async fn fresh_stages(
    bucket: &Bucket,
    budget: &mut Budget,
    cursor: &mut GcCursor,
    grace_ms: u64,
) -> Result<FreshVerdict> {
    if budget.spent_out(now_ms()) {
        freeze(bucket, budget, cursor).await?;
        return Ok(FreshVerdict::Frozen);
    }
    // A listing that cannot fit one tick trips the gate: pausing forever
    // would alarm nobody.
    let Ok(objects) = full_listing(bucket, budget, "").await else {
        let trip = GateTrip::InventoryTruncated;
        let report = build_report(cursor, now_ms(), Some(&trip));
        finish_run(bucket, budget, cursor, report, Some(&trip)).await?;
        return Ok(FreshVerdict::Aborted);
    };
    let inventory: Vec<InventoryItem> = objects
        .iter()
        .map(|object| InventoryItem {
            key: object.key(),
            size_bytes: object.size(),
            uploaded_at_ms: object.uploaded().as_millis(),
        })
        .collect();
    cursor.inventory_paths = count_parseable_narinfos(&inventory);
    write_stage_artifact(
        bucket,
        budget,
        &cursor.run_id,
        "inventory",
        &serde_json::json!({
            "objects": inventory.len(),
            "narinfos": cursor.inventory_paths,
            "finishedAtMs": now_ms(),
        }),
    )
    .await?;

    if budget.spent_out(now_ms()) {
        freeze(bucket, budget, cursor).await?;
        return Ok(FreshVerdict::Frozen);
    }
    let (active, roots) = match stage_leases(bucket, budget, UnixMillis::new(now_ms())).await {
        Ok(found) => found,
        Err(trip) => {
            let report = build_report(cursor, now_ms(), Some(&trip));
            finish_run(bucket, budget, cursor, report, Some(&trip)).await?;
            return Ok(FreshVerdict::Aborted);
        }
    };
    cursor.active_leases = active;
    if matches!(cursor.stage, GcStage::Inventory | GcStage::Leases) {
        cursor.stage = GcStage::Mark;
    }

    if cursor.stage == GcStage::Mark {
        match run_mark_stage(bucket, budget, cursor, &roots).await? {
            MarkVerdict::Done(outcome) => {
                apply_outcome(cursor, &outcome);
                write_stage_artifact(
                    bucket,
                    budget,
                    &cursor.run_id,
                    "mark",
                    &serde_json::json!({
                        "markedPaths": cursor.marked_paths,
                        "unreadableDeep": cursor.unreadable_deep,
                        "finishedAtMs": now_ms(),
                    }),
                )
                .await?;
            }
            MarkVerdict::Frozen => return Ok(FreshVerdict::Frozen),
            MarkVerdict::Aborted => return Ok(FreshVerdict::Aborted),
        }
    }

    // Candidates resume from the frozen completed marks: reached either
    // by falling through or by a tick that started here.
    let outcome = cursor
        .mark
        .as_ref()
        .map(outcome_from_completed_mark)
        .expect("a candidates stage has completed marks");
    if budget.spent_out(now_ms()) {
        freeze(bucket, budget, cursor).await?;
        return Ok(FreshVerdict::Frozen);
    }
    match run_collect_and_plan(bucket, budget, cursor, &inventory, &outcome, grace_ms).await? {
        PlanVerdict::Ready => Ok(FreshVerdict::Ready),
        PlanVerdict::Frozen => Ok(FreshVerdict::Frozen),
        PlanVerdict::Aborted => Ok(FreshVerdict::Aborted),
    }
}

/// How the fresh half of a tick ended.
enum FreshVerdict {
    /// The plan is frozen and the sweep may begin.
    Ready,
    /// The invocation parked.
    Frozen,
    /// A gate tripped; the run ended inside.
    Aborted,
}

/// Narinfo-shaped keys whose hash half parses: the report's path count.
fn count_parseable_narinfos(inventory: &[InventoryItem]) -> u64 {
    inventory
        .iter()
        .filter(|item| {
            item.key
                .ends_with(cachet_core::constants::NARINFO_KEY_SUFFIX)
                && StorePathHash::parse(
                    &item.key[..item.key.len() - cachet_core::constants::NARINFO_KEY_SUFFIX.len()],
                )
                .is_ok()
        })
        .count() as u64
}

/// Rebuild the walk's accounting from its frozen completed form.
fn outcome_from_completed_mark(mark: &cachet_core::gc::MarkCursor) -> WalkOutcome {
    WalkOutcome {
        marked: mark
            .visited
            .iter()
            .filter_map(|text| StorePathHash::parse(text).ok())
            .collect(),
        marked_urls: mark
            .marked_urls
            .iter()
            .filter_map(|(hash, url)| {
                Some((StorePathHash::parse(hash).ok()?, parse_nar_key(url).ok()?))
            })
            .collect(),
        unreadable_deep: usize::try_from(mark.unreadable_deep).unwrap_or(usize::MAX),
        gate: None,
    }
}

/// One walk's end, distinct from a pause and from a gate.
enum MarkVerdict {
    /// The outcome is complete and gate-free.
    Done(Box<WalkOutcome>),
    /// The invocation parked mid-walk or at the stage boundary.
    Frozen,
    /// A gate tripped; the run was aborted inside the stage.
    Aborted,
}

/// Ready to sweep, parked, or aborted.
enum PlanVerdict {
    Ready,
    Frozen,
    Aborted,
}

/// The mark stage: walk from this tick's roots or from the frozen
/// frontier, freezing at the budget's word.
async fn run_mark_stage(
    bucket: &Bucket,
    budget: &mut Budget,
    cursor: &mut GcCursor,
    roots: &[StorePathHash],
) -> Result<MarkVerdict> {
    let mut walker = cursor
        .mark
        .as_ref()
        .filter(|mark| !mark.frontier.is_empty())
        .map_or_else(
            || ClosureWalker::new(roots),
            ClosureWalker::from_mark_cursor,
        );
    loop {
        if let Some(trip) = walker.gate().cloned() {
            let report = build_report(cursor, now_ms(), Some(&trip));
            finish_run(bucket, budget, cursor, report, Some(&trip)).await?;
            return Ok(MarkVerdict::Aborted);
        }
        let Some(hash) = walker.next_read() else {
            break;
        };
        if budget.spent_out(now_ms()) {
            cursor.mark = Some(walker.to_mark_cursor());
            freeze(bucket, budget, cursor).await?;
            return Ok(MarkVerdict::Frozen);
        }
        match read_walk_narinfo(bucket, budget, &hash).await {
            WalkFetch::Document(document) => walker.answer(hash, Ok(*document)),
            WalkFetch::Missing => walker.answer(hash, Err(WalkReadError::Absent)),
            WalkFetch::Corrupt => walker.answer(hash, Err(WalkReadError::Unparseable)),
            WalkFetch::Outage(failure) => {
                log::event(
                    "error",
                    "gc.walk_read_failed",
                    &[("hash", hash.to_string()), ("error", failure.to_string())],
                );
                cursor.mark = Some(walker.to_mark_cursor());
                freeze(bucket, budget, cursor).await?;
                return Ok(MarkVerdict::Frozen);
            }
        }
    }
    Ok(MarkVerdict::Done(Box::new(walker.into_outcome())))
}

/// Move the outcome's facts into the cursor and freeze the marks: the
/// completed walk keeps its marks under `cursor.mark` with an empty
/// frontier, so a later tick reuses them without rewalking.
fn apply_outcome(cursor: &mut GcCursor, outcome: &WalkOutcome) {
    cursor.marked_paths = u64::try_from(outcome.marked.len()).unwrap_or(u64::MAX);
    cursor.unreadable_deep = u64::try_from(outcome.unreadable_deep).unwrap_or(u64::MAX);
    cursor.mark = Some(completed_mark(outcome));
    cursor.stage = GcStage::Candidates;
}

/// The completed walk as its frozen form. Roots stay recorded because the
/// document states which set protected these paths.
fn completed_mark(outcome: &WalkOutcome) -> cachet_core::gc::MarkCursor {
    cachet_core::gc::MarkCursor {
        roots: Vec::new(),
        visited: outcome.marked.iter().map(ToString::to_string).collect(),
        frontier: Vec::new(),
        marked_urls: outcome
            .marked_urls
            .iter()
            .map(|(hash, url)| (hash.to_string(), url.as_str().to_string()))
            .collect(),
        unreadable_deep: outcome.unreadable_deep as u64,
    }
}

/// The collect stage's reads plus the plan: one narinfo fetch per
/// uncollected candidate, then the pure decision. The plan freezes before
/// execution touches the bucket.
async fn run_collect_and_plan(
    bucket: &Bucket,
    budget: &mut Budget,
    cursor: &mut GcCursor,
    inventory: &[InventoryItem],
    outcome: &WalkOutcome,
    grace_ms: u64,
) -> Result<PlanVerdict> {
    let now = UnixMillis::new(now_ms());
    let candidates = sweep_candidates(inventory, &outcome.marked, now, grace_ms);
    let mut collect = cursor.collect.take().unwrap_or_default();
    let mut collected: BTreeMap<String, CollectedUrl> = collect
        .collected
        .drain(..)
        .map(|entry| (entry.hash.clone(), entry))
        .collect();

    for (hash, _key) in &candidates {
        if collected.contains_key(&hash.to_string()) {
            continue;
        }
        if budget.spent_out(now_ms()) {
            cursor.collect = Some(CollectCursor {
                next_index: collected.len() as u64,
                collected: collected.into_values().collect(),
            });
            freeze(bucket, budget, cursor).await?;
            return Ok(PlanVerdict::Frozen);
        }
        match read_walk_narinfo(bucket, budget, hash).await {
            WalkFetch::Document(document) => {
                collected.insert(
                    hash.to_string(),
                    CollectedUrl {
                        hash: hash.to_string(),
                        url: document.url.as_str().to_string(),
                    },
                );
            }
            // A candidate that cannot be read yields no URL: its key still
            // deletes as a narinfo, and the NAR stays for the same reason
            // it stayed before this run.
            WalkFetch::Missing | WalkFetch::Corrupt => {}
            WalkFetch::Outage(_) => {
                cursor.collect = Some(CollectCursor {
                    next_index: collected.len() as u64,
                    collected: collected.into_values().collect(),
                });
                freeze(bucket, budget, cursor).await?;
                return Ok(PlanVerdict::Frozen);
            }
        }
    }

    let candidate_urls: Vec<(StorePathHash, NarKey)> = candidates
        .iter()
        .filter_map(|(hash, _)| {
            collected
                .get(&hash.to_string())
                .and_then(|entry| parse_nar_key(&entry.url).ok())
                .map(|url| (hash.clone(), url))
        })
        .collect();
    let plan = plan_deletions(
        inventory,
        &outcome.marked,
        &outcome.marked_urls,
        &candidate_urls,
        now,
        grace_ms,
    );
    if let Some(trip) = plan.gate.clone() {
        let report = build_report(cursor, now_ms(), Some(&trip));
        finish_run(bucket, budget, cursor, report, Some(&trip)).await?;
        return Ok(PlanVerdict::Aborted);
    }
    cursor.collect = None;
    cursor.sweep = Some(plan_into_sweep(&plan, inventory));
    cursor.stage = GcStage::Sweep;
    write_stage_artifact(
        bucket,
        budget,
        &cursor.run_id,
        "plan",
        &serde_json::json!({
            "narinfoDeletes": cursor.sweep.as_ref().expect("just set").narinfo_deletes.len(),
            "narDeletes": cursor.sweep.as_ref().expect("just set").nar_deletes.len(),
            "candidates": candidates.len(),
            "finishedAtMs": now_ms(),
        }),
    )
    .await?;
    Ok(PlanVerdict::Ready)
}

/// The persisted plan: the two ordered key lists and the size map the
/// freed-bytes accounting reads from.
fn plan_into_sweep(
    plan: &cachet_core::gc::DeletionPlan,
    inventory: &[InventoryItem],
) -> SweepCursor {
    let bytes_by_key: BTreeMap<String, u64> = inventory
        .iter()
        .map(|item| (item.key.clone(), item.size_bytes))
        .collect();
    SweepCursor {
        narinfo_deletes: plan.narinfo_deletes.clone(),
        nar_deletes: plan.nar_deletes.clone(),
        narinfos_deleted: 0,
        nars_deleted: 0,
        bytes_freed: 0,
        bytes_by_key,
    }
}

/// The sweep: narinfo batches, then NAR batches, at the plan's own pace.
/// Narinfos go first at every instant, so the bucket always serves a
/// subset of a coherent earlier state. Returns true when nothing remains.
async fn run_sweep_stage(
    bucket: &Bucket,
    budget: &mut Budget,
    cursor: &mut GcCursor,
) -> Result<bool> {
    let Some(mut sweep) = cursor.sweep.take() else {
        cursor.stage = GcStage::Report;
        return Ok(true);
    };
    // why: the cursor's counters are wire-shaped u64 while the slices
    // want usize; counts are bounded by the plan, which is bounded by the
    // inventory, and always fit memory.
    let mut narinfos_done =
        usize::try_from(sweep.narinfos_deleted).expect("deletion progress fits memory");
    while narinfos_done < sweep.narinfo_deletes.len() {
        if budget.spent_out(now_ms()) {
            sweep.narinfos_deleted = narinfos_done as u64;
            cursor.sweep = Some(sweep);
            freeze(bucket, budget, cursor).await?;
            return Ok(false);
        }
        let end = (narinfos_done + GC_DELETE_BATCH).min(sweep.narinfo_deletes.len());
        budget.spend();
        bucket
            .delete_multiple(sweep.narinfo_deletes[narinfos_done..end].to_vec())
            .await?;
        sweep.bytes_freed += sweep.narinfo_deletes[narinfos_done..end]
            .iter()
            .map(|key| sweep.bytes_by_key.get(key).copied().unwrap_or(0))
            .sum::<u64>();
        narinfos_done = end;
    }
    sweep.narinfos_deleted = narinfos_done as u64;

    let mut nars_done = usize::try_from(sweep.nars_deleted).expect("deletion progress fits memory");
    while nars_done < sweep.nar_deletes.len() {
        if budget.spent_out(now_ms()) {
            sweep.nars_deleted = nars_done as u64;
            cursor.sweep = Some(sweep);
            freeze(bucket, budget, cursor).await?;
            return Ok(false);
        }
        let end = (nars_done + GC_DELETE_BATCH).min(sweep.nar_deletes.len());
        budget.spend();
        bucket
            .delete_multiple(sweep.nar_deletes[nars_done..end].to_vec())
            .await?;
        sweep.bytes_freed += sweep.nar_deletes[nars_done..end]
            .iter()
            .map(|key| sweep.bytes_by_key.get(key).copied().unwrap_or(0))
            .sum::<u64>();
        nars_done = end;
    }
    sweep.nars_deleted = nars_done as u64;

    cursor.sweep = Some(sweep);
    cursor.stage = GcStage::Report;
    Ok(true)
}

/// The report stage: generation bump when deletions happened, the reaper
/// and the pruner inside the remaining budget, then the report itself and
/// the cursor's deletion.
async fn finish_report_stage(
    bucket: &Bucket,
    budget: &mut Budget,
    cursor: &mut GcCursor,
) -> Result<()> {
    let deletions = cursor
        .sweep
        .as_ref()
        .map_or(0, |s| s.narinfo_deletes.len() + s.nar_deletes.len());
    if deletions > 0 {
        if budget.spent_out(now_ms()) {
            return freeze(bucket, budget, cursor).await;
        }
        match bump_generation(bucket, budget).await {
            Ok(()) => {}
            Err(trip) => {
                // The sweep already executed; the report must still land.
                let report = build_report(cursor, now_ms(), Some(&trip));
                return finish_run(bucket, budget, cursor, report, Some(&trip)).await;
            }
        }
    }
    cursor.uploads_aborted = reap_stale_uploads(bucket, budget, UnixMillis::new(now_ms())).await;
    prune_old_runs(bucket, budget, UnixMillis::new(now_ms())).await;
    let report = build_report(cursor, now_ms(), None);
    finish_run(bucket, budget, cursor, report, None).await
}

/// The generation bump: the whole point of sweeping under an edge cache.
async fn bump_generation(
    bucket: &Bucket,
    budget: &mut Budget,
) -> std::result::Result<(), GateTrip> {
    budget.spend();
    let object = bucket
        .get(GENERATION_OBJECT_KEY)
        .execute()
        .await
        .map_err(|_| GateTrip::GenerationCorrupt)?;
    let current = match object {
        None => GenerationDocument::ZERO,
        Some(object) => {
            let Some(body) = object.body() else {
                return Err(GateTrip::GenerationCorrupt);
            };
            let text = body.text().await.map_err(|_| GateTrip::GenerationCorrupt)?;
            GenerationDocument::parse(&text).map_err(|_| GateTrip::GenerationCorrupt)?
        }
    };
    budget.spend();
    bucket
        .put(GENERATION_OBJECT_KEY, current.bump(now_ms()).serialize())
        .execute()
        .await
        .map_err(|_| GateTrip::GenerationCorrupt)?;
    Ok(())
}
