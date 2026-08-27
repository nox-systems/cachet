//! The collector's pure algorithm (CLAUDE.md §4): closure walk, gates,
//! deletion planning, and the run report. Everything destructive is decided
//! here as data, over injected inputs, so every catastrophic shape has a
//! test before it has a deployment: the driver in cachet-worker only
//! executes what these functions return.
//!
//! The laws: marked is never swept, reserved keys are never swept, narinfos
//! delete before NARs, and any gate trip aborts the run with nothing
//! deleted.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::constants::CLOSURE_WALK_PATHS_MAX;
use crate::keys::{NarKey, is_reserved_key};
use crate::narinfo::Narinfo;
use crate::types::{StorePathHash, UnixMillis};

/// A gate trip: the run aborts and deletes nothing. Any of these means the
/// picture of the cache was untrustworthy in a way that makes deletion
/// unsafe, and the report names it so the run is debuggable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateTrip {
    /// A root store path whose narinfo could not be read: the walk would
    /// see less than one lease protects.
    UnreadableRootNarinfo {
        /// The root hash that failed to read.
        hash: StorePathHash,
    },
    /// A lease document that will not parse: its protection is unknown.
    UnparseableLease {
        /// The lease's bucket key.
        key: String,
    },
    /// The R2 listing could not be fully enumerated in this run.
    InventoryTruncated,
    /// The project lease listing could not be fully enumerated.
    LeasesTruncated,
    /// The closure walk hit its visited-path cap.
    WalkBudgetExhausted {
        /// The number of paths visited when the cap tripped.
        visited: usize,
    },
    /// The generation document is corrupt: sweeping without a reliable
    /// epoch would leave the edge serving deleted objects.
    GenerationCorrupt,
}

impl core::fmt::Display for GateTrip {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnreadableRootNarinfo { hash } => write!(f, "unreadable root narinfo {hash}"),
            Self::UnparseableLease { key } => write!(f, "unparseable lease {key}"),
            Self::InventoryTruncated => f.write_str("inventory truncated"),
            Self::LeasesTruncated => f.write_str("leases truncated"),
            Self::WalkBudgetExhausted { visited } => {
                write!(f, "walk budget exhausted at {visited} paths")
            }
            Self::GenerationCorrupt => f.write_str("generation document corrupt"),
        }
    }
}

impl GateTrip {
    /// The stable name the report records: machine-matchable, unlike the
    /// Display text, which carries occurrence specifics.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::UnreadableRootNarinfo { .. } => "unreadable_root_narinfo",
            Self::UnparseableLease { .. } => "unparseable_lease",
            Self::InventoryTruncated => "inventory_truncated",
            Self::LeasesTruncated => "leases_truncated",
            Self::WalkBudgetExhausted { .. } => "walk_budget_exhausted",
            Self::GenerationCorrupt => "generation_corrupt",
        }
    }
}

/// Why a narinfo read failed during the walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkReadError {
    /// The narinfo object is absent (a dangling root or reference).
    Absent,
    /// The object exists but does not parse.
    Unparseable,
}

/// The pure outcome of the closure walk over one root set.
#[derive(Debug, Clone, Default)]
pub struct WalkOutcome {
    /// Every reachable store-path hash.
    pub marked: BTreeSet<StorePathHash>,
    /// For each readable marked path, the NAR key its narinfo named. The
    /// URL set derived from this map is what makes shared-NAR sweeps safe:
    /// a NAR survives if any marked narinfo still names it.
    pub marked_urls: BTreeMap<StorePathHash, NarKey>,
    /// Deep references whose own narinfo could not be read: they are
    /// marked without descent and counted for the report.
    pub unreadable_deep: usize,
    /// The gate a run aborts on, when one tripped.
    pub gate: Option<GateTrip>,
}

/// The closure walk as an incremental state machine: `next_read` names
/// the path whose narinfo must be read, `answer` applies the read. The worker
/// awaits one bucket read per step this way, and a run can freeze the
/// walker into its cursor and resume the next tick with the visited set,
/// the frontier order, and the marks bit-identical.
#[derive(Debug, Clone)]
pub struct ClosureWalker {
    roots: BTreeSet<StorePathHash>,
    visited: BTreeSet<StorePathHash>,
    queue: VecDeque<StorePathHash>,
    outcome: WalkOutcome,
}

