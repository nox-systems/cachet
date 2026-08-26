//! The questions a deployment will answer about its own counters.
//!
//! Reading an Analytics Engine dataset means SQL, and the credential
//! that runs it is a Cloudflare API token. A worker holding one must
//! never let a caller compose the query: a UI that could send SQL would
//! be an injection surface with an account token behind it, and the
//! token's narrow scope bounds the damage without preventing it.
//!
//! So a caller does not send a query. It chooses one, from here. Every
//! part of the emitted SQL is either a literal or a value drawn from a
//! closed enum, so there is no path by which caller text reaches the
//! statement at all. That is a stronger claim than escaping, and the
//! tests below are what hold it: a hostile string simply fails to parse
//! into a choice.
//!
//! Two kinds of question live here. Grouping by a blob column answers
//! "how much, split by what", largest first. Grouping by a time bucket
//! answers "how much, over time", oldest first and gap-filled, because a
//! chart with a missing day claims no traffic happened rather than
//! admitting the dataset had nothing to say.

use crate::stats::{StatActor, StatKind, StatOutcome};

/// Which family of counted thing to report on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuerySubject {
    /// Objects served.
    Reads,
    /// Objects stored, or refused.
    Writes,
    /// Presence probes, one per push.
    Probes,
}

impl QuerySubject {
    /// Parse a caller's choice. Anything else is not a choice.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "reads" => Some(Self::Reads),
            "writes" => Some(Self::Writes),
            "probes" => Some(Self::Probes),
            _ => None,
        }
    }

    /// The name this choice was parsed from, for echoing an answer back
    /// without re-reading the caller's query string.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Reads => "reads",
            Self::Writes => "writes",
            Self::Probes => "probes",
        }
    }

    /// The `index1` value this subject selects on.
    const fn index(self) -> &'static str {
        match self {
            Self::Reads => "read",
            Self::Writes => "write",
            Self::Probes => "probe",
        }
    }
}

/// Seconds in an hour, the smaller bucket.
const HOUR_SECS: u64 = 3_600;
/// Seconds in a day, the larger bucket and the window's unit.
const DAY_SECS: u64 = 86_400;

/// What to group by: a dimension the writers fill, or a slice of time.
///
/// The mapping to a blob position lives here so a caller never names
/// one, and the time variants emit an expression rather than a column
/// for the same reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryDimension {
    /// What the thing was.
    Kind,
    /// How it went.
    Outcome,
    /// Who asked.
    Actor,
    /// Which repository's workflow run.
    Repository,
    /// Which ref that run was on.
    Reference,
    /// One row per hour.
    Hour,
    /// One row per day.
    Day,
}

impl QueryDimension {
    /// Parse a caller's choice. Anything else is not a choice.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "kind" => Some(Self::Kind),
            "outcome" => Some(Self::Outcome),
            "actor" => Some(Self::Actor),
            "repository" => Some(Self::Repository),
            "reference" => Some(Self::Reference),
            "hour" => Some(Self::Hour),
            "day" => Some(Self::Day),
            _ => None,
        }
    }

    /// The name this choice was parsed from, for echoing an answer back
    /// without re-reading the caller's query string.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Kind => "kind",
            Self::Outcome => "outcome",
            Self::Actor => "actor",
            Self::Repository => "repository",
            Self::Reference => "reference",
            Self::Hour => "hour",
            Self::Day => "day",
        }
    }

    /// How wide one row is, for a time dimension.
    #[must_use]
    pub const fn bucket_secs(self) -> Option<u64> {
        match self {
            Self::Hour => Some(HOUR_SECS),
            Self::Day => Some(DAY_SECS),
            _ => None,
        }
    }

    /// The projection this dimension groups by.
    ///
    /// A blob dimension names its column; a time dimension floors the
    /// platform's ingest timestamp to a bucket and renders it as epoch
    /// seconds. The rendering is deliberate: a formatted date would make
    /// the answer depend on the SQL engine's calendar formatting, where
    /// an integer means the same thing to every reader.
    fn projection(self) -> String {
        match self {
            Self::Kind => "blob1".to_string(),
            Self::Outcome => "blob2".to_string(),
            Self::Actor => "blob3".to_string(),
            Self::Repository => "blob4".to_string(),
            Self::Reference => "blob5".to_string(),
            Self::Hour | Self::Day => {
                let bucket = self.bucket_secs().unwrap_or(DAY_SECS);
                format!("toString(intDiv(toUInt32(timestamp), {bucket}) * {bucket})")
            }
        }
    }

    /// Every dimension, for the tests that hold the mapping honest.
    #[must_use]
    pub const fn all() -> [Self; 7] {
        [
            Self::Kind,
            Self::Outcome,
            Self::Actor,
            Self::Repository,
            Self::Reference,
            Self::Hour,
            Self::Day,
        ]
    }
}

