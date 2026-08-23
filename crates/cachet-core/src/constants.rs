//! Named wire constants shared by every crate (CLAUDE.md §4, §6). Bounds
//! stay here so the worker, the CLI, and the action cannot disagree about a
//! limit by drifting copies. Every constant cites the reason for its value.

/// The exact body of `GET /nix-cache-info`. Priority 30 sorts cachet ahead
/// of cache.nixos.org (priority 40) in a client's substituter order, and
/// WantMassQuery permits nix to batch narinfo lookups.
pub const NIX_CACHE_INFO: &str = "StoreDir: /nix/store\nWantMassQuery: 1\nPriority: 30\n";

/// The nix base32 alphabet: lowercase, excluding e, o, u, and t. Store-path
/// hashes are 32 chars; NAR file hashes are 52.
pub const NIX_BASE32_ALPHABET: &str = "0123456789abcdfghijklmnpqrsvwxyz";

/// Store-path hashes carry exactly this many nix base32 characters.
pub const STORE_PATH_HASH_LENGTH: usize = 32;

/// NAR file hashes carry exactly this many nix base32 characters.
pub const NAR_FILE_HASH_LENGTH: usize = 52;

/// The store directory every cachet narinfo names.
pub const NIX_STORE_DIR: &str = "/nix/store";

/// A store path's name half: letters, digits, and `+-._?=`. Nix admits
/// exactly this set, and it excludes `/`, which is what stops a name from
/// smuggling a path segment.
pub const STORE_PATH_NAME_PATTERN: &str = r"^[a-zA-Z0-9+._?=-]+$";

/// The longest store path name cachet accepts. 211 matches nixpkgs'
/// longest known name plus headroom; nix itself never publishes longer.
pub const STORE_PATH_NAME_BYTES_MAX: usize = 211;

/// The suffix identifying a narinfo object.
pub const NARINFO_KEY_SUFFIX: &str = ".narinfo";

/// The prefix under which every NAR object lives.
pub const NAR_KEY_PREFIX: &str = "nar/";

/// The compression suffixes nix appends to a NAR key, by `Compression`
/// field value. Read paths accept exactly this set; the signing path
/// accepts only `""` and `.zst`.
pub const NAR_SUFFIXES: [&str; 8] = ["", ".xz", ".zst", ".bz2", ".gz", ".br", ".lzip", ".lz4"];

/// The longest legal key is well under 70 bytes; anything over 128 is
/// refused by a length comparison before any pattern scan runs.
pub const KEY_BYTES_MAX: usize = 128;

/// A narinfo body larger than this is refused with body_too_large: narinfos
/// are a few hundred bytes in practice, so 64 KiB is generous headroom and
/// still a hard guard in front of the parser.
pub const NARINFO_BYTES_MAX: u64 = 65_536;

/// A narinfo with more lines than this is refused: the parser's work stays
/// linear and bounded regardless of content.
pub const NARINFO_LINES_MAX: usize = 2_048;

/// One narinfo may name at most this many references. Caps both the parse
/// and the closure walk's fan-out per node.
pub const NARINFO_REFERENCES_MAX: usize = 1_024;

/// The roots (lease renewal) payload is at most this many bytes: it is a
/// lease, not a bulk upload channel.
pub const ROOTS_BODY_BYTES_MAX: u64 = 524_288;

/// A roots payload carries at most this many store paths or installables,
/// matching the push pipeline's closure cap.
pub const ROOTS_PATHS_MAX: usize = 4_096;

/// The collector enumerates at most this many project leases per run.
pub const ROOTS_PROJECTS_MAX: usize = 256;

/// An Authorization header longer than this is refused as malformed_auth:
/// JWTs run a few kilobytes, so 8 KiB is generous room while still a hard
/// guard in front of the parsers.
pub const AUTH_HEADER_BYTES_MAX: usize = 8_192;

/// The nix `Compression` value narinfos default to when the field is
/// absent.
pub const DEFAULT_COMPRESSION: &str = "bzip2";