impl ClosureWalker {
    /// Start a walk from the root set, sorted so the walk order never
    /// depends on how the roots arrived.
    pub fn new(roots: &[StorePathHash]) -> Self {
        let mut walker = Self {
            roots: roots.iter().cloned().collect(),
            visited: BTreeSet::new(),
            queue: VecDeque::new(),
            outcome: WalkOutcome::default(),
        };
        let mut sorted = roots.to_vec();
        sorted.sort();
        walker.queue.extend(sorted);
        walker
    }

    /// Restore a frozen walker from a cursor; cursor prefixes clamp the
    /// continuation to the same deterministic order the frozen run had.
    #[must_use]
    pub fn from_mark_cursor(cursor: &MarkCursor) -> Self {
        let roots: Vec<StorePathHash> = cursor
            .roots
            .iter()
            .filter_map(|text| StorePathHash::parse(text).ok())
            .collect();
        let mut walker = Self::new(&roots);
        walker.visited = cursor
            .visited
            .iter()
            .filter_map(|text| StorePathHash::parse(text).ok())
            .collect();
        walker.queue = cursor
            .frontier
            .iter()
            .filter_map(|text| StorePathHash::parse(text).ok())
            .collect();
        walker.outcome.marked = walker.visited.clone();
        walker.outcome.marked_urls = cursor
            .marked_urls
            .iter()
            .filter_map(|(hash, url)| {
                Some((
                    StorePathHash::parse(hash).ok()?,
                    crate::keys::parse_nar_key(url).ok()?,
                ))
            })
            .collect();
        walker.outcome.unreadable_deep =
            usize::try_from(cursor.unreadable_deep).unwrap_or(usize::MAX);
        walker
    }

    /// Freeze the walker into its cursor form.
    #[must_use]
    pub fn to_mark_cursor(&self) -> MarkCursor {
        MarkCursor {
            roots: self.roots.iter().map(ToString::to_string).collect(),
            visited: self.visited.iter().map(ToString::to_string).collect(),
            frontier: self.queue.iter().map(ToString::to_string).collect(),
            marked_urls: self
                .outcome
                .marked_urls
                .iter()
                .map(|(hash, url)| (hash.to_string(), url.as_str().to_string()))
                .collect(),
            unreadable_deep: self.outcome.unreadable_deep as u64,
        }
    }

    /// The hash whose narinfo must be read next, or `None` when the walk is
    /// done. Already-visited front names are skipped over without budget:
    /// duplicates from converging fans cost a queue pop, never a read.
    pub fn next_read(&mut self) -> Option<StorePathHash> {
        while let Some(hash) = self.queue.pop_front() {
            if !self.visited.contains(&hash) {
                return Some(hash);
            }
        }
        None
    }

    /// Apply the read for the hash `next_read` just handed out.
    pub fn answer(
        &mut self,
        hash: StorePathHash,
        read: std::result::Result<Narinfo, WalkReadError>,
    ) {
        self.visited.insert(hash.clone());
        if self.visited.len() > CLOSURE_WALK_PATHS_MAX {
            self.outcome.gate = Some(GateTrip::WalkBudgetExhausted {
                visited: self.visited.len(),
            });
            return;
        }
        if let Ok(document) = read {
            let url = document.url.clone();
            for edge in document.reference_hashes() {
                if !self.visited.contains(&edge) {
                    self.queue.push_back(edge);
                }
            }
            self.outcome.marked.insert(hash.clone());
            self.outcome.marked_urls.insert(hash, url);
        } else {
            // Marking happens for either failure, so the sweep never
            // touches this path. The failure decides only whether the
            // walk goes on.
            self.outcome.marked.insert(hash.clone());
            let absent = read == Err(WalkReadError::Absent);
            if self.roots.contains(&hash) && !absent {
                // why: only unparseable. A root whose narinfo will not
                // parse is present and servable, and its references
                // cannot be enumerated, so continuing would sweep a
                // closure whose top a client still reaches.
                self.outcome.gate = Some(GateTrip::UnreadableRootNarinfo { hash });
                return;
            }
            // why: an absent root does not gate, and used to. A lease
            // naming a path whose narinfo is gone describes something
            // this cache no longer holds, and no client can substitute
            // it, because a substitution starts by fetching that
            // narinfo. Refusing there protected nothing and was
            // permanent: the lease keeps naming the path, the narinfo
            // stays gone, and every run tripped until somebody pushed
            // the project again or edited the lease by hand. Counting it
            // matches how an absent reference deeper in the closure has
            // always been handled (ADR 0018).
            self.outcome.unreadable_deep += 1;
        }
    }

