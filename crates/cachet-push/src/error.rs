//! The failure vocabulary: the push never fails the job, and its lines
//! carry the reason in full. Display text is the diagnostic contract
//! operators grep for.

use core::fmt;

use cachet_core::constants::PUSH_PATHS_MAX;

/// Everything that stops the push short. CI maps any of these to one
/// stderr line and a green job; the text stays close to the previous
/// implementation's wording because runbooks still quote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushError {
    /// A required input was missing, named for the operator.
    MissingConfig {
        /// The unset variables, in the documented order.
        missing: Vec<&'static str>,
    },
    /// A nix command failed.
    CommandFailed {
        /// The invocation as printed.
        argv: String,
        /// The failure text the command gave.
        message: String,
    },
    /// The diff outgrew the candidate cap.
    TooManyCandidates(usize),
    /// The OIDC request failed before it issued a token.
    OidcUnavailable(String),
    /// The OIDC response carried no token.
    OidcEmpty,
    /// An upload HTTP call failed its attempts.
    UploadFailed {
        /// The operation label: `PUT {key}`, `part N of {key}`, and so on.
        what: String,
        /// How many attempts ran.
        attempts: u32,
        /// The last failure text.
        last: String,
    },
    /// The object exceeded what the plan can ship.
    TooLarge {
        /// The staging-relative key.
        key: String,
    },
    /// A staging object had an implausible size.
    ImplausibleSize {
        /// The staging-relative key.
        key: String,
    },
    /// A mid-attempt complaint with only its detail worth repeating: the
    /// retry envelope writes the label and the count around it.
    Detail {
        /// The complaint.
        message: String,
    },
    /// The staging directory could not be read.
    StagingUnreadable {
        /// The failure text.
        message: String,
    },
}

impl fmt::Display for PushError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingConfig { missing } => write!(
                f,
                "nothing pushed, because {} {} unset. The cachet-setup composite action exports \
                 these to the job environment; if you are running the post action directly, set \
                 them yourself.",
                missing.join(", "),
                if missing.len() == 1 { "is" } else { "are" },
            ),
            Self::CommandFailed { argv, message } => write!(f, "{argv} failed: {message}"),
            Self::TooManyCandidates(count) => {
                write!(
                    f,
                    "this job added {count} store paths, more than the {PUSH_PATHS_MAX} cap"
                )
            }
            Self::OidcUnavailable(message) => write!(f, "{message}"),
            Self::OidcEmpty => write!(f, "the OIDC token response carried no token"),
            Self::UploadFailed {
                what,
                attempts,
                last,
            } => write!(f, "{what} failed after {attempts} attempts: {last}"),
            Self::TooLarge { key } => write!(f, "{key} is too large to upload"),
            Self::ImplausibleSize { key } => write!(f, "{key} has an implausible size"),
            Self::Detail { message } => f.write_str(message),
            Self::StagingUnreadable { message } => {
                write!(f, "could not read the staging directory: {message}")
            }
        }
    }
}

impl std::error::Error for PushError {}
