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
//!
//! The kind and outcome vocabularies are enums rather than free strings
//! because `stats_query` filters on them. A filter value has to parse
//! into a closed set before it can be formatted into SQL, and the set a
//! reader may filter by is exactly the set the writers emit, so both
//! sides read from the one definition here.

use std::borrow::Cow;

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
}

impl StatEvent {
    /// The name the dataset stores and a query groups by.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Probe => "probe",
        }
    }
}

/// What the counted thing was: `blob1`.
///
/// One variant per branch a writer can take, and nothing else. A kind
/// nobody emits would be a question `/api/self/events?kind=` accepts and
/// can only answer with zero, which reads as "none happened" rather than
/// "this deployment never records that".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatKind {
    /// A `{hash}.narinfo` document.
    Narinfo,
    /// A `nar/{key}` object, whole.
    Nar,
    /// One part of a multipart upload.
    Part,
    /// A multipart upload was opened.
    Begin,
    /// A multipart upload was completed.
    Complete,
    /// A multipart upload was abandoned.
    Abort,
    /// A push asked what the cache already holds.
    Probe,
    /// A write whose route matched no other branch.
    Unknown,
}

impl StatKind {
    /// The name the dataset stores.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Narinfo => "narinfo",
            Self::Nar => "nar",
            Self::Part => "part",
            Self::Begin => "begin",
            Self::Complete => "complete",
            Self::Abort => "abort",
            Self::Probe => "probe",
            Self::Unknown => "unknown",
        }
    }

    /// Parse a reader's filter choice. Anything else is not a choice.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "narinfo" => Some(Self::Narinfo),
            "nar" => Some(Self::Nar),
            "part" => Some(Self::Part),
            "begin" => Some(Self::Begin),
            "complete" => Some(Self::Complete),
            "abort" => Some(Self::Abort),
            "probe" => Some(Self::Probe),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }

    /// Every kind, for the tests that hold the two sides in step.
    #[must_use]
    pub const fn all() -> [Self; 8] {
        [
            Self::Narinfo,
            Self::Nar,
            Self::Part,
            Self::Begin,
            Self::Complete,
            Self::Abort,
            Self::Probe,
            Self::Unknown,
        ]
    }
}

impl From<crate::read::ObjectKind> for StatKind {
    /// The read path names its object kinds in its own enum; counting
    /// them means the one vocabulary a filter can also name.
    fn from(kind: crate::read::ObjectKind) -> Self {
        match kind {
            crate::read::ObjectKind::Narinfo => Self::Narinfo,
            crate::read::ObjectKind::Nar => Self::Nar,
        }
    }
}

/// How the counted thing went: `blob2`.
///
/// The named variants are the successful and near-successful shapes. A
/// refusal carries its HTTP status instead, because the error code lives
/// in a body the counting layer would have to consume to read, and a
/// rejection rate answers the question either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatOutcome {
    /// Served from the Cache API without touching the bucket.
    EdgeHit,
    /// Served from the bucket.
    BucketHit,
    /// The bucket does not hold it.
    Miss,
    /// The write stored it.
    Stored,
    /// The probe answered.
    Answered,
    /// A refusal, named by the status it answered with.
    Status(u16),
}

/// The smallest HTTP status a refusal can carry.
const STATUS_MIN: u16 = 100;
/// One past the largest HTTP status a refusal can carry.
const STATUS_END: u16 = 600;

impl StatOutcome {
    /// The text the dataset stores.
    ///
    /// Borrowed for the named variants and owned for a status, so a
    /// writer pays an allocation only on the refusal path.
    #[must_use]
    pub fn render(self) -> Cow<'static, str> {
        match self {
            Self::EdgeHit => Cow::Borrowed("edge_hit"),
            Self::BucketHit => Cow::Borrowed("bucket_hit"),
            Self::Miss => Cow::Borrowed("miss"),
            Self::Stored => Cow::Borrowed("stored"),
            Self::Answered => Cow::Borrowed("answered"),
            Self::Status(status) => Cow::Owned(status.to_string()),
        }
    }

    /// Parse a reader's filter choice. Anything else is not a choice.
    ///
    /// A status parses only as exactly three ASCII digits naming a real
    /// HTTP status, so the value formatted into SQL is an integer this
    /// function produced rather than text a caller wrote.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "edge_hit" => Some(Self::EdgeHit),
            "bucket_hit" => Some(Self::BucketHit),
            "miss" => Some(Self::Miss),
            "stored" => Some(Self::Stored),
            "answered" => Some(Self::Answered),
            _ => {
                if text.len() != 3 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
                    return None;
                }
                let status: u16 = text.parse().ok()?;
                (STATUS_MIN..STATUS_END)
                    .contains(&status)
                    .then_some(Self::Status(status))
            }
        }
    }

    /// Every named outcome, for the tests that hold the two sides in
    /// step. `Status` is excluded: it is a family, not a name.
    #[must_use]
    pub const fn named() -> [Self; 5] {
        [
            Self::EdgeHit,
            Self::BucketHit,
            Self::Miss,
            Self::Stored,
            Self::Answered,
        ]
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

    /// Parse a reader's filter choice. Anything else is not a choice.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "ci" => Some(Self::Ci),
            "laptop" => Some(Self::Laptop),
            "browser" => Some(Self::Browser),
            "anonymous" => Some(Self::Anonymous),
            _ => None,
        }
    }

    /// Every actor, for the tests that hold the two sides in step.
    #[must_use]
    pub const fn all() -> [Self; 4] {
        [Self::Ci, Self::Laptop, Self::Browser, Self::Anonymous]
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
    /// What it happened to.
    pub kind: StatKind,
    /// How it went.
    pub outcome: StatOutcome,
    /// Who asked.
    pub actor: StatActor,
    /// `owner/repo` when a workflow run is the caller, else empty.
    pub repository: String,
    /// The git ref of that run, else empty.
    pub reference: String,
    /// How many things this point counts. Almost always one; a batch
    /// says how big it was.
    pub count: f64,
    /// Bytes moved, where the answer is bytes.
    pub bytes: f64,
}

