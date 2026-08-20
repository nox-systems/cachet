//! Key and path validation: the boundary between a URL and a bucket key.
//!
//! Nothing reaches the bucket without crossing this module, which makes it
//! the path-traversal guard as much as a format check. There is no
//! escaping, normalising, or sanitising step, because a validator that
//! transforms its input is a validator you have to reason about twice.
//! Every function is total: arbitrary bytes yield a typed failure, never a
//! panic (CLAUDE.md §7).

use crate::constants::{
    KEY_BYTES_MAX, NAR_FILE_HASH_LENGTH, NAR_KEY_PREFIX, NAR_SUFFIXES, NARINFO_KEY_SUFFIX,
    NIX_BASE32_ALPHABET, NIX_STORE_DIR, RESERVED_KEY_PREFIXES, STORE_PATH_HASH_LENGTH,
    STORE_PATH_NAME_BYTES_MAX,
};
use crate::error::{ClientError, Result};
use crate::types::{ProjectName, StorePathHash, StorePathParts};

/// A validated NAR bucket key: `nar/<52 nix base32>.nar[.suffix]`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NarKey(String);

impl NarKey {
    /// Borrow the validated key text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The 52-character file hash embedded in the key.
    pub fn file_hash(&self) -> &str {
        &self.0[NAR_KEY_PREFIX.len()..NAR_KEY_PREFIX.len() + NAR_FILE_HASH_LENGTH]
    }

    /// The compression suffix, `""` for an uncompressed NAR.
    pub fn suffix(&self) -> &str {
        &self.0[NAR_KEY_PREFIX.len() + NAR_FILE_HASH_LENGTH + ".nar".len()..]
    }
}

impl core::fmt::Display for NarKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Validate a narinfo request path, with or without its leading slash.
///
/// ```
/// use cachet_core::keys::parse_narinfo_request_path;
/// let hash = parse_narinfo_request_path("/0123456789abcdfghijklmnpqrsvwxyz.narinfo").unwrap();
/// assert_eq!(hash.as_str(), "0123456789abcdfghijklmnpqrsvwxyz");
/// ```
pub fn parse_narinfo_request_path(request_path: &str) -> Result<StorePathHash> {
    let bounded = bounded_path(request_path)?;
    let hash = bounded
        .strip_suffix(NARINFO_KEY_SUFFIX)
        .ok_or(ClientError::MalformedKey)?;
    StorePathHash::parse(hash)
}

/// Validate a NAR request path. The returned key is the bucket key
/// verbatim, because the cache's URL space and the bucket's key space are
/// the same for NARs.
pub fn parse_nar_request_path(request_path: &str) -> Result<NarKey> {
    parse_nar_key(bounded_path(request_path)?)
}

/// Validate a NAR key as it appears in a narinfo's `URL` field. Same shape
/// as a request path, different provenance: this one arrives inside a
/// document a client uploaded, and it decides which object the write path
/// probes for the never-dangle invariant. A loose check here would let a
/// narinfo point at an object outside the NAR key space.
pub fn parse_nar_key(text: &str) -> Result<NarKey> {
    if text.len() > KEY_BYTES_MAX {
        return Err(ClientError::MalformedKey);
    }
    let Some(rest) = text.strip_prefix(NAR_KEY_PREFIX) else {
        return Err(ClientError::MalformedKey);
    };
    let Some(hash) = rest.get(..NAR_FILE_HASH_LENGTH) else {
        return Err(ClientError::MalformedKey);
    };
    // why: byte membership over the alphabet's bytes, not char iteration;
    // see StorePathHash::parse for the reasoning.
    if !hash
        .bytes()
        .all(|b| NIX_BASE32_ALPHABET.as_bytes().contains(&b))
    {
        return Err(ClientError::MalformedKey);
    }
    let Some(suffix) = rest.get(NAR_FILE_HASH_LENGTH..) else {
        return Err(ClientError::MalformedKey);
    };
    let Some(nar_suffix) = suffix.strip_prefix(".nar") else {
        return Err(ClientError::MalformedKey);
    };
    if !NAR_SUFFIXES.contains(&nar_suffix) {
        return Err(ClientError::MalformedKey);
    }
    Ok(NarKey(text.to_string()))
}

/// Validate a full store path and split it into hash and name.
///
/// Used for the `StorePath` field of a narinfo and for every entry in a
/// roots payload.
pub fn parse_store_path(text: &str) -> Result<StorePathParts> {
    let prefix = format!("{NIX_STORE_DIR}/");
    if text.len() > prefix.len() + STORE_PATH_HASH_LENGTH + 1 + STORE_PATH_NAME_BYTES_MAX {
        return Err(ClientError::MalformedKey);
    }
    let basename = text
        .strip_prefix(&prefix)
        .ok_or(ClientError::MalformedKey)?;
    // Paired enforcement: a traversal like `<hash>-<name>/../etc` is
    // refused here by the separator check and again by the name charset
    // below, which admits no `/`. Either check alone suffices today; both
    // exist so that widening the name charset later cannot silently open a
    // traversal.
    if basename.contains('/') {
        return Err(ClientError::MalformedKey);
    }
    parse_store_path_basename(basename)
}

