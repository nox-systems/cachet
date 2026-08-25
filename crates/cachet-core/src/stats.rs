//! What a deployment counts about itself.
//!
//! Every interesting request writes one data point to an Analytics
//! Engine dataset: what happened, to what, for whom, and how much. The
//! worker's event log already says the same things, but a log line is
//! something an operator greps during an incident. These are for the
//! questions asked afterwards and continuously: what is the hit rate,
//! which repository pushes the most bytes, is CI reading more than
//! laptops, did last night's collector actually delete anything.
//!
//! The schema lives here, in the pure crate, because a data point is
//! positional: `blob3` means an outcome only because every writer agrees
//! it does, and a query written against the wrong position is silently
//! wrong rather than broken. One type defines the positions, every
//! writer fills that type, and docs/DEPLOY.md's queries name the same
//! columns.

/// Which family of thing happened. The dataset's index, so it is also
/// the sampling key: coarse on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatEvent {
    /// An object was served, or was not.
    Read,
    /// An object was stored, or was refused.
    Write,
    /// A push asked what the cache already holds.
    Probe,
    /// The collector ran.
    Collect,
    /// A credential was issued or refused.
    Auth,
}

impl StatEvent {
    /// The name the dataset stores and a query groups by.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Probe => "probe",
            Self::Collect => "collect",
            Self::Auth => "auth",
        }
    }
}

/// Who the request was. Reads split three ways and the split is the
/// point: a deployment whose reads are all CI is a different thing from
/// one people substitute from daily.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatActor {
    /// A workflow run, holding an OIDC token.
    Ci,
    /// A person's machine, holding a credential this deployment issued.
    Laptop,
    /// A browser session.
    Browser,
    /// Nobody: the request carried no usable credential.
    Anonymous,
}

impl StatActor {
    /// The name the dataset stores.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ci => "ci",
            Self::Laptop => "laptop",
            Self::Browser => "browser",
            Self::Anonymous => "anonymous",
        }
    }
}

/// Who made a request, with whatever the credential knew about them.
///
/// Carried as one value so a caller cannot pass the actor and forget the
/// run it belonged to: for a workflow those two are the same fact, and a
/// read counted without its repository cannot be grouped by one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatCaller {
    /// The caller class.
    pub actor: Option<StatActor>,
    /// `owner/repo` when a workflow run, else empty.
    pub repository: String,
    /// That run's ref, else empty.
    pub reference: String,
}

impl StatCaller {
    /// A caller with nothing known about it.
    #[must_use]
    pub fn anonymous() -> Self {
        Self {
            actor: Some(StatActor::Anonymous),
            ..Self::default()
        }
    }

    /// A person at a machine.
    #[must_use]
    pub fn laptop() -> Self {
        Self {
            actor: Some(StatActor::Laptop),
            ..Self::default()
        }
    }

    /// A browser session.
    #[must_use]
    pub fn browser() -> Self {
        Self {
            actor: Some(StatActor::Browser),
            ..Self::default()
        }
    }

    /// A workflow run, which knows where it ran.
    #[must_use]
    pub fn ci(repository: &str, reference: &str) -> Self {
        Self {
            actor: Some(StatActor::Ci),
            repository: repository.to_string(),
            reference: reference.to_string(),
        }
    }
}

/// One data point, with every dimension a query may group by.
///
/// Absent dimensions are the empty string rather than omitted: a data
/// point's blobs are positional, so a writer that skipped one would
/// shift every dimension after it.
#[derive(Debug, Clone, PartialEq)]
pub struct StatPoint {
    /// The family of thing that happened.
    pub event: StatEvent,
    /// What it happened to: `narinfo`, `nar`, `part`, `lease`, `sweep`.
    pub kind: String,
    /// How it went: `edge_hit`, `bucket_hit`, `miss`, `stored`, or the
    /// error code of a refusal, so a rejection rate is one query.
    pub outcome: String,
    /// Who asked.
    pub actor: StatActor,
    /// `owner/repo` when a workflow run is the caller, else empty.
    pub repository: String,
    /// The git ref of that run, else empty.
    pub reference: String,
    /// The lease name a push writes under, else empty.
    pub project: String,
    /// How many things this point counts. Almost always one; a batch
    /// says how big it was.
    pub count: f64,
    /// Bytes moved, where the answer is bytes.
    pub bytes: f64,
}

impl StatPoint {
    /// A point with every dimension empty, for a writer to fill.
    #[must_use]
    pub fn new(event: StatEvent, kind: &str, outcome: &str) -> Self {
        Self {
            event,
            kind: kind.to_string(),
            outcome: outcome.to_string(),
            actor: StatActor::Anonymous,
            repository: String::new(),
            reference: String::new(),
            project: String::new(),
            count: 1.0,
            bytes: 0.0,
        }
    }

