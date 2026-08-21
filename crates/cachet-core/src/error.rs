//! The error taxonomy (CLAUDE.md §7). Every client-reachable failure is one
//! of these codes with a fixed HTTP status; the worker renders it as an RFC
//! 9457 problem+json body. The table only grows: previous codes and their
//! statuses never change, so deployed clients keep matching.

use core::fmt;

/// A client-reachable failure with its wire vocabulary. One enum, one
/// table: the mapping from a code to its status is a single reviewable
/// fact, and a condition a client can cause lands here rather than in an
/// assertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientError {
    /// Key length or format rejected; also the path-traversal guard.
    MalformedKey,
    /// The narinfo did not parse, or a required field was missing or
    /// malformed.
    MalformedNarinfo,
    /// The roots payload did not parse, or an entry was not a valid store
    /// path.
    MalformedRoots,
    /// The Authorization header was unparseable or oversized.
    MalformedAuth,
    /// A multipart part number outside `1..=expectedParts`.
    PartNumberInvalid,
    /// A part whose length violates the uniform-part-size rule.
    PartSizeMismatch,
    /// The completion part set was not exactly the expected parts, or
    /// contained duplicates.
    CompletePartsMismatch,
    /// A narinfo's compression suffix is not one the signing path verifies.
    UnsupportedCompression,
    /// The narinfo's `StorePath` hash disagrees with the request's key.
    StorePathMismatch,
    /// The stored NAR's compressed hash disagrees with the narinfo's
    /// FileHash or FileSize.
    FileHashMismatch,
    /// The stored NAR's decompressed hash or size disagrees with the
    /// narinfo's NarHash or NarSize.
    NarHashMismatch,
    /// No credential, or one that did not verify.
    Unauthorized,
    /// A valid OIDC token from outside the configured org.
    ForbiddenOrg,
    /// Lease renewal attempted from a ref other than the configured
    /// default branch.
    ForbiddenRef,
    /// Lease renewal naming a project other than the token's repository.
    ForbiddenProject,
    /// No such object. Nix reads 404 as a cache miss and moves to the next
    /// substituter, so a build never fails on it.
    NotFound,
    /// The upload id has no bookkeeping record: unknown, already
    /// completed, or aborted.
    UploadUnknown,
    /// A narinfo whose NAR is absent. The write-time half of the
    /// never-dangle invariant: the uploader's NAR-first ordering is
    /// verified here rather than trusted.
    NarinfoNarMissing,
    /// A write without Content-Length: the size guard cannot run, so the
    /// write cannot.
    LengthRequired,
    /// Body or payload over its cap.
    BodyTooLarge,
    /// The JWKS could not be fetched and no fresh cached copy exists, so
    /// an OIDC token can be neither accepted nor honestly rejected. The
    /// 503 is never a bypass: it tells the client to retry.
    AuthUnavailable,
    /// A bucket operation the request depended on failed. Distinct from
    /// every 4xx because the client did nothing wrong.
    StorageUnavailable,
    /// The OAuth callback query did not parse, or one of its required
    /// parameters was absent.
    MalformedOauth,
    /// The OAuth state named in a callback never existed, was already
    /// consumed, or outlived its window: the flow starts over.
    OauthStateUnknown,
    /// A valid org member outside the configured admins, asking for an
    /// admin route.
    ForbiddenAdmin,
}

impl ClientError {
    /// The HTTP status the worker answers with.
    pub const fn status(self) -> u16 {
        match self {
            Self::MalformedKey
            | Self::MalformedNarinfo
            | Self::MalformedRoots
            | Self::MalformedAuth
            | Self::PartNumberInvalid
            | Self::PartSizeMismatch
            | Self::CompletePartsMismatch
            | Self::UnsupportedCompression
            | Self::StorePathMismatch
            | Self::FileHashMismatch
            | Self::NarHashMismatch
            | Self::MalformedOauth => 400,
            Self::Unauthorized | Self::OauthStateUnknown => 401,
            Self::ForbiddenOrg
            | Self::ForbiddenRef
            | Self::ForbiddenProject
            | Self::ForbiddenAdmin => 403,
            Self::NotFound | Self::UploadUnknown => 404,
            Self::NarinfoNarMissing => 409,
            Self::LengthRequired => 411,
            Self::BodyTooLarge => 413,
            Self::AuthUnavailable | Self::StorageUnavailable => 503,
        }
    }