/// The sentinel value for provenance fields missing from a lease: claims
/// old documents may predate, readable as absence rather than as truth.
pub const UNKNOWN_CLAIM: &str = "unknown";

/// A project name is at most this long. GitHub caps an owner at 39
/// characters and a repository at 100, so 140 covers the longest
/// owner-repository default that can exist.
pub const PROJECT_NAME_BYTES_MAX: usize = 140;

/// A project name: alphanumerics, dots, underscores, and dashes, starting
/// alphanumeric; never containing `..`.
pub const PROJECT_NAME_PATTERN: &str = r"^[A-Za-z0-9][A-Za-z0-9._-]*$";

/// One lease document per project under this prefix.
pub const ROOTS_KEY_PREFIX: &str = "roots/";

/// In-flight multipart bookkeeping lives under this prefix, unreachable
/// from any request by construction.
pub const UPLOADS_KEY_PREFIX: &str = "uploads/";

/// The GC's per-run stage artifacts live under this prefix.
pub const GC_RUNS_KEY_PREFIX: &str = "gc-runs/";

/// The GC's run reports live under this prefix.
pub const GC_REPORTS_KEY_PREFIX: &str = "gc-reports/";

/// The edge-cache generation document's key.
pub const GENERATION_OBJECT_KEY: &str = "meta/generation";

/// A well-formed generation document is under a hundred bytes; anything
/// approaching 256 is corrupt, and the read path bypasses the edge cache
/// rather than trusting it.
pub const GENERATION_DOCUMENT_BYTES_MAX: u64 = 256;

/// The internal prefixes that requests can never address. Key validation
/// makes these unreachable from a request; the sweep's candidate filter
/// refuses them a second time, because deletion is unrecoverable.
pub const RESERVED_KEY_PREFIXES: [&str; 5] = [
    ROOTS_KEY_PREFIX,
    UPLOADS_KEY_PREFIX,
    GC_RUNS_KEY_PREFIX,
    GC_REPORTS_KEY_PREFIX,
    "meta/",
];

/// Single-shot NAR PUT refuses bodies larger than this with body_too_large:
/// 90 MiB of headroom under Cloudflare's 100 MB edge request cap. Larger
/// NARs use the multipart routes.
pub const UPLOAD_SINGLE_MAX_BYTES: u64 = 94_371_800;

/// A multipart completion body: 1000 parts at 256 bytes each is room to
/// spare, and the cap exists so the parser's bound precedes the read.
pub const COMPLETE_BODY_BYTES_MAX: u64 = 262_144;

/// An upload bookkeeping record: four small fields; anything larger is
/// not one.
pub const UPLOAD_RECORD_BYTES_MAX: u64 = 1_024;

/// Every multipart part except the last is exactly this size; the last is
/// the declared remainder.
pub const UPLOAD_PART_BYTES: u64 = 67_108_864;

/// The multipart protocol rejects a plan with more than this many parts.
pub const MULTIPART_PARTS_MAX: u64 = 1_000;

/// The push pipeline refuses closures adding more than this many store
/// paths. The guard's target is the missing-snapshot disaster (a before-set
/// that died, making the whole store one diff), not honest cold closures:
/// a fresh runner's devshell plus build closure legitimately lands in the
/// thousands, so the cap sits well above any first push and far below the
/// upload-everything shape.
pub const PUSH_PATHS_MAX: u64 = 16_384;

/// An in-flight upload older than this is aborted: a week is far past any
/// honest client's retry window.
pub const UPLOAD_STALE_MAX_MS: u64 = 7 * MILLIS_PER_DAY;

/// A lease renewed longer ago than this no longer protects its closure.
pub const LEASE_RETENTION_MS: u64 = 30 * MILLIS_PER_DAY;

/// An unmarked object younger than the grace window survives the sweep:
/// in-flight writes and just-missed leases get one collection cycle to
/// settle.
pub const GRACE_WINDOW_MS: u64 = 14 * MILLIS_PER_DAY;

