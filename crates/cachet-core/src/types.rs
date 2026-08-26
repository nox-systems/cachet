//! Branded types with validating constructors (CLAUDE.md §4). A value that
//! exists has already passed its grammar check, so downstream code handles
//! the shape, not the defense. Constructors are total: arbitrary input
//! yields a typed [`ClientError`], never a panic.

use core::fmt;

use crate::constants::{NIX_BASE32_ALPHABET, PROJECT_NAME_BYTES_MAX, STORE_PATH_HASH_LENGTH};
use crate::error::{ClientError, Result};

/// Exactly 32 characters of the nix base32 alphabet: the hash half of a
/// store path, and the name of every narinfo object.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StorePathHash(String);

impl StorePathHash {
    /// The total validating constructor.
    pub fn parse(text: &str) -> Result<Self> {
        if text.len() != STORE_PATH_HASH_LENGTH {
            return Err(ClientError::MalformedKey);
        }
        // why: byte membership over the alphabet's bytes, not char
        // iteration: the alphabet is all-ASCII so the checks are identical,
        // and `str::contains(char)` spins a Chars iterator that both the CPU
        // and the kani lane pay for.
        if !text
            .bytes()
            .all(|b| NIX_BASE32_ALPHABET.as_bytes().contains(&b))
        {
            return Err(ClientError::MalformedKey);
        }
        Ok(Self(text.to_string()))
    }

    /// Borrow the validated text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StorePathHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A nix store path, split into the parts cachet works with: the hash and
/// the name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StorePathParts {
    /// The 32-character nix base32 hash.
    pub hash: StorePathHash,
    /// The name half, charset-validated.
    pub name: String,
}

/// A project name as a `roots/<project>` key segment accepts: 1-140 bytes
/// of `[A-Za-z0-9._-]`, starting alphanumeric, never containing `..`. The
/// last rule refuses a name that reads as a parent-directory reference even
/// though a single `.` is legal; a lease key is a bucket key, and hugging
/// the separator is a hazard, not a feature.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProjectName(String);

impl ProjectName {
    /// The total validating constructor.
    pub fn parse(text: &str) -> Result<Self> {
        // Length before shape, so a hostile name costs a comparison.
        if text.is_empty() || text.len() > PROJECT_NAME_BYTES_MAX {
            return Err(ClientError::MalformedKey);
        }
        let mut bytes = text.bytes();
        if !matches!(bytes.next(), Some(b) if b.is_ascii_alphanumeric()) {
            return Err(ClientError::MalformedKey);
        }
        if !text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        {
            return Err(ClientError::MalformedKey);
        }
        if text.contains("..") {
            return Err(ClientError::MalformedKey);
        }
        // why: folded, because this becomes a bucket key and GitHub is
        // case-insensitive about the repository it is derived from. A
        // renewal whose URL wrote Owner-Repo and whose token said
        // owner/repo would otherwise bind to two different leases, and a
        // read of one would miss the other.
        Ok(Self(crate::auth::fold_identifier(text)))
    }

    /// Borrow the validated text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The derived project for a GitHub repository: owner and repo joined
    /// by a hyphen. Write-side lease renewal binds to exactly this (the
    /// previous implementation let any org repo renew any project's
    /// lease; that hole is closed).
    pub fn from_repository(repository: &str) -> Result<Self> {
        let hyphenated = repository.replace('/', "-");
        Self::parse(&hyphenated)
    }
}

impl fmt::Display for ProjectName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Milliseconds since the unix epoch. Every timestamp in the system is one
/// of these, in one unit, so arithmetic stays integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnixMillis(u64);

impl UnixMillis {
    /// Wrap a raw value; range checking is the caller's decision (the value
    /// may be a duration field being parsed for arithmetic).
    pub const fn new(ms: u64) -> Self {
        Self(ms)
    }

    /// The raw value.
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Checked addition for ttl arithmetic.
    pub fn add_millis(self, delta: u64) -> Option<Self> {
        Some(Self(self.0.checked_add(delta)?))
    }

    /// The difference in milliseconds, saturating at zero for a backwards
    /// clock reading.
    pub fn saturating_ms_since(self, earlier: Self) -> u64 {
        self.0.saturating_sub(earlier.0)
    }
}
