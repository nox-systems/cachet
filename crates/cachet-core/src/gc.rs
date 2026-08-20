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

use crate::constants::{
    CLOSURE_WALK_PATHS_MAX, SWEEP_MAX_FRACTION_DENOMINATOR, SWEEP_MAX_FRACTION_NUMERATOR,
};
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
    /// Deleting the candidates would remove more than the configured
    /// fraction of the path universe.
    SweepFractionExceeded {
        /// Candidate narinfo count.
        deletions: usize,
        /// Total narinfo inventory.
        inventory: usize,
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
            Self::SweepFractionExceeded {
                deletions,
                inventory,
            } => write!(
                f,
                "sweep of {deletions}/{inventory} paths exceeds the fraction gate"
            ),
            Self::GenerationCorrupt => f.write_str("generation document corrupt"),
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

/// Walk the reference graph from the root hashes, breadth-first, one
/// narinfo read per newly visited path. Root read failures trip the
/// walk-aborting gate; deep failures mark the node without descending,
/// because the alternative, deleting under it, would eat descendants we
/// cannot see. The walk is deterministic: insertion-ordered queue over
/// sorted roots, `BTreeSet` visited set.
pub fn closure_walk(
    roots: &[StorePathHash],
    mut read_narinfo: impl FnMut(&StorePathHash) -> std::result::Result<Narinfo, WalkReadError>,
) -> WalkOutcome {
    let mut outcome = WalkOutcome::default();
    let mut visited = BTreeSet::new();
    let mut queue: VecDeque<StorePathHash> = roots.iter().cloned().collect();
    queue.make_contiguous().sort();

    while let Some(hash) = queue.pop_front() {
        if !visited.insert(hash.clone()) {
            continue;
        }
        if visited.len() > CLOSURE_WALK_PATHS_MAX {
            outcome.gate = Some(GateTrip::WalkBudgetExhausted {
                visited: visited.len(),
            });
            return outcome;
        }
        if let Ok(document) = read_narinfo(&hash) {
            let url = document.url.clone();
            for edge in document.reference_hashes() {
                if !visited.contains(&edge) {
                    queue.push_back(edge);
                }
            }
            outcome.marked.insert(hash.clone());
            outcome.marked_urls.insert(hash, url);
        } else {
            let is_root = roots.contains(&hash);
            outcome.marked.insert(hash.clone());
            if is_root {
                outcome.gate = Some(GateTrip::UnreadableRootNarinfo { hash });
                return outcome;
            }
            outcome.unreadable_deep += 1;
        }
    }
    outcome
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

/// Decide the destructive plan.
///
/// Candidates are narinfo objects that are unmarked and older than the
/// grace window. NAR deletions resolve from the candidate narinfos' own
/// URLs minus every URL a marked narinfo still names, so a NAR shared
/// between a live and a dead path survives. The fraction gate compares
/// candidate count against the full narinfo inventory: a run that would
/// empty the cache is refused outright.
pub fn plan_deletions(
    inventory: &[InventoryItem],
    marked: &BTreeSet<StorePathHash>,
    marked_urls: &BTreeMap<StorePathHash, NarKey>,
    candidate_urls: &[(StorePathHash, NarKey)],
    now: UnixMillis,
    grace_ms: u64,
) -> DeletionPlan {
    let mut plan = DeletionPlan::default();

    let mut narinfo_inventory = 0usize;
    let mut narinfo_deletes = Vec::new();
    for item in inventory {
        if is_reserved_key(&item.key) {
            continue;
        }
        if item.key.ends_with(crate::constants::NARINFO_KEY_SUFFIX) {
            let Ok(hash) = StorePathHash::parse(
                &item.key[..item.key.len() - crate::constants::NARINFO_KEY_SUFFIX.len()],
            ) else {
                // Unnameable narinfo-shaped object: it cannot map to a
                // lease-protected path either way, so plan says nothing and
                // the report's gate list has it noted by the driver.
                continue;
            };
            narinfo_inventory += 1;
            if !marked.contains(&hash)
                && now.saturating_ms_since(UnixMillis::new(item.uploaded_at_ms)) > grace_ms
            {
                narinfo_deletes.push(item.key.clone());
            }
        }
    }
    narinfo_deletes.sort();

    // why: the fraction gate compares in integers, not floats: usize→f64
    // casts lose precision at pathological cache sizes, and a safety gate
    // must never silently widen.
    if narinfo_deletes.len() * SWEEP_MAX_FRACTION_DENOMINATOR
        > narinfo_inventory * SWEEP_MAX_FRACTION_NUMERATOR
    {
        plan.gate = Some(GateTrip::SweepFractionExceeded {
            deletions: narinfo_deletes.len(),
            inventory: narinfo_inventory,
        });
        return plan;
    }
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
    /// Deep narinfo reads that failed during the walk.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::GRACE_WINDOW_MS;

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
    fn a_root_read_failure_trips_the_abortion_gate() {
        let hash = StorePathHash::parse(&"a".repeat(32)).unwrap();
        let outcome = closure_walk(&[hash], |_| Err(WalkReadError::Absent));
        assert!(matches!(
            outcome.gate,
            Some(GateTrip::UnreadableRootNarinfo { .. })
        ));
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
        // Six fresh paths keep the sweep small against the fraction gate:
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
        assert!(plan.gate.is_none(), "fraction gate held: {plan:?}");
        assert_eq!(plan.narinfo_deletes, vec![dead_key]);
        assert_eq!(plan.nar_deletes, vec![dead_url.as_str().to_string()]);
        assert!(!plan.narinfo_deletes.contains(&live_key));
    }

    #[test]
    fn the_fraction_gate_aborts() {
        let hashes: Vec<String> = (0..8)
            .map(|i| format!("{:0>32}", i * 111_111_111_u64))
            .collect();
        let keys: Vec<String> = hashes.iter().map(|h| format!("{h}.narinfo")).collect();
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
        assert!(matches!(
            plan.gate,
            Some(GateTrip::SweepFractionExceeded { .. })
        ));
        assert!(plan.narinfo_deletes.is_empty());
        assert!(plan.nar_deletes.is_empty());
    }
}