/// A run that would delete more than this fraction of the inventory is
/// refused: a cache that empties overnight is worse than a cache that
/// grows. Kept as an integer ratio (the previous float compared after
/// lossy casts).
pub const SWEEP_MAX_FRACTION_NUMERATOR: usize = 1;

/// The denominator half of [`SWEEP_MAX_FRACTION_NUMERATOR`]: 1/4.
pub const SWEEP_MAX_FRACTION_DENOMINATOR: usize = 4;

/// The closure walk refuses to grow past this many visited paths, which
/// also bounds narinfo reads per run.
pub const CLOSURE_WALK_PATHS_MAX: usize = 100_000;

/// GC run artifacts older than this are pruned; reports keep forever.
pub const GC_RUNS_RETENTION_MS: u64 = 30 * MILLIS_PER_DAY;

/// Binding operations one GC invocation may spend before it hands the run
/// to the next tick: list pages, reads, puts, and delete batches count one
/// each.
pub const GC_OP_BUDGET: u64 = 900;

/// Wall time one invocation may use before handing off, in milliseconds.
/// The scheduled-event ceiling is fifteen minutes and the margin absorbs
/// the final stage's writes.
pub const GC_HEADROOM_MS: u64 = 13 * 60 * 1_000;

/// Run entries per gc-runs listing page: an admin page is a bounded read,
/// and deeper history pages through.
pub const GC_RUNS_PAGE_LIMIT: usize = 100;

/// The index key the latest completed report's copy lives under, so the
/// stats endpoint answers from one read instead of a listing scan.
pub const GC_LATEST_REPORT_KEY: &str = "gc-reports/latest.json";

/// Keys per deletion call: one batch is one binding operation, and the
/// batch shape is what keeps a 900-operation tick honest about real
/// deletions.
pub const GC_DELETE_BATCH: usize = 128;

/// The run cursor's bucket key, inside the reserved prefix: the sweep
/// grammar and every request path already refuse it.
pub const GC_CURSOR_OBJECT_KEY: &str = "meta/gc-cursor";

/// GitHub rotates its JWKS on its own schedule; ten minutes bounds our
/// staleness.
pub const JWKS_CACHE_TTL_MS: u64 = 600_000;

/// Clock-skew tolerated on OIDC times, in milliseconds: five seconds
/// absorbs CAQ Runner clock drift without widening the replay window
/// materially.
pub const OIDC_CLOCK_TOLERANCE_MS: u64 = 5_000;

/// A configuration may name at most this many GitHub orgs. More is far more
/// likely a delimiter mistake than a decision.
pub const ACCEPTED_ORGS_MAX: usize = 8;

/// The verdict-cache lifetime for an allowed GitHub token: revoking a
/// laptop credential converges within this window.
pub const VERDICT_ALLOW_TTL_MS: u64 = 600_000;

/// The verdict-cache lifetime for a denied GitHub token: membership grants
/// converge quickly without hammering the GitHub API.
pub const VERDICT_DENY_TTL_MS: u64 = 60_000;

/// An OAuth state token lives this long: long enough for a human to finish
/// the GitHub prompt, short enough to bound replay.
pub const OAUTH_STATE_TTL_MS: u64 = 600_000;

/// A browser session lives this long from its creation.
pub const SESSION_TTL_MS: u64 = 14 * MILLIS_PER_DAY;

/// Objects per bucket-listing page: a parametric ceiling, not a knob.
pub const BUCKET_LIST_PAGE_LIMIT: usize = 1_000;

/// The KV prefix under which GitHub-token verdicts live:
/// `ghverdict/{sha256}`.
pub const VERDICT_KEY_PREFIX: &str = "ghverdict/";

/// The KV prefix under which browser sessions live: `sess/{id}`.
pub const SESSION_KEY_PREFIX: &str = "sess/";

/// The KV prefix for OAuth state tickets: `oauth-state/{state}`.
pub const OAUTH_STATE_KEY_PREFIX: &str = "oauth-state/";