/// Validate a `<hash>-<name>` basename, the form the `References` field
/// uses. This is the reference graph's entry point, and therefore what the
/// closure walk depends on.
pub fn parse_store_path_basename(basename: &str) -> Result<StorePathParts> {
    if basename.len() < STORE_PATH_HASH_LENGTH + 2 {
        return Err(ClientError::MalformedKey);
    }
    if basename.as_bytes()[STORE_PATH_HASH_LENGTH] != b'-' {
        return Err(ClientError::MalformedKey);
    }
    let hash = StorePathHash::parse(&basename[..STORE_PATH_HASH_LENGTH])?;
    let name = &basename[STORE_PATH_HASH_LENGTH + 1..];
    if name.len() > STORE_PATH_NAME_BYTES_MAX {
        return Err(ClientError::MalformedKey);
    }
    // why: nix permits letters, digits, and `+-._?=` in a name; `/` is
    // notably absent, which is what stops a name from smuggling a path
    // segment.
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.' | b'_' | b'?' | b'='))
    {
        return Err(ClientError::MalformedKey);
    }
    Ok(StorePathParts {
        hash,
        name: name.to_string(),
    })
}

/// Whether a key belongs to one of the internal prefixes. Key validation
/// already makes these unreachable from a request; the sweep's candidate
/// filter asks again on the deletion side, where being wrong is
/// unrecoverable.
pub fn is_reserved_key(key: &str) -> bool {
    RESERVED_KEY_PREFIXES
        .iter()
        .any(|prefix| key.starts_with(prefix))
}

/// The bucket key a project's lease lives under.
pub fn lease_key_for_project(project: &ProjectName) -> String {
    format!("{}{project}", crate::constants::ROOTS_KEY_PREFIX)
}

/// Recover a project name from a lease key.
pub fn project_from_lease_key(key: &str) -> Result<ProjectName> {
    let name = key
        .strip_prefix(crate::constants::ROOTS_KEY_PREFIX)
        .ok_or(ClientError::MalformedKey)?;
    ProjectName::parse(name)
}

/// Bound a request path and strip its leading slash. An empty path and a
/// path over the cap are typed failures: an assertion here once let a
/// client-supplied string raise a 500 where the contract requires a 400, so
/// this function accepts a path with or without the slash and always
/// answers in the error vocabulary.
fn bounded_path(request_path: &str) -> Result<&str> {
    // A cheap upper bound before touching the string, so a pathological
    // path costs one comparison. The +1 covers a leading slash.
    if request_path.len() > KEY_BYTES_MAX + 1 {
        return Err(ClientError::MalformedKey);
    }
    let trimmed = request_path.strip_prefix('/').unwrap_or(request_path);
    if trimmed.is_empty() {
        return Err(ClientError::MalformedKey);
    }
    if trimmed.len() > KEY_BYTES_MAX {
        return Err(ClientError::MalformedKey);
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The full alphabet is itself a valid hash, and it keeps every test
    // free of magic strings.
    const HASH32: &str = "0123456789abcdfghijklmnpqrsvwxyz";

    #[test]
    fn narinfo_path_roundtrips_without_slash() {
        assert_eq!(
            parse_narinfo_request_path(&format!("{HASH32}.narinfo"))
                .unwrap()
                .as_str(),
            HASH32
        );
    }

    #[test]
    fn traversal_is_refused_everywhere() {
        for path in [
            "/../etc/passwd",
            "/nar/../../x.nar.zst",
            "/%2e%2e/hidden.narinfo",
            "/nar/../0000000000000000000000000000000000000000000000000000.nar",
        ] {
            assert!(parse_nar_request_path(path).is_err(), "{path} refused");
        }
    }

    #[test]
    fn nar_key_accepts_every_suffix() {
        let hash = "x".repeat(52);
        for suffix in ["", ".xz", ".zst", ".bz2", ".gz", ".br", ".lzip", ".lz4"] {
            let key = format!("nar/{hash}.nar{suffix}");
            let parsed = parse_nar_key(&key).unwrap_or_else(|_| panic!("{key} parses"));
            assert_eq!(parsed.suffix(), suffix);
        }
        assert!(parse_nar_key(&format!("nar/{hash}.nar.zstd")).is_err());
    }

    #[test]
    fn store_path_name_charset_refuses_separator_and_space() {
        let name = format!("/nix/store/{HASH32}-bash-5.2");
        assert!(parse_store_path(&name).is_ok());
        assert!(parse_store_path(&format!("{name}/extra")).is_err());
        assert!(parse_store_path(&format!("{name} a")).is_err());
    }

    #[test]
    fn reserved_prefixes_are_refused_twice() {
        for key in [
            "roots/a",
            "uploads/u1",
            "gc-runs/r/1",
            "gc-reports/r",
            "meta/generation",
        ] {
            assert!(is_reserved_key(key), "{key} reserved");
        }
        assert!(!is_reserved_key(&format!("nar/{}.nar", "x".repeat(52))));
    }

    #[test]
    fn project_names_follow_the_grammar() {
        assert!(ProjectName::parse("my-org-my-repo").is_ok());
        assert!(ProjectName::parse("a.b_c-d0").is_ok());
        for bad in ["", ".dot", "-dash", "..", "a..b", &"x".repeat(141)] {
            assert!(ProjectName::parse(bad).is_err(), "{bad} refused");
        }
    }

    #[test]
    fn repository_hyphenates_into_project() {
        let project = ProjectName::from_repository("my-org/my-repo").unwrap();
        assert_eq!(project.as_str(), "my-org-my-repo");
    }
}
