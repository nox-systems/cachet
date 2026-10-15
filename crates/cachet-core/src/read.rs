//! The read path's decisions (CLAUDE.md §4): which headers a response
//! carries, what a miss looks like, and whether an object may live at the
//! edge. The worker performs the I/O; everything it could get subtly wrong
//! lives here, pure and testable.
//!
//! Two facts shape the module. Every cacheable answer carries an explicit
//! `Cache-Control` with a positive max-age, because the Cache API silently
//! refuses to store a response without one. And a 404 is a cacheable
//! answer, because nix reads it as "this cache does not have it" and moves
//! to the next substituter, where a 5xx would make nix retry us instead.

use crate::constants::{
    CACHE_INFO_CONTENT_TYPE, CACHE_INFO_EDGE_TTL_SECONDS, EDGE_CACHE_SIZE_CAP_BYTES,
    EDGE_NEGATIVE_TTL_SECONDS, GENERATION_EDGE_TTL_SECONDS, NAR_CONTENT_TYPE, NARINFO_CONTENT_TYPE,
    OBJECT_EDGE_TTL_SECONDS,
};
use crate::keys::NarKey;
use crate::types::UnixMillis;

/// Which kind of object a read serves: decides the content type. The TTL
/// is one value for both kinds (`OBJECT_EDGE_TTL_SECONDS`), because both
/// are content-addressed bytes behind generation-scoped keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    /// A `{hash}.narinfo` document.
    Narinfo,
    /// A `nar/{hash}.nar[...]` object.
    Nar,
}

impl ObjectKind {
    /// The content type nix expects for this kind.
    pub const fn content_type(self) -> &'static str {
        match self {
            Self::Narinfo => NARINFO_CONTENT_TYPE,
            Self::Nar => NAR_CONTENT_TYPE,
        }
    }

    /// The kind in logs.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Narinfo => "narinfo",
            Self::Nar => "nar",
        }
    }
}

/// The body of a plain 404: one line of text. Nix never reads it; its
/// presence makes human debugging legible.
pub const NOT_FOUND_BODY: &str = "not found\n";

/// Headers for a successful object read: the content type for the kind,
/// the exact length so a client can show progress and detect truncation,
/// an immutable edge lifetime, and the advertisement that a broken
/// download can resume. `immutable` is honest: the bytes behind a
/// content-derived key never change, and the generation prefix handles
/// invalidation after sweeps.
#[must_use]
pub fn object_response_headers(kind: ObjectKind, size_bytes: u64) -> [(&'static str, String); 4] {
    [
        ("content-type", kind.content_type().to_string()),
        ("content-length", size_bytes.to_string()),
        ("accept-ranges", "bytes".to_string()),
        (
            "cache-control",
            format!("public, max-age={OBJECT_EDGE_TTL_SECONDS}, immutable"),
        ),
    ]
}

/// One byte range a GET asked for, in the forms RFC 9110 defines for a
/// single range (section 14.1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteRange {
    /// `bytes=N-`: from offset N to the end. This is the form a client
    /// resuming a broken download sends.
    From(u64),
    /// `bytes=N-M`: offsets N through M, inclusive.
    Between(u64, u64),
    /// `bytes=-S`: the last S bytes.
    Last(u64),
}

/// The range a GET asks for, or `None` to serve the whole object.
///
/// A broken download of a large NAR resumes with `Range: bytes=N-`, and
/// curl, which nix downloads through, fails the transfer when the answer
/// is the whole object again ("Requested range was not delivered by the
/// server"), so serving the range is what lets a substitution survive a
/// dropped connection.
///
/// Everything this does not serve answers the whole object, which RFC
/// 9110 permits for a server that ignores a Range header (section 14.2):
/// a unit other than bytes, several ranges, a malformed range, and a
/// range whose last offset precedes its first. A request carrying
/// `If-Range` answers whole as well. The deployment sends no validator a
/// client could have kept, and the whole object is the answer the
/// condition calls for when a validator does not match (section 13.1.5),
/// so it is always a correct one.
#[must_use]
pub fn requested_range(range: Option<&str>, if_range: Option<&str>) -> Option<ByteRange> {
    if if_range.is_some() {
        return None;
    }
    let spec = range?.trim();
    let (unit, set) = spec.split_once('=')?;
    if !unit.trim().eq_ignore_ascii_case("bytes") || set.contains(',') {
        return None;
    }
    let (first, last) = set.trim().split_once('-')?;
    match (offset(first), offset(last)) {
        (Some(first), None) if last.is_empty() => Some(ByteRange::From(first)),
        (Some(first), Some(last)) if first <= last => Some(ByteRange::Between(first, last)),
        (None, Some(suffix)) if first.is_empty() => Some(ByteRange::Last(suffix)),
        _ => None,
    }
}

/// A decimal offset, saturating at `u64::MAX`. why saturate: an offset
/// too large for any object still names a place past its end, which is
/// a 416 rather than a range to ignore.
fn offset(digits: &str) -> Option<u64> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(digits.bytes().fold(0_u64, |value, digit| {
        value
            .saturating_mul(10)
            .saturating_add(u64::from(digit - b'0'))
    }))
}

