//! The retry policy as pure shape: counts, delays, and the failure line.
//! Retries are the whole mitigation against a flaky connection, and the
//! envelope is small enough to review as data.

/// Total attempts per operation.
pub const RETRY_MAX: u32 = 3;
/// The base of the linear backoff, in milliseconds.
pub const RETRY_BASE_DELAY_MS: u64 = 500;

/// The sleep after attempt `tries` (zero-based) fails: base times
/// `tries + 1`. There is no sleep after the final attempt — its failure
/// returns instead.
pub fn attempt_delay_ms(tries: u32) -> u64 {
    RETRY_BASE_DELAY_MS * u64::from(tries + 1)
}

/// Sleep decisions: how long after a failed attempt, if it is not the
/// last.
pub fn delay_after(tries: u32) -> Option<u64> {
    (tries + 1 < RETRY_MAX).then(|| attempt_delay_ms(tries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_envelope_is_500_then_1000_then_done() {
        assert_eq!(delay_after(0), Some(500));
        assert_eq!(delay_after(1), Some(1_000));
        assert_eq!(delay_after(2), None);
    }
}
