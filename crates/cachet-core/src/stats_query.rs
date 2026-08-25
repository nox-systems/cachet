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

    /// The `index1` value this subject selects on.
    const fn index(self) -> &'static str {
        match self {
            Self::Reads => "read",
            Self::Writes => "write",
            Self::Probes => "probe",
        }
    }
}

/// Which column to group by. One per dimension the writers fill, and
/// nothing else: the mapping to a blob position lives here so a caller
/// never names one.
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
    /// Which lease.
    Project,
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
            "project" => Some(Self::Project),
            _ => None,
        }
    }

    /// The blob column this dimension lives in, matching
    /// `stats::StatPoint::blobs`.
    const fn column(self) -> &'static str {
        match self {
            Self::Kind => "blob1",
            Self::Outcome => "blob2",
            Self::Actor => "blob3",
            Self::Repository => "blob4",
            Self::Reference => "blob5",
            Self::Project => "blob6",
        }
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

    /// The interval's day count. An integer, so it is a literal in the
    /// emitted SQL rather than caller text.
    const fn days(self) -> u8 {
        match self {
            Self::Day => 1,
            Self::Week => 7,
            Self::Month => 30,
        }
    }
}

/// One chosen question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatsQuery {
    /// What to count.
    pub subject: QuerySubject,
    /// What to group by.
    pub dimension: QueryDimension,
    /// How far back.
    pub window: QueryWindow,
}

/// The rows one answer may carry. A dimension with more distinct values
/// than this is a dimension nobody reads to the bottom of, and an
/// unbounded answer is an unbounded response body.
pub const STATS_ROWS_MAX: u32 = 100;

impl StatsQuery {
    /// The SQL for this question against one dataset.
    ///
    /// `dataset` is the deployment's own name from its configuration,
    /// never a caller's. Every other interpolation is a `&'static str`
    /// or an integer from an enum, which is why no escaping appears
    /// here: there is nothing to escape.
    #[must_use]
    pub fn sql(&self, dataset: &str) -> String {
        format!(
            "SELECT {dimension} AS dimension, \
             SUM(_sample_interval * double1) AS count, \
             SUM(_sample_interval * double2) AS bytes \
             FROM {dataset} \
             WHERE index1 = '{index}' AND timestamp > NOW() - INTERVAL '{days}' DAY \
             GROUP BY dimension ORDER BY count DESC LIMIT {STATS_ROWS_MAX}",
            dimension = self.dimension.column(),
            index = self.subject.index(),
            days = self.window.days(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chosen_question_reads_as_written() {
        let query = StatsQuery {
            subject: QuerySubject::Writes,
            dimension: QueryDimension::Repository,
            window: QueryWindow::Week,
        };
        let sql = query.sql("cachet_production");
        assert!(sql.contains("blob4 AS dimension"), "{sql}");
        assert!(sql.contains("index1 = 'write'"), "{sql}");
        assert!(sql.contains("INTERVAL '7' DAY"), "{sql}");
        assert!(sql.contains("FROM cachet_production"), "{sql}");
        assert!(sql.ends_with("LIMIT 100"), "{sql}");
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
        ] {
            assert!(QueryDimension::parse(hostile).is_none(), "{hostile}");
            assert!(QuerySubject::parse(hostile).is_none(), "{hostile}");
            assert!(QueryWindow::parse(Some(hostile)).is_none(), "{hostile}");
        }
    }

    #[test]
    fn every_choice_maps_to_one_column_and_back() {
        // A dimension whose column collided with another would silently
        // answer the wrong question.
        let all = [
            QueryDimension::Kind,
            QueryDimension::Outcome,
            QueryDimension::Actor,
            QueryDimension::Repository,
            QueryDimension::Reference,
            QueryDimension::Project,
        ];
        let mut columns: Vec<&str> = all.iter().map(|one| one.column()).collect();
        columns.sort_unstable();
        columns.dedup();
        assert_eq!(
            columns.len(),
            all.len(),
            "every dimension has its own column"
        );
    }

    #[test]
    fn an_unstated_window_is_a_day_and_a_wrong_one_is_refused() {
        assert_eq!(QueryWindow::parse(None), Some(QueryWindow::Day));
        assert_eq!(QueryWindow::parse(Some("month")), Some(QueryWindow::Month));
        assert_eq!(QueryWindow::parse(Some("decade")), None);
    }
}
