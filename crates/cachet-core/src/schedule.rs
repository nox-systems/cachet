//! When the collector next runs.
//!
//! The cron expression lives in the deploy program, which is the only
//! place that knows what schedule the worker was created with, so the
//! worker learns it as configuration and reads it here. A console
//! counting down to the next collection is the only caller: nothing in
//! the collector's own path consults this, because the platform decides
//! when to invoke the scheduled handler and the worker never asks.
//!
//! Only the daily shape is recognized. A deployment's cron is
//! `M H * * *` by construction (ADR 0005), and a parser that guessed at
//! the rest of cron's grammar would answer a countdown that quietly
//! disagreed with the platform. An unrecognized expression answers
//! nothing, and a caller shows no countdown rather than a wrong one.

/// Seconds in a day. Unix time has no leap seconds, so every UTC day is
/// exactly this long and the arithmetic below needs no calendar.
const DAY_SECS: u64 = 86_400;

/// A daily schedule: one firing per UTC day, at a fixed time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DailySchedule {
    /// The hour it fires, 0 through 23.
    pub hour: u64,
    /// The minute within that hour, 0 through 59.
    pub minute: u64,
}

impl DailySchedule {
    /// Read a five-field cron expression, if it names a daily firing.
    ///
    /// `0 5 * * *` is the shape a deployment ships with. Anything with a
    /// list, a range, a step, or a day restriction answers `None`,
    /// because this recognizes one shape rather than implementing cron.
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
        let hour = plain_number(hour, 23)?;
        Some(Self { hour, minute })
    }

    /// The next firing at or after `now`, in epoch milliseconds.
    ///
    /// Ties go forward: a request arriving exactly on the minute the
    /// collector fires counts down to tomorrow's, because the run it is
    /// asking about is already happening.
    #[must_use]
    pub fn next_after_ms(self, now_ms: u64) -> u64 {
        let now_secs = now_ms / 1_000;
        let midnight = (now_secs / DAY_SECS) * DAY_SECS;
        let today = midnight + self.hour * 3_600 + self.minute * 60;
        let next = if today > now_secs {
            today
        } else {
            today + DAY_SECS
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
    fn the_shipped_schedule_reads_as_five_in_the_morning() {
        assert_eq!(
            DailySchedule::parse("0 5 * * *"),
            Some(DailySchedule { hour: 5, minute: 0 })
        );
        assert_eq!(
            DailySchedule::parse("30 23 * * *"),
            Some(DailySchedule {
                hour: 23,
                minute: 30
            })
        );
    }

    #[test]
    fn anything_that_is_not_a_daily_firing_answers_nothing() {
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
            "0 5 * *",
            "0 5 * * * *",
            "",
            "   ",
            "0 24 * * *",
            "60 5 * * *",
            "0 005 * * *",
            "x 5 * * *",
            "-1 5 * * *",
        ] {
            assert_eq!(DailySchedule::parse(expression), None, "{expression:?}");
        }
    }

    #[test]
    fn the_countdown_crosses_midnight_and_never_points_at_now() {
        let schedule = DailySchedule { hour: 5, minute: 0 };
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
}
