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
/// and an immutable edge lifetime. `immutable` is honest: the bytes behind
/// a content-derived key never change, and the generation prefix handles
/// invalidation after sweeps.
#[must_use]
pub fn object_response_headers(kind: ObjectKind, size_bytes: u64) -> [(&'static str, String); 3] {
    [
        ("content-type", kind.content_type().to_string()),
        ("content-length", size_bytes.to_string()),
        (
            "cache-control",
            format!("public, max-age={OBJECT_EDGE_TTL_SECONDS}, immutable"),
        ),
    ]
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
                (
                    "cache-control",
                    "public, max-age=2592000, immutable".to_string()
                ),
            ]
        );
        let nar = object_response_headers(ObjectKind::Nar, 0);
        assert_eq!(nar[0], ("content-type", NAR_CONTENT_TYPE.to_string()));
        assert!(nar[2].1.contains("immutable"));
    }

    #[test]
    fn a_missing_object_is_cacheable_briefly() {
        let headers = not_found_response_headers();
        assert_eq!(headers[1].1, "public, max-age=30");
        assert_eq!(NOT_FOUND_BODY, "not found\n");
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