    /// The walk's final accounting.
    #[must_use]
    pub fn into_outcome(self) -> WalkOutcome {
        self.outcome
    }

    /// The gate tripped so far, if any: checked after every answer by an
    /// async driver that must stop before asking for another read.
    pub fn gate(&self) -> Option<&GateTrip> {
        self.outcome.gate.as_ref()
    }
}

/// Walk the reference graph from the root hashes, breadth-first, one
/// narinfo read per newly visited path. Root read failures trip the
/// walk-aborting gate; deep failures mark the node without descending,
/// because the alternative, deleting under it, would eat descendants we
/// cannot see.
pub fn closure_walk(
    roots: &[StorePathHash],
    mut read_narinfo: impl FnMut(&StorePathHash) -> std::result::Result<Narinfo, WalkReadError>,
) -> WalkOutcome {
    let mut walker = ClosureWalker::new(roots);
    while walker.outcome.gate.is_none() {
        let Some(hash) = walker.next_read() else {
            break;
        };
        let read = read_narinfo(&hash);
        walker.answer(hash, read);
    }
    walker.into_outcome()
}

/// One object from the bucket listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryItem {
    /// The bucket key.
    pub key: String,
    /// Its size in bytes, for the freed-space report.
    pub size_bytes: u64,
    /// Its upload time, in epoch milliseconds, for the grace window.
    pub uploaded_at_ms: u64,
}

/// The destructive plan: ordered, explicit, and empty whenever a gate
/// tripped. Execution deletes `narinfo_deletes` first, so at every instant
/// the cache serves a subset of a coherent earlier state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeletionPlan {
    /// Narinfo keys to delete, all of them before any NAR.
    pub narinfo_deletes: Vec<String>,
    /// NAR keys to delete after every narinfo is gone.
    pub nar_deletes: Vec<String>,
    /// The gate that aborted planning, if any. A tripped plan is always
    /// empty; the report records the trip.
    pub gate: Option<GateTrip>,
}

/// The sweep's candidate narinfos: unmarked and past the grace window.
/// Extracted as one predicate because two consumers must agree exactly:
/// the planner, which orders their deletion, and the driver, which reads
/// each candidate's URL before the plan can name NARs.
pub fn sweep_candidates(
    inventory: &[InventoryItem],
    marked: &BTreeSet<StorePathHash>,
    now: UnixMillis,
    grace_ms: u64,
) -> Vec<(StorePathHash, String)> {
    let mut candidates = Vec::new();
    for item in inventory {
        if is_reserved_key(&item.key) || !item.key.ends_with(crate::constants::NARINFO_KEY_SUFFIX) {
            continue;
        }
        let Ok(hash) = StorePathHash::parse(
            &item.key[..item.key.len() - crate::constants::NARINFO_KEY_SUFFIX.len()],
        ) else {
            // Unnameable narinfo-shaped object: it cannot map to a
            // lease-protected path either way, so it is left alone.
            continue;
        };
        if !marked.contains(&hash)
            && now.saturating_ms_since(UnixMillis::new(item.uploaded_at_ms)) > grace_ms
        {
            candidates.push((hash, item.key.clone()));
        }
    }
    candidates.sort_by(|(a, _), (b, _)| a.cmp(b));
    candidates
}