/// How far back to look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryWindow {
    /// The last day.
    Day,
    /// The last week.
    Week,
    /// The last month.
    Month,
}

impl QueryWindow {
    /// Parse a caller's choice, defaulting to a day when unstated.
    #[must_use]
    pub fn parse(text: Option<&str>) -> Option<Self> {
        match text {
            None | Some("day") => Some(Self::Day),
            Some("week") => Some(Self::Week),
            Some("month") => Some(Self::Month),
            _ => None,
        }
    }

    /// The name this choice was parsed from, for echoing an answer back.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
        }
    }

    /// The interval's day count. An integer, so it is a literal in the
    /// emitted SQL rather than caller text.
    const fn days(self) -> u64 {
        match self {
            Self::Day => 1,
            Self::Week => 7,
            Self::Month => 30,
        }
    }

    /// How many seconds the window spans.
    #[must_use]
    pub const fn secs(self) -> u64 {
        self.days() * DAY_SECS
    }
}

/// The dimensions a caller may narrow an answer by.
///
/// Only the closed-vocabulary dimensions are here. `repository`,
/// `reference`, and the lease name hold values a pusher chose, so
/// admitting them as filters would put caller text into a statement that
/// runs with the deployment's Cloudflare token. Grouping by them stays
/// available, because a `GROUP BY` names a column and never a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QueryFilters {
    /// Only this kind of thing.
    pub kind: Option<StatKind>,
    /// Only this outcome.
    pub outcome: Option<StatOutcome>,
    /// Only this caller class.
    pub actor: Option<StatActor>,
}

impl QueryFilters {
    /// True when nothing is narrowed.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.kind.is_none() && self.outcome.is_none() && self.actor.is_none()
    }

    /// The `AND` clauses these filters contribute, in column order.
    fn clauses(self) -> String {
        let mut sql = String::new();
        let mut clause = |column: &str, value: &str| {
            sql.push_str(" AND ");
            sql.push_str(column);
            sql.push_str(" = '");
            sql.push_str(value);
            sql.push('\'');
        };
        if let Some(kind) = self.kind {
            clause("blob1", kind.name());
        }
        if let Some(outcome) = self.outcome {
            clause("blob2", &outcome.render());
        }
        if let Some(actor) = self.actor {
            clause("blob3", actor.name());
        }
        sql
    }
}

/// The rows one answer may carry. A dimension with more distinct values
/// than this is a dimension nobody reads to the bottom of, and an
/// unbounded answer is an unbounded response body.
pub const STATS_ROWS_MAX: u32 = 100;

/// One chosen question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatsQuery {
    /// What to count.
    pub subject: QuerySubject,
    /// What to group by.
    pub dimension: QueryDimension,
    /// How far back.
    pub window: QueryWindow,
    /// What to narrow to.
    pub filters: QueryFilters,
}

impl StatsQuery {
    /// Assemble a question, or refuse a pair that cannot be answered.
    ///
    /// The only inadmissible pairs are a bucket finer than the window
    /// can hold: hourly rows over a month is 720 of them against a cap
    /// of 100. Refusing beats truncating, because a truncated series is
    /// a chart that silently starts partway through its own window.
    #[must_use]
    pub fn new(
        subject: QuerySubject,
        dimension: QueryDimension,
        window: QueryWindow,
        filters: QueryFilters,
    ) -> Option<Self> {
        let query = Self {
            subject,
            dimension,
            window,
            filters,
        };
        match query.bucket_count() {
            Some(count) if count > u64::from(STATS_ROWS_MAX) => None,
            _ => Some(query),
        }
    }