/// The bytes a range covers in an object of `size_bytes`, as inclusive
/// first and last offsets, or `None` when it covers none of them, which
/// answers 416. A last offset past the end stops at the end, and a suffix
/// longer than the object is the whole object (RFC 9110 section 14.1.2).
#[must_use]
pub fn resolve_range(range: ByteRange, size_bytes: u64) -> Option<(u64, u64)> {
    let end = size_bytes.checked_sub(1)?;
    match range {
        ByteRange::From(first) => (first <= end).then_some((first, end)),
        ByteRange::Between(first, last) => (first <= end).then_some((first, last.min(end))),
        ByteRange::Last(0) => None,
        ByteRange::Last(suffix) => Some((size_bytes.saturating_sub(suffix), end)),
    }
}

/// Headers for a 206 answering a range: the object's own headers, with
/// the length of the part and where it sits in the whole.
#[must_use]
pub fn partial_response_headers(
    kind: ObjectKind,
    first: u64,
    last: u64,
    size_bytes: u64,
) -> [(&'static str, String); 5] {
    [
        ("content-type", kind.content_type().to_string()),
        ("content-length", (last - first + 1).to_string()),
        (
            "content-range",
            format!("bytes {first}-{last}/{size_bytes}"),
        ),
        ("accept-ranges", "bytes".to_string()),
        (
            "cache-control",
            format!("public, max-age={OBJECT_EDGE_TTL_SECONDS}, immutable"),
        ),
    ]
}

/// The header a 416 carries beside its problem body: the object's size,
/// so a client can see where the object ends (RFC 9110 section 15.5.17).
#[must_use]
pub fn unsatisfiable_range_header(size_bytes: u64) -> (&'static str, String) {
    ("content-range", format!("bytes */{size_bytes}"))
}

/// Headers for an object we do not have: a short positive max-age so the
/// edge caches the miss. Nix asks about a path before every substitution,
/// so an uncached 404 costs a bucket round trip per query; the short
/// window means a freshly pushed path becomes visible almost immediately.
#[must_use]
pub fn not_found_response_headers() -> [(&'static str, String); 2] {
    [
        ("content-type", "text/plain; charset=utf-8".to_string()),
        (
            "cache-control",
            format!("public, max-age={EDGE_NEGATIVE_TTL_SECONDS}"),
        ),
    ]
}

/// Headers for `/nix-cache-info`: the one unauthenticated response. Its
/// edge lifetime is short because the body changes with configuration.
#[must_use]
pub fn cache_info_response_headers() -> [(&'static str, String); 2] {
    [
        ("content-type", CACHE_INFO_CONTENT_TYPE.to_string()),
        (
            "cache-control",
            format!("public, max-age={CACHE_INFO_EDGE_TTL_SECONDS}"),
        ),
    ]
}

/// Headers for the generation document as cached at the edge: the short
/// TTL is the staleness bound after a destructive sweep.
#[must_use]
pub fn generation_response_headers() -> [(&'static str, String); 2] {
    [
        ("content-type", "application/json".to_string()),
        (
            "cache-control",
            format!("public, max-age={GENERATION_EDGE_TTL_SECONDS}"),
        ),
    ]
}

/// Whether a read may delete a narinfo on a head it already made: the
/// head that found `found_missing` absent from the bucket, made for this
/// colo's stale copy of the same narinfo.
///
/// It may when the narinfo the bucket holds names that same NAR and was
/// stored before this request began. A push writes a NAR before the
/// narinfo that names it, so a narinfo stored after the request began
/// can belong to a push that stored the NAR after the head, and deleting
/// it on that head would throw away the repair the push just made. The
/// read heads the NAR again instead.
#[must_use]
pub fn missing_answer_holds(
    found_missing: &NarKey,
    named: &NarKey,
    narinfo_stored_at: UnixMillis,
    request_started: UnixMillis,
) -> bool {
    found_missing == named && narinfo_stored_at < request_started
}

/// Whether an object of this size may be stored at the edge. Larger
/// objects stream from the bucket on every read: the response works, it
/// just costs a bucket operation each time, which the worker logs.
#[must_use]
pub const fn is_edge_cacheable(size_bytes: u64) -> bool {
    size_bytes <= EDGE_CACHE_SIZE_CAP_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_headers_carry_the_kind_and_an_explicit_ttl() {
        let narinfo = object_response_headers(ObjectKind::Narinfo, 1234);
        assert_eq!(
            narinfo,
            [
                ("content-type", "text/x-nix-narinfo".to_string()),
                ("content-length", "1234".to_string()),
                ("accept-ranges", "bytes".to_string()),
                (
                    "cache-control",
                    "public, max-age=2592000, immutable".to_string()
                ),
            ]
        );
        let nar = object_response_headers(ObjectKind::Nar, 0);
        assert_eq!(nar[0], ("content-type", NAR_CONTENT_TYPE.to_string()));
        assert!(nar[3].1.contains("immutable"));
    }

    #[test]
    fn a_resume_asks_for_the_rest_of_the_object() {
        assert_eq!(
            requested_range(Some("bytes=87664593-"), None),
            Some(ByteRange::From(87_664_593))
        );
        assert_eq!(
            resolve_range(ByteRange::From(87_664_593), 100_000_000),
            Some((87_664_593, 99_999_999))
        );
    }

    #[test]
    fn the_single_range_forms_parse() {
        for (header, range) in [
            ("bytes=0-", ByteRange::From(0)),
            ("bytes=0-0", ByteRange::Between(0, 0)),
            ("bytes=10-19", ByteRange::Between(10, 19)),
            ("bytes=-5", ByteRange::Last(5)),
            ("Bytes = 7-", ByteRange::From(7)),
            (" bytes=1-2 ", ByteRange::Between(1, 2)),
        ] {
            assert_eq!(requested_range(Some(header), None), Some(range), "{header}");
        }
    }

    #[test]
    fn whatever_is_not_served_answers_whole() {
        for header in [
            "bytes=5-2",
            "bytes=0-1,4-5",
            "items=0-1",
            "bytes=",
            "bytes=-",
            "bytes=a-",
            "bytes=+1-",
            "bytes=1",
            "0-1",
            "bytes=-1-2",
        ] {
            assert_eq!(requested_range(Some(header), None), None, "{header}");
        }
        assert_eq!(requested_range(None, None), None);
        assert_eq!(
            requested_range(Some("bytes=10-"), Some("\"etag\"")),
            None,
            "If-Range answers whole"
        );
    }

    #[test]
    fn a_range_is_clamped_to_the_object_or_refused() {
        assert_eq!(
            resolve_range(ByteRange::Between(10, 999), 20),
            Some((10, 19))
        );
        assert_eq!(resolve_range(ByteRange::From(19), 20), Some((19, 19)));
        assert_eq!(resolve_range(ByteRange::From(20), 20), None, "past the end");
        assert_eq!(resolve_range(ByteRange::Last(5), 20), Some((15, 19)));
        assert_eq!(resolve_range(ByteRange::Last(50), 20), Some((0, 19)));
        assert_eq!(resolve_range(ByteRange::Last(0), 20), None);
        assert_eq!(
            resolve_range(ByteRange::From(0), 0),
            None,
            "an empty object"
        );
        // An offset no u64 holds is still past the end, never a panic.
        let huge = requested_range(Some("bytes=99999999999999999999999-"), None).expect("a range");
        assert_eq!(huge, ByteRange::From(u64::MAX));
        assert_eq!(resolve_range(huge, 20), None);
    }

    #[test]
    fn a_partial_answer_says_where_it_sits() {
        assert_eq!(
            partial_response_headers(ObjectKind::Nar, 10, 19, 100),
            [
                ("content-type", NAR_CONTENT_TYPE.to_string()),
                ("content-length", "10".to_string()),
                ("content-range", "bytes 10-19/100".to_string()),
                ("accept-ranges", "bytes".to_string()),
                (
                    "cache-control",
                    "public, max-age=2592000, immutable".to_string()
                ),
            ]
        );
        assert_eq!(
            unsatisfiable_range_header(100),
            ("content-range", "bytes */100".to_string())
        );
    }

    #[test]
    fn a_missing_object_is_cacheable_briefly() {
        let headers = not_found_response_headers();
        assert_eq!(headers[1].1, "public, max-age=30");
        assert_eq!(NOT_FOUND_BODY, "not found\n");
    }

    #[test]
    fn a_missing_answer_holds_only_for_a_narinfo_older_than_the_request() {
        let nar = |letter: &str| {
            crate::keys::parse_nar_key(&format!("nar/{}.nar.zst", letter.repeat(52)))
                .expect("valid nar key")
        };
        let (a, b) = (nar("a"), nar("b"));
        let started = UnixMillis::new(1_000);
        let before = UnixMillis::new(999);
        let during = UnixMillis::new(1_001);
        assert!(missing_answer_holds(&a, &a, before, started));
        // A narinfo stored as the request began, or during it, may belong
        // to a push that stored the NAR after the head.
        assert!(!missing_answer_holds(&a, &a, started, started));
        assert!(!missing_answer_holds(&a, &a, during, started));
        // A narinfo naming another NAR needs a head of its own.
        assert!(!missing_answer_holds(&a, &b, before, started));
    }

    #[test]
    fn the_size_cap_is_the_edge_limit() {
        assert!(is_edge_cacheable(EDGE_CACHE_SIZE_CAP_BYTES));
        assert!(!is_edge_cacheable(EDGE_CACHE_SIZE_CAP_BYTES + 1));
    }

    #[test]
    fn cache_info_headers_carry_their_short_ttl() {
        let headers = cache_info_response_headers();
        assert_eq!(headers[0].1, "text/x-nix-cache-info");
        assert_eq!(headers[1].1, "public, max-age=300");
    }
}