/// Decide the destructive plan.
///
/// Candidates come from [`sweep_candidates`]. NAR deletions resolve from
/// the candidate narinfos' own URLs minus every URL a marked narinfo
/// still names, so a NAR shared between a live and a dead path survives.
///
/// How much one run may delete is not bounded, and used to be. A gate
/// refusing any sweep past a quarter of the inventory could not be
/// satisfied by the run after it either: refusing deleted nothing, so the
/// next run saw the same inventory and the same candidates and refused
/// again, and a deployment with a lot of genuinely dead paths stopped
/// collecting permanently. The gates that remain fire when the collector
/// cannot see the truth, which is the condition worth refusing on
/// (ADR 0017).
pub fn plan_deletions(
    inventory: &[InventoryItem],
    marked: &BTreeSet<StorePathHash>,
    marked_urls: &BTreeMap<StorePathHash, NarKey>,
    candidate_urls: &[(StorePathHash, NarKey)],
    now: UnixMillis,
    grace_ms: u64,
) -> DeletionPlan {
    let mut plan = DeletionPlan::default();

    let narinfo_deletes: Vec<String> = sweep_candidates(inventory, marked, now, grace_ms)
        .into_iter()
        .map(|(_, key)| key)
        .collect();

    plan.narinfo_deletes = narinfo_deletes;

    let live_urls: BTreeSet<&str> = marked_urls.values().map(NarKey::as_str).collect();
    let mut nar_deletes: Vec<String> = candidate_urls
        .iter()
        .filter(|(_, url)| !live_urls.contains(url.as_str()))
        .map(|(_, url)| url.as_str().to_string())
        .collect();
    nar_deletes.sort();
    nar_deletes.dedup();
    plan.nar_deletes = nar_deletes;

    plan
}

/// The run report: what one collector cycle concluded, whether it swept or
/// aborted. Golden-locked; the `/api/self/gc-runs` surface serves these
/// verbatim.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GcReport {
    /// The run's id.
    #[serde(rename = "runId")]
    pub run_id: String,
    /// When the run started.
    #[serde(rename = "startedAtMs")]
    pub started_at_ms: u64,
    /// When the run finished (or aborted).
    #[serde(rename = "finishedAtMs")]
    pub finished_at_ms: u64,
    /// Narinfos in the inventory.
    #[serde(rename = "inventoryPaths")]
    pub inventory_paths: u64,
    /// Active leases consulted.
    #[serde(rename = "activeLeases")]
    pub active_leases: u64,
    /// Paths marked reachable.
    #[serde(rename = "markedPaths")]
    pub marked_paths: u64,
    /// Narinfo reads that failed during the walk without stopping it: a
    /// reference at any depth, and a root whose narinfo is absent
    /// (ADR 0018).
    #[serde(rename = "unreadableDeep")]
    pub unreadable_deep: u64,
    /// Narinfo objects deleted.
    #[serde(rename = "narinfosDeleted")]
    pub narinfos_deleted: u64,
    /// NAR objects deleted.
    #[serde(rename = "narsDeleted")]
    pub nars_deleted: u64,
    /// Bytes freed, by listing sizes.
    #[serde(rename = "bytesFreed")]
    pub bytes_freed: u64,
    /// Stale multipart uploads aborted.
    #[serde(rename = "uploadsAborted")]
    pub uploads_aborted: u64,
    /// The gate that aborted the run, if one tripped, as its name.
    pub gate: Option<String>,
}

impl GcReport {
    /// Serialize the report the way the driver stores it: two-space
    /// indent and a trailing newline, so a diff between two runs of a
    /// debugging session stays legible.
    pub fn serialize(&self) -> String {
        let mut body = serde_json::to_string_pretty(self).expect("the report fields serialize");
        body.push('\n');
        body
    }

    /// Read a stored report back, for the API surface. Field-missing
    /// documents fail rather than fabricate numbers.
    ///
    /// # Errors
    ///
    /// [`ClientError::MalformedRoots`] (the document-rejection vocabulary's
    /// shared member) when the JSON does not describe a report.
    pub fn parse(text: &str) -> crate::error::Result<Self> {
        serde_json::from_str(text).map_err(|_| crate::error::ClientError::MalformedRoots)
    }
}

/// Validate a run id from the wire: milliseconds, a dash, sixteen hex
/// characters, nothing else. The report keys this shapes are internal,
/// so a tidy grammar costs nothing and keeps oddballs out of the lookup
/// path.
///
/// # Errors
///
/// [`crate::error::ClientError::MalformedKey`] for anything off the shape.
pub fn parse_run_id(text: &str) -> crate::error::Result<()> {
    let Some((digits, hex)) = text.split_once('-') else {
        return Err(crate::error::ClientError::MalformedKey);
    };
    let digits_ok = !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());
    let hex_ok = hex.len() == 16
        && hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if digits_ok && hex_ok {
        return Ok(());
    }
    Err(crate::error::ClientError::MalformedKey)
}