impl StatPoint {
    /// A point with every dimension empty, for a writer to fill.
    #[must_use]
    pub fn new(event: StatEvent, kind: StatKind, outcome: StatOutcome) -> Self {
        Self {
            event,
            kind,
            outcome,
            actor: StatActor::Anonymous,
            repository: String::new(),
            reference: String::new(),
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

    /// Say how many, and how much.
    #[must_use]
    pub fn measuring(mut self, count: u64, bytes: u64) -> Self {
        self.count = exact(count);
        self.bytes = exact(bytes);
        self
    }

    /// The blob columns, in the order every query names them.
    ///
    /// `blob6` is reserved and always empty. It held a lease name that
    /// no writer ever filled, and the column stays in place rather than
    /// closing the gap because renumbering would make every point
    /// already in the dataset read as a different question.
    #[must_use]
    pub fn blobs(&self) -> [Cow<'_, str>; 6] {
        [
            Cow::Borrowed(self.kind.name()),
            self.outcome.render(),
            Cow::Borrowed(self.actor.name()),
            Cow::Borrowed(self.repository.as_str()),
            Cow::Borrowed(self.reference.as_str()),
            Cow::Borrowed(""),
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
        let point = StatPoint::new(StatEvent::Read, StatKind::Narinfo, StatOutcome::EdgeHit)
            .by(&StatCaller::ci("nox-systems/cachet", "refs/heads/main"))
            .measuring(1, 4_096);
        assert_eq!(
            point.blobs(),
            [
                "narinfo",
                "edge_hit",
                "ci",
                "nox-systems/cachet",
                "refs/heads/main",
                "",
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
        let point = StatPoint::new(StatEvent::Probe, StatKind::Probe, StatOutcome::Answered);
        assert_eq!(point.blobs().len(), 6);
        assert_eq!(point.blobs()[3], "", "no repository is the empty string");
        assert_eq!(point.blobs()[5], "", "blob6 is reserved and never filled");
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
    fn every_written_name_parses_back_to_what_wrote_it() {
        // The writer's vocabulary and the reader's filter vocabulary are
        // the same set. A kind a writer can emit but a filter cannot name
        // would be invisible; a name a filter accepts but no writer emits
        // would answer zero and read as "none happened".
        for kind in StatKind::all() {
            assert_eq!(StatKind::parse(kind.name()), Some(kind), "{kind:?}");
        }
        for outcome in StatOutcome::named() {
            assert_eq!(
                StatOutcome::parse(&outcome.render()),
                Some(outcome),
                "{outcome:?}"
            );
        }
        for actor in StatActor::all() {
            assert_eq!(StatActor::parse(actor.name()), Some(actor), "{actor:?}");
        }
    }

    #[test]
    fn a_refusal_names_itself_by_status_and_only_by_status() {
        assert_eq!(StatOutcome::Status(404).render(), "404");
        assert_eq!(StatOutcome::parse("404"), Some(StatOutcome::Status(404)));
        assert_eq!(StatOutcome::parse("503"), Some(StatOutcome::Status(503)));
        // Three ASCII digits naming a real status, and nothing else. A
        // value that parsed loosely here would be a value formatted into
        // SQL that a caller chose the shape of.
        for refused in [
            "4041", "40", "0404", "4o4", "099", "600", "999", "+04", " 404", "",
        ] {
            assert_eq!(StatOutcome::parse(refused), None, "{refused}");
        }
    }

    #[test]
    fn every_event_names_itself() {
        for event in [StatEvent::Read, StatEvent::Write, StatEvent::Probe] {
            assert!(!event.name().is_empty());
        }
    }
}
