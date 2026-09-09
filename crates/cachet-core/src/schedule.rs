//! When the collector next runs.
//!
//! The cron expression lives in the deploy program, which is the only
//! place that knows what schedule the worker was created with, so the
//! worker learns it as configuration and reads it here. Two callers use
//! it, both on the health route: a console counting down to the next
//! collection, and the staleness bound that decides whether a
//! deployment's collector is working. Nothing in the collector's own
//! path consults this, because the platform decides when to invoke the
//! scheduled handler and the worker never asks.
//!
//! Two shapes are recognized, the two a deployment ships with: `M * * * *`
//! for the hourly schedule and `M H * * *` for a daily one. A parser that
//! guessed at the rest of cron's grammar would answer a countdown that
//! quietly disagreed with the platform, so anything else answers nothing
//! and a caller shows no countdown rather than a wrong one.

/// Seconds in a day. Unix time has no leap seconds, so every UTC day is
/// exactly this long and the arithmetic below needs no calendar.
const DAY_SECS: u64 = 86_400;

/// Seconds in an hour.
const HOUR_SECS: u64 = 3_600;

/// The collector's schedule, as one of the two shapes a deployment ships.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schedule {
    /// One firing per hour, at a fixed minute: `M * * * *`.
    Hourly {
        /// The minute within each hour, 0 through 59.
        minute: u64,
    },
    /// One firing per UTC day, at a fixed time: `M H * * *`.
    Daily {
        /// The hour it fires, 0 through 23.
        hour: u64,
        /// The minute within that hour, 0 through 59.
        minute: u64,
    },
}

impl Schedule {
    /// Read a five-field cron expression, if it names one of the two
    /// shapes.
    ///
    /// `0 * * * *` is what a deployment ships with. Anything with a list,
    /// a range, a step, or a day restriction answers `None`, because this
    /// recognizes two shapes rather than implementing cron.
    #[must_use]
    pub fn parse(expression: &str) -> Option<Self> {
        let fields: Vec<&str> = expression.split_whitespace().collect();
        let [minute, hour, day_of_month, month, day_of_week] = fields.as_slice() else {
            return None;
        };
        if *day_of_month != "*" || *month != "*" || *day_of_week != "*" {
            return None;
        }
        let minute = plain_number(minute, 59)?;
        if *hour == "*" {
            return Some(Self::Hourly { minute });
        }
        let hour = plain_number(hour, 23)?;
        Some(Self::Daily { hour, minute })
    }

    /// How long one firing is from the next, in milliseconds.
    ///
    /// The health route's staleness bound is two of these. A bound in
    /// days would read every hourly schedule as healthy for
    /// forty-eight firings, which is a colour that stops meaning
    /// anything.
    #[must_use]
    pub fn period_ms(self) -> u64 {
        match self {
            Self::Hourly { .. } => HOUR_SECS * 1_000,
            Self::Daily { .. } => DAY_SECS * 1_000,
        }
    }

    /// The next firing at or after `now`, in epoch milliseconds.
    ///
    /// Ties go forward: a request arriving exactly on the minute the
    /// collector fires counts down to the firing after it, because the
    /// run it is asking about is already happening.
    #[must_use]
    pub fn next_after_ms(self, now_ms: u64) -> u64 {
        let now_secs = now_ms / 1_000;
        let (period, offset) = match self {
            Self::Hourly { minute } => (HOUR_SECS, minute * 60),
            Self::Daily { hour, minute } => (DAY_SECS, hour * HOUR_SECS + minute * 60),
        };
        let boundary = (now_secs / period) * period;
        let this_period = boundary + offset;
        let next = if this_period > now_secs {
            this_period
        } else {
            this_period + period
        };
        next * 1_000
    }
}

/// A field that is only digits, within a bound, and not written with a
/// leading zero run long enough to be something else.
fn plain_number(field: &str, max: u64) -> Option<u64> {
    if field.is_empty() || field.len() > 2 || !field.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let value: u64 = field.parse().ok()?;
    (value <= max).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_schedule_reads_as_the_top_of_every_hour() {
        assert_eq!(
            Schedule::parse("0 * * * *"),
            Some(Schedule::Hourly { minute: 0 })
        );
        assert_eq!(
            Schedule::parse("30 * * * *"),
            Some(Schedule::Hourly { minute: 30 })
        );
    }

    #[test]
    fn a_pinned_hour_still_reads_as_daily() {
        assert_eq!(
            Schedule::parse("0 5 * * *"),
            Some(Schedule::Daily { hour: 5, minute: 0 })
        );
        assert_eq!(
            Schedule::parse("30 23 * * *"),
            Some(Schedule::Daily {
                hour: 23,
                minute: 30
            })
        );
    }

    #[test]
    fn each_shape_reports_its_own_period() {
        // The health route multiplies this by two, so an hourly
        // deployment goes degraded after two missed hours where a daily
        // one has two days.
        assert_eq!(Schedule::Hourly { minute: 0 }.period_ms(), 3_600_000);
        assert_eq!(
            Schedule::Daily { hour: 5, minute: 0 }.period_ms(),
            86_400_000
        );
    }

    #[test]
    fn anything_that_is_not_one_of_the_two_shapes_answers_nothing() {
        // Each of these is a legal cron expression this does not
        // implement. Answering a countdown for one would put a number on
        // screen that the platform disagrees with.
        for expression in [
            "0 5 * * 1",
            "0 5 1 * *",
            "0 5 * 6 *",
            "*/15 * * * *",
            "0 5,17 * * *",
            "0 1-5 * * *",
            "* * * * *",
            "0 5 * *",
            "0 5 * * * *",
            "",
            "   ",
            "0 24 * * *",
            "60 5 * * *",
            "60 * * * *",
            "0 005 * * *",
            "x 5 * * *",
            "-1 5 * * *",
        ] {
            assert_eq!(Schedule::parse(expression), None, "{expression:?}");
        }
    }

    #[test]
    fn the_daily_countdown_crosses_midnight_and_never_points_at_now() {
        let schedule = Schedule::Daily { hour: 5, minute: 0 };
        let midnight = 1_780_000_000_000_u64 / 86_400_000 * 86_400_000;
        // Before the firing: later today.
        assert_eq!(
            schedule.next_after_ms(midnight + 3_600_000),
            midnight + 5 * 3_600_000
        );
        // After it: tomorrow.
        assert_eq!(
            schedule.next_after_ms(midnight + 6 * 3_600_000),
            midnight + 86_400_000 + 5 * 3_600_000
        );
        // Exactly on it: tomorrow, because today's run is happening now.
        assert_eq!(
            schedule.next_after_ms(midnight + 5 * 3_600_000),
            midnight + 86_400_000 + 5 * 3_600_000
        );
    }

    #[test]
    fn the_hourly_countdown_crosses_the_hour_and_never_points_at_now() {
        let schedule = Schedule::Hourly { minute: 15 };
        let hour = 1_780_000_000_000_u64 / 3_600_000 * 3_600_000;
        // Before the firing: later this hour.
        assert_eq!(schedule.next_after_ms(hour + 60_000), hour + 15 * 60_000);
        // After it: next hour.
        assert_eq!(
            schedule.next_after_ms(hour + 20 * 60_000),
            hour + 3_600_000 + 15 * 60_000
        );
        // Exactly on it: next hour, because this one is happening now.
        assert_eq!(
            schedule.next_after_ms(hour + 15 * 60_000),
            hour + 3_600_000 + 15 * 60_000
        );
    }
}