/// Where one invocation hands the bucket to the next tick. Ordering is
/// the run's stage sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GcStage {
    /// Enumerating the bucket.
    Inventory,
    /// Reading the lease documents.
    Leases,
    /// Walking the closures.
    Mark,
    /// Reading each candidate narinfo's URL.
    Candidates,
    /// Executing the deletion plan.
    Sweep,
    /// Writing artifacts, bumping the generation, reaping stale uploads.
    Report,
}

/// The frozen half of a mark stage: the visited set, the walk frontier in
/// its deterministic order, and the marks so far.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MarkCursor {
    /// The root set the walk started with: frozen, because root-ness
    /// decides which unreadable narinfos abort the run, and an answer's
    /// meaning must not drift when leases change mid-run.
    pub roots: Vec<String>,
    /// Visited hashes, sorted by the set.
    pub visited: BTreeSet<String>,
    /// Frontier hashes in walk order.
    pub frontier: Vec<String>,
    /// For each readable marked hash, the NAR key its narinfo named.
    #[serde(rename = "markedUrls", default)]
    pub marked_urls: BTreeMap<String, String>,
    /// Deep narinfo reads that failed so far.
    #[serde(rename = "unreadableDeep")]
    pub unreadable_deep: u64,
}

/// One candidate whose URL the collect stage has read.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CollectedUrl {
    /// The store-path hash.
    pub hash: String,
    /// The NAR key the narinfo named.
    pub url: String,
}

/// The frozen half of the collect stage: which of the candidates the run
/// has read the URL of so far. Candidates themselves recompute from the
/// tick's fresh inventory, and the index walks that tick's list, so a
/// candidate list that shifted between ticks settles at plan time, not
/// mid-collect: the frozen URLs pair with hashes, so each read is applied
/// exactly once regardless of ordering.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CollectCursor {
    /// One-based progress through the candidate list.
    #[serde(rename = "nextIndex")]
    pub next_index: u64,
    /// Hash-URL pairs read so far.
    pub collected: Vec<CollectedUrl>,
}

/// The frozen half of a sweep stage: the fixed plan and how much of it
/// this run has executed. The plan is computed once and persisted; ages
/// answer to the tick that computed them, and boundary objects settle on
/// the next run.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SweepCursor {
    /// Narinfo keys to delete, sorted.
    #[serde(rename = "narinfoDeletes")]
    pub narinfo_deletes: Vec<String>,
    /// NAR keys to delete after the narinfos, sorted and deduplicated.
    #[serde(rename = "narDeletes")]
    pub nar_deletes: Vec<String>,
    /// The leading prefix of each list already deleted.
    #[serde(rename = "narinfosDeleted")]
    pub narinfos_deleted: u64,
    /// NAR deletions executed so far.
    #[serde(rename = "narsDeleted")]
    pub nars_deleted: u64,
    /// Bytes freed by deletions executed so far.
    #[serde(rename = "bytesFreed", default)]
    pub bytes_freed: u64,
    /// Listing sizes by key, so the report's freed bytes sum without
    /// re-reading inventory.
    #[serde(rename = "bytesByKey", default)]
    pub bytes_by_key: BTreeMap<String, u64>,
}

/// The whole freeze: everything the next tick needs to resume this run
/// rather than start over. Stored under `meta/gc-cursor`, which the
/// reserved-prefix grammar keeps out of both the sweep and every request.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GcCursor {
    /// The run's id: `{startedAtMs}-{entropy hex}`, so artifacts sort.
    #[serde(rename = "runId")]
    pub run_id: String,
    /// When the run started.
    #[serde(rename = "startedAtMs")]
    pub started_at_ms: u64,
    /// The stage the next tick resumes.
    pub stage: GcStage,
    /// Counts the report accumulates across ticks.
    #[serde(rename = "inventoryPaths")]
    pub inventory_paths: u64,
    /// Active leases read so far.
    #[serde(rename = "activeLeases")]
    pub active_leases: u64,
    /// Paths the walk marked reachable, set when the mark stage completes.
    #[serde(rename = "markedPaths")]
    pub marked_paths: u64,
    /// Deep narinfo reads that failed in the walk, set at completion.
    #[serde(rename = "unreadableDeep")]
    pub unreadable_deep: u64,
    /// Frozen mark state, present exactly at a mark-boundary freeze.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark: Option<MarkCursor>,
    /// Frozen collect state, present exactly while the URL reads continue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collect: Option<CollectCursor>,
    /// Frozen sweep state, present exactly at a sweep-boundary freeze.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sweep: Option<SweepCursor>,
    /// Stale uploads aborted so far.
    #[serde(rename = "uploadsAborted")]
    pub uploads_aborted: u64,
}

