//! The upload plan: per-object mechanics and URL construction. Every
//! decision here is pure data; the pipeline executes what these functions
//! answer.

use cachet_core::constants::UPLOAD_SINGLE_MAX_BYTES;
use cachet_core::multipart::{PlanShape, plan_shape};

use crate::error::PushError;

/// One object's ride: the whole body in one PUT, or the multipart
/// quartet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadMechanics {
    /// `PUT /{key}` with the whole body.
    Single,
    /// The multipart plan: part count and final length from the shared
    /// part math.
    Multipart(PlanShape),
}

/// Decide the mechanics. Nix's stat answers the size; an implausible
/// answer fails before any byte moves. The single-PUT cap compares
/// inclusively: exactly-at-cap rides single, as the worker's guard
/// agrees. The multipart refusal delegates to the shared plan's own
/// reasoning.
///
/// # Errors
///
/// [`PushError::TooLarge`] past the parts cap;
/// [`PushError::ImplausibleSize`]!? — sizes come from stat so this branch
/// exists for completeness against adapters, expecting never to fire.
pub fn plan_mechanics(key: &str, size_bytes: u64) -> Result<UploadMechanics, PushError> {
    if size_bytes <= UPLOAD_SINGLE_MAX_BYTES {
        return Ok(UploadMechanics::Single);
    }
    plan_shape(size_bytes)
        .map(UploadMechanics::Multipart)
        .map_err(|_| PushError::TooLarge {
            key: key.to_string(),
        })
}

/// The request URL: trailing slashes stripped from the base once, the
/// key verbatim, the query appended. Project and upload ids are owned by
/// grammar-checked constructors upstream of here.
pub fn object_url(cache_url: &str, key: &str, query: &str) -> String {
    format!("{}/{key}{query}", cache_url.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cap_rides_single_and_past_it_goes_multipart() {
        assert_eq!(
            plan_mechanics("nar/x.nar", UPLOAD_SINGLE_MAX_BYTES).expect("at cap"),
            UploadMechanics::Single,
        );
        match plan_mechanics("nar/x.nar", UPLOAD_SINGLE_MAX_BYTES + 1).expect("past cap") {
            UploadMechanics::Multipart(shape) => {
                assert_eq!(
                    shape.count,
                    (UPLOAD_SINGLE_MAX_BYTES + 1)
                        .div_ceil(cachet_core::constants::UPLOAD_PART_BYTES),
                );
                assert!(shape.last_len > 0);
            }
            UploadMechanics::Single => panic!("past the cap must not ride single"),
        }
    }

    #[test]
    fn urls_strip_the_base_once() {
        assert_eq!(
            object_url("https://cache.test/", "nar/x.nar.zst", ""),
            "https://cache.test/nar/x.nar.zst"
        );
        assert_eq!(
            object_url("https://cache.test", "aaa.narinfo", "?uploads"),
            "https://cache.test/aaa.narinfo?uploads"
        );
    }
}