    /// How many rows a bucketed answer carries, or `None` when the
    /// answer is a dimension list rather than a series.
    #[must_use]
    pub const fn bucket_count(&self) -> Option<u64> {
        match self.dimension.bucket_secs() {
            Some(bucket) => Some(self.window.secs() / bucket),
            None => None,
        }
    }

    /// The SQL for this question against one dataset.
    ///
    /// `dataset` is the deployment's own name from its configuration,
    /// never a caller's. Every other interpolation is a `&'static str`,
    /// an integer from an enum, or a name an enum produced, which is why
    /// no escaping appears here: there is nothing to escape.
    #[must_use]
    pub fn sql(&self, dataset: &str) -> String {
        // A series reads oldest first and is bounded by its own bucket
        // count; a dimension list reads largest first and is bounded by
        // the row cap. Both bounds are integers this type computed.
        let (order, limit) = match self.bucket_count() {
            Some(count) => ("dimension ASC", count),
            None => ("count DESC", u64::from(STATS_ROWS_MAX)),
        };
        format!(
            "SELECT {projection} AS dimension, \
             SUM(_sample_interval * double1) AS count, \
             SUM(_sample_interval * double2) AS bytes \
             FROM {dataset} \
             WHERE index1 = '{index}'{filters} \
             AND timestamp > NOW() - INTERVAL '{days}' DAY \
             GROUP BY dimension ORDER BY {order} LIMIT {limit}",
            projection = self.dimension.projection(),
            index = self.subject.index(),
            filters = self.filters.clauses(),
            days = self.window.days(),
        )
    }
}

/// One bucket of a series: when it starts, and what happened in it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeriesPoint {
    /// The bucket's first instant, in epoch seconds.
    pub start_secs: u64,
    /// How many things it counts.
    pub count: f64,
    /// Bytes, where the answer is bytes.
    pub bytes: f64,
}