impl GcCursor {
    /// Serialize with the document conventions (indent, trailing newline).
    pub fn serialize(&self) -> String {
        let mut body = serde_json::to_string_pretty(self).expect("the cursor fields serialize");
        body.push('\n');
        body
    }

    /// Read a stored cursor. A corrupt cursor is fatal information for a
    /// run, never a parse to defaults: the caller names the gate.
    ///
    /// # Errors
    ///
    /// [`ClientError::MalformedRoots`] when the stored bytes are not a
    /// cursor.
    pub fn parse(text: &str) -> crate::error::Result<Self> {
        serde_json::from_str(text).map_err(|_| crate::error::ClientError::MalformedRoots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::GRACE_WINDOW_MS;

    #[test]
    fn run_ids_follow_their_grammar() {
        assert!(parse_run_id("1780000000000-00ff112233445566").is_ok());
        for bad in [
            "",
            "1780000000000",
            "x-00ff112233445566",
            "1780000000000-00ff11223344556",
            "1780000000000-00ff11223344556677",
            "1780000000000-00FF11223344556Z",
            "../178-00ff112233445566",
            "1780000000000-00ff112233445566/extra",
            "1780000000000-00FF112233445566",
        ] {
            assert!(parse_run_id(bad).is_err(), "{bad} refused");
        }
    }

    #[test]
    fn a_frozen_walk_resumes_to_the_same_outcome() {
        let a = "a".repeat(32);
        let b = "b".repeat(32);
        let c = "c".repeat(32);
        let docs: BTreeMap<String, Narinfo> = [
            (a.clone(), narinfo_for(&a, &[format!("{b}-dep")])),
            (b.clone(), narinfo_for(&b, &[format!("{c}-dep")])),
            (c.clone(), narinfo_for(&c, &[])),
        ]
        .into_iter()
        .collect();
        let roots = [StorePathHash::parse(&a).unwrap()];

        let whole = closure_walk(&roots, |h| {
            docs.get(h.as_str()).cloned().ok_or(WalkReadError::Absent)
        });

        // Freeze after exactly one answer, resume, and finish.
        let mut walker = ClosureWalker::new(&roots);
        let first = walker.next_read().expect("a root waits");
        walker.answer(
            first.clone(),
            docs.get(first.as_str())
                .cloned()
                .ok_or(WalkReadError::Absent),
        );
        let cursor = walker.to_mark_cursor();
        let mut resumed = ClosureWalker::from_mark_cursor(&cursor);
        while resumed.outcome.gate.is_none() {
            let Some(hash) = resumed.next_read() else {
                break;
            };
            let read = docs
                .get(hash.as_str())
                .cloned()
                .ok_or(WalkReadError::Absent);
            resumed.answer(hash, read);
        }
        let halved = resumed.into_outcome();

        assert_eq!(whole.marked, halved.marked);
        assert_eq!(whole.marked_urls, halved.marked_urls);
        assert_eq!(whole.unreadable_deep, halved.unreadable_deep);
        assert_eq!(whole.gate.is_none(), halved.gate.is_none());
    }

    #[test]
    fn the_walk_marks_roots_first_and_terminates_self_loops() {
        let a = "a".repeat(32);
        let docs: BTreeMap<String, Narinfo> =
            [(a.clone(), narinfo_for(&a, &[format!("{a}-self")]))]
                .into_iter()
                .collect();
        let mut walker = ClosureWalker::new(&[StorePathHash::parse(&a).unwrap()]);
        let mut reads = 0;
        while let Some(hash) = walker.next_read() {
            reads += 1;
            let read = docs
                .get(hash.as_str())
                .cloned()
                .ok_or(WalkReadError::Absent);
            walker.answer(hash, read);
        }
        assert_eq!(reads, 1, "the self loop never re-reads");
        assert_eq!(walker.into_outcome().marked.len(), 1);
    }

    #[test]
    fn the_report_and_cursor_round_trip() {
        let report = GcReport {
            run_id: "1780000000000-00ff".to_string(),
            started_at_ms: 1_780_000_000_000,
            finished_at_ms: 1_780_000_000_500,
            inventory_paths: 12,
            active_leases: 2,
            marked_paths: 9,
            unreadable_deep: 1,
            narinfos_deleted: 3,
            nars_deleted: 2,
            bytes_freed: 486_400,
            uploads_aborted: 1,
            gate: Some("inventory_truncated".to_string()),
        };
        let text = report.serialize();
        assert!(text.ends_with("}\n"));
        assert_eq!(
            GcReport::parse(&text).expect("a written report parses"),
            report
        );

        let cursor = GcCursor {
            run_id: report.run_id.clone(),
            started_at_ms: report.started_at_ms,
            stage: GcStage::Sweep,
            inventory_paths: 12,
            active_leases: 2,
            marked_paths: 9,
            unreadable_deep: 1,
            mark: None,
            collect: None,
            sweep: Some(SweepCursor {
                narinfo_deletes: vec!["a".repeat(32) + ".narinfo"],
                nar_deletes: vec!["nar/".to_string() + &"b".repeat(52) + ".nar.zst"],
                narinfos_deleted: 1,
                nars_deleted: 0,
                bytes_freed: 0,
                bytes_by_key: BTreeMap::new(),
            }),
            uploads_aborted: 0,
        };
        let parsed = GcCursor::parse(&cursor.serialize()).expect("a written cursor parses");
        assert_eq!(parsed, cursor);
        assert!(GcCursor::parse("{not json").is_err());
    }

    fn narinfo_for(hash: &str, refs: &[String]) -> Narinfo {
        let body = format!(
            "StorePath: /nix/store/{hash}-pkg\nURL: nar/{}.nar.zst\nNarHash: sha256:0iqi0\nNarSize: 12\nReferences: {}\n",
            "x".repeat(52),
            refs.join(" ")
        );
        Narinfo::parse(&body).expect("test narinfo parses")
    }

    fn items(keys: &[&str], age_ms: u64) -> Vec<InventoryItem> {
        keys.iter()
            .map(|key| InventoryItem {
                key: (*key).to_string(),
                size_bytes: 10,
                uploaded_at_ms: 1_000_000_000_000 - age_ms,
            })
            .collect()
    }

    #[test]
    fn walk_marks_the_whole_closure_and_cycles_terminate() {
        let a = "a".repeat(32);
        let b = "b".repeat(32);
        let docs: BTreeMap<String, Narinfo> = [
            (
                a.clone(),
                narinfo_for(&a, &[format!("{b}-dep"), format!("{a}-self")]),
            ),
            (b.clone(), narinfo_for(&b, &[format!("{a}-back")])),
        ]
        .into_iter()
        .collect();
        let outcome = closure_walk(&[StorePathHash::parse(&a).unwrap()], |hash| {
            docs.get(hash.as_str())
                .cloned()
                .ok_or(WalkReadError::Absent)
        });
        assert!(outcome.gate.is_none());
        assert_eq!(outcome.marked.len(), 2);
        assert_eq!(outcome.unreadable_deep, 0);
    }

    #[test]
    fn an_unparseable_root_trips_the_abortion_gate() {
        let hash = StorePathHash::parse(&"a".repeat(32)).unwrap();
        let outcome = closure_walk(std::slice::from_ref(&hash), |_| {
            Err(WalkReadError::Unparseable)
        });
        assert!(matches!(
            outcome.gate,
            Some(GateTrip::UnreadableRootNarinfo { .. })
        ));
        // Marking is what keeps the sweep off a root the walk could not
        // read. The gate decides whether the walk goes on, never whether
        // the path survives.
        assert!(outcome.marked.contains(&hash));
    }

    #[test]
    fn an_absent_root_is_counted_rather_than_refused() {
        // This gate used to fire here too, and could never stop firing: a
        // lease naming a path whose narinfo is gone keeps naming it, and
        // no later run brings the narinfo back (ADR 0018). Refusing also
        // protected nothing, because no client can substitute a path
        // whose narinfo is absent.
        let hash = StorePathHash::parse(&"a".repeat(32)).unwrap();
        let outcome = closure_walk(std::slice::from_ref(&hash), |_| Err(WalkReadError::Absent));
        assert_eq!(outcome.gate, None);
        assert_eq!(outcome.unreadable_deep, 1);
        assert!(outcome.marked.contains(&hash));
    }

    #[test]
    fn a_deep_failure_marks_without_descent() {
        let a = "a".repeat(32);
        let c = "c".repeat(32);
        let docs: BTreeMap<String, Narinfo> =
            [(a.clone(), narinfo_for(&a, &[format!("{c}-deep")]))]
                .into_iter()
                .collect();
        let outcome = closure_walk(&[StorePathHash::parse(&a).unwrap()], |hash| {
            docs.get(hash.as_str())
                .cloned()
                .ok_or(WalkReadError::Absent)
        });
        assert!(outcome.gate.is_none());
        assert_eq!(outcome.unreadable_deep, 1);
        assert!(outcome.marked.contains(&StorePathHash::parse(&c).unwrap()));
    }

    #[test]
    fn sweeps_the_dead_and_spares_the_shared() {
        let live = "a".repeat(32);
        let dead = "d".repeat(32);
        let live_key = format!("{live}.narinfo");
        let dead_key = format!("{dead}.narinfo");
        // Six fresh paths beside the dead one, so the sweep is proven to
        // spare what a lease pins rather than to empty a bucket:
        // one deletion over nine paths is far below 0.25.
        let fresh_keys: Vec<String> = (0..6)
            .map(|i| format!("{:0>32}.narinfo", 10_007 * i))
            .collect();
        let mut inventory = items(&[&live_key, &dead_key, "roots/x"], GRACE_WINDOW_MS * 2);
        inventory.extend(items(
            &fresh_keys.iter().map(String::as_str).collect::<Vec<_>>(),
            0,
        ));
        let live_url =
            crate::keys::parse_nar_key(&format!("nar/{}.nar.zst", "g".repeat(52))).unwrap();
        let dead_url =
            crate::keys::parse_nar_key(&format!("nar/{}.nar.zst", "w".repeat(52))).unwrap();
        let live_hash = StorePathHash::parse(&live).unwrap();
        let dead_hash = StorePathHash::parse(&dead).unwrap();
        let marked: BTreeSet<StorePathHash> = [live_hash.clone()].into_iter().collect();
        let marked_urls: BTreeMap<StorePathHash, NarKey> = [(live_hash.clone(), live_url.clone())]
            .into_iter()
            .collect();
        let plan = plan_deletions(
            &inventory,
            &marked,
            &marked_urls,
            &[(live_hash, live_url.clone()), (dead_hash, dead_url.clone())],
            UnixMillis::new(1_000_000_000_000),
            GRACE_WINDOW_MS,
        );
        assert!(plan.gate.is_none(), "no gate tripped: {plan:?}");
        assert_eq!(plan.narinfo_deletes, vec![dead_key]);
        assert_eq!(plan.nar_deletes, vec![dead_url.as_str().to_string()]);
        assert!(!plan.narinfo_deletes.contains(&live_key));
    }

    #[test]
    fn a_wholesale_sweep_is_planned_rather_than_refused() {
        // The gate that used to refuse this could not be satisfied by the
        // run after it either: refusing deleted nothing, so the next run
        // saw the same inventory and refused again. A cache whose problem
        // is unbounded growth would then never collect again.
        let keys: Vec<String> = (0..8)
            .map(|i| format!("{:0>32}.narinfo", i * 111_111_111_u64))
            .collect();
        let inventory = items(
            &keys.iter().map(String::as_str).collect::<Vec<_>>(),
            GRACE_WINDOW_MS * 2,
        );
        let plan = plan_deletions(
            &inventory,
            &BTreeSet::new(),
            &BTreeMap::new(),
            &[],
            UnixMillis::new(1_000_000_000_000),
            GRACE_WINDOW_MS,
        );
        assert!(plan.gate.is_none(), "{plan:?}");
        assert_eq!(plan.narinfo_deletes.len(), 8);
    }
}
