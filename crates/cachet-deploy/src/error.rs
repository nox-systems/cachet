//! The deploy grammar's closed error taxonomy.
//!
//! Every variant names a shape the operator got wrong, and none of them
//! carries a value from the environment map. The map holds the signing
//! key and the OAuth client secret, and an error type that could carry a
//! value would eventually print one into a CI log. The property lane
//! asserts this over arbitrary maps.

use core::fmt;

/// Why a deployment plan could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployError {
    /// The deployment name is outside `^[a-z][a-z0-9-]{1,31}$`.
    BadName,
    /// Required values are absent. Every missing name at once, so the
    /// operator fixes the set rather than one failure per run (ADR 0009).
    Missing(Vec<&'static str>),
    /// A present value's shape is wrong. The name only.
    Malformed(&'static str),
    /// One value is set and another it depends on is not.
    Requires {
        /// The name that is set.
        present: &'static str,
        /// The name it needs.
        absent: &'static str,
    },
}

impl fmt::Display for DeployError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadName => f.write_str(
                "the deployment name is a lowercase word of 2 to 32 characters: \
                 a letter, then letters, digits, and dashes",
            ),
            Self::Missing(names) => {
                write!(
                    f,
                    "missing: {}. Set them in infra/.env.<name> or in the CI environment's variables and secrets (docs/DEPLOY.md)",
                    names.join(", ")
                )
            }
            Self::Malformed(name) => write!(f, "{name} is present and malformed"),
            Self::Requires { present, absent } => {
                write!(
                    f,
                    "{present} is set and {absent} is not; the first needs the second"
                )
            }
        }
    }
}

impl std::error::Error for DeployError {}