    /// Name the caller, and whatever their credential knew about them.
    #[must_use]
    pub fn by(mut self, caller: &StatCaller) -> Self {
        self.actor = caller.actor.unwrap_or(StatActor::Anonymous);
        self.repository.clone_from(&caller.repository);
        self.reference.clone_from(&caller.reference);
        self
    }

    /// Name the workflow run this came from.
    #[must_use]
    pub fn from_run(mut self, repository: &str, reference: &str) -> Self {
        self.repository = repository.to_string();
        self.reference = reference.to_string();
        self
    }

    /// Name the lease this concerns.
    #[must_use]
    pub fn for_project(mut self, project: &str) -> Self {
        self.project = project.to_string();
        self
    }

    /// Say how many, and how much.
    #[must_use]
    pub fn measuring(mut self, count: u64, bytes: u64) -> Self {
        self.count = exact(count);
        self.bytes = exact(bytes);
        self
    }

    /// The blob columns, in the order every query names them.
    #[must_use]
    pub fn blobs(&self) -> [&str; 6] {
        [
            self.kind.as_str(),
            self.outcome.as_str(),
            self.actor.name(),
            self.repository.as_str(),
            self.reference.as_str(),
            self.project.as_str(),
        ]
    }

    /// The double columns, in the order every query names them.
    #[must_use]
    pub fn doubles(&self) -> [f64; 2] {
        [self.count, self.bytes]
    }
}

/// A count as the dataset stores it.
///
/// Analytics Engine holds doubles, whose mantissa is 53 bits, so a value
/// past nine petabytes would round. Nothing here is nine petabytes, and
/// a number claiming to be is worth capping rather than rounding: a
/// total that saturates is visibly wrong, where one that rounds is
/// quietly wrong.
#[must_use]
pub fn exact(value: u64) -> f64 {
    /// Two to the fifty-third: the largest integer an f64 holds exactly.
    const EXACT_MAX: u64 = 9_007_199_254_740_992;
    #[allow(
        clippy::cast_precision_loss,
        reason = "clamped to 2^53, below which f64 is exact"
    )]
    {
        value.min(EXACT_MAX) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_column_order_is_the_documented_one() {
        // docs/DEPLOY.md's queries name blob1..blob6 and double1..double2
        // in exactly this order. A writer that reordered them would make
        // every committed query silently wrong, so the order is asserted
        // rather than assumed.
        let point = StatPoint::new(StatEvent::Read, "narinfo", "edge_hit")
            .by(&StatCaller::ci("nox-systems/cachet", "refs/heads/main"))
            .for_project("nox-systems-cachet")
            .measuring(1, 4_096);
        assert_eq!(
            point.blobs(),
            [
                "narinfo",
                "edge_hit",
                "ci",
                "nox-systems/cachet",
                "refs/heads/main",
                "nox-systems-cachet",
            ]
        );
        // why: bit equality. The claim is that these land exactly, not
        // approximately, which is the whole reason `exact` clamps.
        assert_eq!(
            point.doubles().map(f64::to_bits),
            [1.0_f64, 4_096.0].map(f64::to_bits)
        );
        assert_eq!(point.event.name(), "read");
    }

    #[test]
    fn an_unfilled_dimension_is_empty_and_not_missing() {
        // Positional columns: skipping one would shift the rest.
        let point = StatPoint::new(StatEvent::Probe, "probe", "answered");
        assert_eq!(point.blobs().len(), 6);
        assert_eq!(point.blobs()[3], "", "no repository is the empty string");
        assert_eq!(point.actor, StatActor::Anonymous);
    }

    #[test]
    fn counts_clamp_rather_than_round() {
        assert_eq!(exact(0).to_bits(), 0.0_f64.to_bits());
        assert_eq!(exact(4_096).to_bits(), 4_096.0_f64.to_bits());
        // Past the mantissa, the value stops rather than drifting.
        assert_eq!(
            exact(u64::MAX).to_bits(),
            9_007_199_254_740_992.0_f64.to_bits()
        );
    }

    #[test]
    fn every_actor_and_event_names_itself() {
        for actor in [
            StatActor::Ci,
            StatActor::Laptop,
            StatActor::Browser,
            StatActor::Anonymous,
        ] {
            assert!(!actor.name().is_empty());
        }
        for event in [
            StatEvent::Read,
            StatEvent::Write,
            StatEvent::Probe,
            StatEvent::Collect,
            StatEvent::Auth,
        ] {
            assert!(!event.name().is_empty());
        }
    }
}