/// The session cookie's name.
pub const SESSION_COOKIE_NAME: &str = "cachet_session";

/// Milliseconds in a day, the unit conversion used by the retention
/// constants.
pub const MILLIS_PER_DAY: u64 = 86_400_000;

/// Negative answers to the edge cache last this long: a missing narinfo is
/// usually being written this second.
pub const EDGE_NEGATIVE_TTL_SECONDS: u32 = 30;

/// The generation document's own edge TTL: the staleness bound after a
/// destructive sweep.
pub const GENERATION_EDGE_TTL_SECONDS: u32 = 60;

/// Positive edge-cache lifetime for cache objects: thirty days, immutable,
/// because an object under a content-derived key never changes.
pub const OBJECT_EDGE_TTL_SECONDS: u32 = 2_592_000;

/// Objects larger than this stream without touching the edge cache: R2
/// streams them fine, and one early-release closure would evict everything
/// worth keeping.
pub const EDGE_CACHE_SIZE_CAP_BYTES: u64 = 512 * 1024 * 1024;

/// The synthetic origin every edge-cache key is built under. The Cache API
/// keys on URLs, and `.invalid` is reserved by RFC 2606, so nothing here
/// can ever become a real request.
pub const EDGE_CACHE_KEY_ORIGIN: &str = "https://cachet-edge.invalid";

/// The `/nix-cache-info` body changes with configuration, so its edge
/// lifetime is short: five minutes.
pub const CACHE_INFO_EDGE_TTL_SECONDS: u32 = 300;

/// The content type nix expects for a narinfo.
pub const NARINFO_CONTENT_TYPE: &str = "text/x-nix-narinfo";

/// The content type nix expects for a NAR object.
pub const NAR_CONTENT_TYPE: &str = "application/x-nix-nar";

/// The content type nix expects for `/nix-cache-info`.
pub const CACHE_INFO_CONTENT_TYPE: &str = "text/x-nix-cache-info";

/// The content type of an RFC 9457 problem document.
pub const PROBLEM_CONTENT_TYPE: &str = "application/problem+json";

/// The OIDC issuer's JWKS document for verification of GitHub Actions
/// tokens. `CACHET_JWKS_URL` overrides it only in the lanes and for
/// issuers that are not github.com.
pub const JWKS_URL_DEFAULT: &str = "https://token.actions.githubusercontent.com/.well-known/jwks";

/// The GitHub REST API root the verdict path and OAuth flows call.
/// `CACHET_GITHUB_API_URL` overrides it for the stub server in the lanes.
pub const GITHUB_API_URL_DEFAULT: &str = "https://api.github.com";

/// The GitHub web origin the OAuth exchange posts to. The device flow and
/// the authorize redirect live on the same origin by construction;
/// `CACHET_GITHUB_WEB_URL` overrides it for the stub server in the lanes.
pub const GITHUB_WEB_URL_DEFAULT: &str = "https://github.com";

/// An explicitly set value wins; an absent or blank value reads as the
/// default. This is the whole override rule the worker's outbound-URL
/// reads share, kept in one place so no call site invents its own
/// interpretation of an empty string.
pub fn override_or<'a>(value: Option<&'a str>, default: &'a str) -> &'a str {
    value.filter(|v| !v.trim().is_empty()).unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_answer_is_the_default() {
        assert_eq!(override_or(None, JWKS_URL_DEFAULT), JWKS_URL_DEFAULT);
    }

    #[test]
    fn blank_answers_read_as_absent() {
        assert_eq!(override_or(Some(""), JWKS_URL_DEFAULT), JWKS_URL_DEFAULT);
        assert_eq!(
            override_or(Some("  "), GITHUB_API_URL_DEFAULT),
            GITHUB_API_URL_DEFAULT
        );
    }

    #[test]
    fn a_set_value_wins() {
        let stub = "http://127.0.0.1:9/jwks";
        assert_eq!(override_or(Some(stub), JWKS_URL_DEFAULT), stub);
    }
}