/// Fill a bucketed answer to exactly the buckets its window implies.
///
/// Analytics Engine returns no row for a bucket nothing happened in, so
/// a raw answer is a series with holes in it, and a line drawn through
/// holes says traffic was smooth when it was absent. This produces one
/// point per bucket, ascending, ending at the bucket `now` falls in.
///
/// Three edges are decided here rather than left to a caller. A bucket
/// the dataset reported that falls outside the window is dropped,
/// because the platform stamps ingest time and the request clock is a
/// different clock, so a point can land one bucket past what this
/// request believes "now" to be. A duplicate start keeps the first,
/// because the SQL grouped by that expression and can only produce one.
/// And the series is anchored at the newest bucket rather than the
/// oldest, so a clock close enough to the epoch that the window would
/// reach behind it yields a shorter series instead of one that ends in
/// the future.
#[must_use]
pub fn fill_series(query: &StatsQuery, now_ms: u64, observed: &[SeriesPoint]) -> Vec<SeriesPoint> {
    let (Some(bucket), Some(count)) = (query.dimension.bucket_secs(), query.bucket_count()) else {
        return Vec::new();
    };
    let newest = (now_ms / 1_000 / bucket) * bucket;
    (0..count)
        .filter_map(|step| {
            let start_secs = newest.checked_sub(bucket.checked_mul(count - 1 - step)?)?;
            Some(
                observed
                    .iter()
                    .find(|point| point.start_secs == start_secs)
                    .copied()
                    .unwrap_or(SeriesPoint {
                        start_secs,
                        count: 0.0,
                        bytes: 0.0,
                    }),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one question every other test is written against.
    fn query(dimension: QueryDimension, window: QueryWindow) -> StatsQuery {
        StatsQuery::new(
            QuerySubject::Reads,
            dimension,
            window,
            QueryFilters::default(),
        )
        .expect("the fixture pairs are admissible")
    }

    #[test]
    fn a_chosen_question_reads_as_written() {
        let query = StatsQuery::new(
            QuerySubject::Writes,
            QueryDimension::Repository,
            QueryWindow::Week,
            QueryFilters::default(),
        )
        .expect("admissible");
        assert_eq!(
            query.sql("cachet_production"),
            "SELECT blob4 AS dimension, \
             SUM(_sample_interval * double1) AS count, \
             SUM(_sample_interval * double2) AS bytes \
             FROM cachet_production \
             WHERE index1 = 'write' \
             AND timestamp > NOW() - INTERVAL '7' DAY \
             GROUP BY dimension ORDER BY count DESC LIMIT 100"
        );
    }

    #[test]
    fn a_series_orders_by_time_and_is_bounded_by_its_buckets() {
        assert_eq!(
            query(QueryDimension::Day, QueryWindow::Week).sql("cachet_production"),
            "SELECT toString(intDiv(toUInt32(timestamp), 86400) * 86400) AS dimension, \
             SUM(_sample_interval * double1) AS count, \
             SUM(_sample_interval * double2) AS bytes \
             FROM cachet_production \
             WHERE index1 = 'read' \
             AND timestamp > NOW() - INTERVAL '7' DAY \
             GROUP BY dimension ORDER BY dimension ASC LIMIT 7"
        );
    }

    #[test]
    fn a_filtered_question_narrows_with_literals_only() {
        let query = StatsQuery::new(
            QuerySubject::Reads,
            QueryDimension::Outcome,
            QueryWindow::Week,
            QueryFilters {
                actor: Some(StatActor::Laptop),
                ..QueryFilters::default()
            },
        )
        .expect("admissible");
        assert_eq!(
            query.sql("cachet_production"),
            "SELECT blob2 AS dimension, \
             SUM(_sample_interval * double1) AS count, \
             SUM(_sample_interval * double2) AS bytes \
             FROM cachet_production \
             WHERE index1 = 'read' AND blob3 = 'laptop' \
             AND timestamp > NOW() - INTERVAL '7' DAY \
             GROUP BY dimension ORDER BY count DESC LIMIT 100"
        );
    }

    #[test]
    fn filters_stack_in_column_order() {
        let query = StatsQuery::new(
            QuerySubject::Reads,
            QueryDimension::Day,
            QueryWindow::Week,
            QueryFilters {
                kind: Some(StatKind::Narinfo),
                outcome: Some(StatOutcome::Status(404)),
                actor: Some(StatActor::Ci),
            },
        )
        .expect("admissible");
        assert!(
            query.sql("ds").contains(
                "WHERE index1 = 'read' AND blob1 = 'narinfo' AND blob2 = '404' AND blob3 = 'ci' "
            ),
            "{}",
            query.sql("ds")
        );
    }

    #[test]
    fn nothing_a_caller_writes_can_reach_the_statement() {
        // Every hostile shape a query string could carry. None of them
        // parse into a choice, so none of them are ever formatted: the
        // defence is the absence of a path, not an escape.
        for hostile in [
            "actor'; DROP TABLE x; --",
            "blob1, blob2",
            "1=1",
            "kind UNION SELECT 1",
            "",
            "KIND",
            "../kind",
            "laptop' OR '1'='1",
            "narinfo; --",
        ] {
            assert!(QueryDimension::parse(hostile).is_none(), "{hostile}");
            assert!(QuerySubject::parse(hostile).is_none(), "{hostile}");
            assert!(QueryWindow::parse(Some(hostile)).is_none(), "{hostile}");
            assert!(StatKind::parse(hostile).is_none(), "{hostile}");
            assert!(StatOutcome::parse(hostile).is_none(), "{hostile}");
            assert!(StatActor::parse(hostile).is_none(), "{hostile}");
        }
    }

    #[test]
    fn every_choice_maps_to_one_projection_and_back() {
        // A dimension whose projection collided with another would
        // silently answer the wrong question.
        let all = QueryDimension::all();
        let mut projections: Vec<String> = all.iter().map(|one| one.projection()).collect();
        projections.sort();
        projections.dedup();
        assert_eq!(projections.len(), all.len(), "one projection each");
        for dimension in all {
            assert_eq!(QueryDimension::parse(dimension.name()), Some(dimension));
        }
    }

    #[test]
    fn an_unstated_window_is_a_day_and_a_wrong_one_is_refused() {
        assert_eq!(QueryWindow::parse(None), Some(QueryWindow::Day));
        assert_eq!(QueryWindow::parse(Some("month")), Some(QueryWindow::Month));
        assert_eq!(QueryWindow::parse(Some("decade")), None);
    }

    #[test]
    fn a_bucket_finer_than_its_window_can_hold_is_refused() {
        let admissible = [
            (QueryDimension::Hour, QueryWindow::Day, 24),
            (QueryDimension::Day, QueryWindow::Day, 1),
            (QueryDimension::Day, QueryWindow::Week, 7),
            (QueryDimension::Day, QueryWindow::Month, 30),
        ];
        for (dimension, window, expected) in admissible {
            let query = StatsQuery::new(
                QuerySubject::Reads,
                dimension,
                window,
                QueryFilters::default(),
            )
            .expect("admissible");
            assert_eq!(query.bucket_count(), Some(expected), "{dimension:?}");
        }
        for window in [QueryWindow::Week, QueryWindow::Month] {
            assert!(
                StatsQuery::new(
                    QuerySubject::Reads,
                    QueryDimension::Hour,
                    window,
                    QueryFilters::default(),
                )
                .is_none(),
                "hourly over a {window:?} is past the row cap"
            );
        }
    }

    #[test]
    fn a_dimension_list_has_no_buckets_to_fill() {
        let query = query(QueryDimension::Actor, QueryWindow::Week);
        assert_eq!(query.bucket_count(), None);
        assert!(fill_series(&query, 1_700_000_000_000, &[]).is_empty());
    }

    #[test]
    fn an_empty_answer_fills_to_a_flat_series() {
        let query = query(QueryDimension::Day, QueryWindow::Week);
        // Midday, so the flooring has something to do.
        let now_ms = 1_700_000_000_000;
        let filled = fill_series(&query, now_ms, &[]);
        assert_eq!(filled.len(), 7);
        assert!(
            filled
                .iter()
                .all(|point| point.count.to_bits() == 0.0_f64.to_bits())
        );
        for pair in filled.windows(2) {
            assert_eq!(pair[1].start_secs - pair[0].start_secs, DAY_SECS);
        }
        assert_eq!(
            filled[6].start_secs,
            (now_ms / 1_000 / DAY_SECS) * DAY_SECS,
            "the last bucket is the one now falls in"
        );
    }

    #[test]
    fn observed_buckets_land_where_they_belong_and_strays_are_dropped() {
        let query = query(QueryDimension::Day, QueryWindow::Week);
        let now_ms = 1_700_000_000_000;
        let newest = (now_ms / 1_000 / DAY_SECS) * DAY_SECS;
        let filled = fill_series(
            &query,
            now_ms,
            &[
                SeriesPoint {
                    start_secs: newest,
                    count: 5.0,
                    bytes: 50.0,
                },
                SeriesPoint {
                    start_secs: newest - DAY_SECS * 3,
                    count: 2.0,
                    bytes: 20.0,
                },
                // One bucket into the future: the ingest clock and the
                // request clock are different clocks.
                SeriesPoint {
                    start_secs: newest + DAY_SECS,
                    count: 99.0,
                    bytes: 990.0,
                },
            ],
        );
        assert_eq!(filled.len(), 7);
        assert_eq!(filled[6].count.to_bits(), 5.0_f64.to_bits());
        assert_eq!(filled[3].count.to_bits(), 2.0_f64.to_bits());
        assert_eq!(filled[3].bytes.to_bits(), 20.0_f64.to_bits());
        assert!(
            filled
                .iter()
                .all(|point| point.count.to_bits() != 99.0_f64.to_bits()),
            "a bucket outside the window is not in the answer"
        );
    }
}
