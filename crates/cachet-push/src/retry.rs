//! The retry policy as pure shape: counts, delays, and the failure line.
//! Retries are the whole mitigation against a flaky connection, and the
//! envelope is small enough to review as data.

/// Total attempts per operation.
///
/// Five rather than three. The previous envelope was sized for a client
/// that re-sent a buffered body on every attempt, so each retry cost a
/// copy of up to ninety mebibytes and giving up early was the cheap
/// answer. A body is now a refcount or a file range, so an attempt costs
/// the wire and nothing else, and a push that dies four requests into a
/// three-thousand-path run has to start over.
pub const RETRY_MAX: u32 = 5;

/// The base of the backoff, in milliseconds.
pub const RETRY_BASE_DELAY_MS: u64 = 500;

/// The sleep after attempt `tries` (zero-based) fails: the base doubled
/// once per attempt. There is no sleep after the final attempt, whose
/// failure returns instead.
///
/// Doubling rather than the previous linear step. Sixteen uploads are in
/// flight at once, so the failures a retry answers are usually the edge
/// shedding load from all of them together; stepping back linearly means
/// every one of them returns at nearly the same moment and sheds again.
pub fn attempt_delay_ms(tries: u32) -> u64 {
    RETRY_BASE_DELAY_MS.saturating_mul(1_u64 << tries.min(6))
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
    fn the_envelope_doubles_and_then_gives_up() {
        assert_eq!(delay_after(0), Some(500));
        assert_eq!(delay_after(1), Some(1_000));
        assert_eq!(delay_after(2), Some(2_000));
        assert_eq!(delay_after(3), Some(4_000));
        assert_eq!(delay_after(4), None, "the last attempt returns its failure");
    }

    #[test]
    fn the_backoff_never_overflows() {
        // The shift is bounded, so a caller that asks past the envelope
        // gets a large delay rather than a panic.
        assert!(attempt_delay_ms(u32::MAX) > 0);
    }
}
