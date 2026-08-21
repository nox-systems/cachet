//! The RFC 9457 problem document (CLAUDE.md §7). Every client-reachable
//! failure leaves the worker as one of these bodies; the shape is locked
//! by the golden lane so a change is a deliberate wire decision. The body
//! carries the machine `code` a client matches and a human `title`, and
//! nothing about the occurrence: specifics belong to worker logs, not to
//! bytes an attacker reads.

use crate::error::ClientError;

/// Render the problem document for a client error. The field order is the
/// contract: `type`, `status`, `title`, `code`, one line, trailing
/// newline.
#[must_use]
pub fn problem_body(error: ClientError) -> String {
    format!(
        "{{\"type\":\"about:blank\",\"status\":{},\"title\":\"{}\",\"code\":\"{}\"}}\n",
        error.status(),
        error.title(),
        error.code(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_renders_a_body_with_its_status_and_code() {
        for error in [
            ClientError::MalformedKey,
            ClientError::MalformedNarinfo,
            ClientError::MalformedRoots,
            ClientError::MalformedAuth,
            ClientError::PartNumberInvalid,
            ClientError::PartSizeMismatch,
            ClientError::CompletePartsMismatch,
            ClientError::UnsupportedCompression,
            ClientError::StorePathMismatch,
            ClientError::FileHashMismatch,
            ClientError::NarHashMismatch,
            ClientError::Unauthorized,
            ClientError::ForbiddenOrg,
            ClientError::ForbiddenRef,
            ClientError::ForbiddenProject,
            ClientError::NotFound,
            ClientError::UploadUnknown,
            ClientError::NarinfoNarMissing,
            ClientError::LengthRequired,
            ClientError::BodyTooLarge,
            ClientError::AuthUnavailable,
            ClientError::StorageUnavailable,
            ClientError::MalformedOauth,
            ClientError::OauthStateUnknown,
            ClientError::ForbiddenAdmin,
        ] {
            let body = problem_body(error);
            let parsed: serde_json::Value =
                serde_json::from_str(&body).expect("the body is valid JSON");
            assert_eq!(parsed["status"], error.status());
            assert_eq!(parsed["code"], error.code());
            assert_eq!(parsed["title"], error.title());
            assert_eq!(parsed["type"], "about:blank");
        }
    }
}