    /// The `code` field of the problem+json body.
    pub const fn code(self) -> &'static str {
        match self {
            Self::MalformedKey => "malformed_key",
            Self::MalformedNarinfo => "malformed_narinfo",
            Self::MalformedRoots => "malformed_roots",
            Self::MalformedAuth => "malformed_auth",
            Self::PartNumberInvalid => "part_number_invalid",
            Self::PartSizeMismatch => "part_size_mismatch",
            Self::CompletePartsMismatch => "complete_parts_mismatch",
            Self::UnsupportedCompression => "unsupported_compression",
            Self::StorePathMismatch => "store_path_mismatch",
            Self::FileHashMismatch => "file_hash_mismatch",
            Self::NarHashMismatch => "nar_hash_mismatch",
            Self::Unauthorized => "unauthorized",
            Self::ForbiddenOrg => "forbidden_org",
            Self::ForbiddenRef => "forbidden_ref",
            Self::ForbiddenProject => "forbidden_project",
            Self::NotFound => "not_found",
            Self::UploadUnknown => "upload_unknown",
            Self::NarinfoNarMissing => "narinfo_nar_missing",
            Self::LengthRequired => "length_required",
            Self::BodyTooLarge => "body_too_large",
            Self::AuthUnavailable => "auth_unavailable",
            Self::StorageUnavailable => "storage_unavailable",
            Self::MalformedOauth => "malformed_oauth",
            Self::OauthStateUnknown => "oauth_state_unknown",
            Self::ForbiddenAdmin => "forbidden_admin",
        }
    }

    /// The `title` field of the problem+json body: the code in words.
    /// Occurrence specifics go to worker logs rather than the body, so the
    /// body tells a client only what it may know.
    pub const fn title(self) -> &'static str {
        match self {
            Self::MalformedKey => "key grammar rejected",
            Self::MalformedNarinfo => "narinfo did not parse",
            Self::MalformedRoots => "roots payload did not parse",
            Self::MalformedAuth => "authorization header did not parse",
            Self::PartNumberInvalid => "multipart part number out of range",
            Self::PartSizeMismatch => "multipart part size is not the uniform size",
            Self::CompletePartsMismatch => "multipart completion set is not the expected part set",
            Self::UnsupportedCompression => "compression is not one the signing path verifies",
            Self::StorePathMismatch => "store path and request key disagree",
            Self::FileHashMismatch => "stored file hash disagrees with the narinfo",
            Self::NarHashMismatch => "stored NAR hash disagrees with the narinfo",
            Self::Unauthorized => "missing or invalid credential",
            Self::ForbiddenOrg => "outside the configured org",
            Self::ForbiddenRef => "not the configured default branch",
            Self::ForbiddenProject => "project does not match the token's repository",
            Self::NotFound => "no such object",
            Self::UploadUnknown => "unknown upload",
            Self::NarinfoNarMissing => "the narinfo's NAR is absent",
            Self::LengthRequired => "write without a content length",
            Self::BodyTooLarge => "body over its cap",
            Self::AuthUnavailable => "authentication backend unavailable",
            Self::StorageUnavailable => "storage backend unavailable",
            Self::MalformedOauth => "oauth request did not parse",
            Self::OauthStateUnknown => "oauth state unknown or expired",
            Self::ForbiddenAdmin => "outside the configured admins",
        }
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for ClientError {}

/// The shared result type: parse and validation outcomes are always typed
/// failures, never panics.
pub type Result<T> = std::result::Result<T, ClientError>;
