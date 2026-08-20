//! The error boundary (CLAUDE.md §7): a typed [`ClientError`] leaves the
//! worker as an RFC 9457 problem+json body with the error table's status.
//! The body comes from cachet-core byte-for-byte; this module only frames
//! it as a response. Nothing internal (binding names, backend failures'
//! causes, request bytes) enters the body.

use cachet_core::constants::PROBLEM_CONTENT_TYPE;
use cachet_core::error::ClientError;
use cachet_core::problem::problem_body;
use worker::{Headers, Response, Result};

/// Render a client error as a problem+json response. The failure of this
/// rendering itself bubbles as the worker's generic 500: a failure to
/// frame an error is not a client error, typed or otherwise.
pub fn problem_response(error: ClientError) -> Result<Response> {
    let headers = Headers::new();
    headers.set("content-type", PROBLEM_CONTENT_TYPE)?;
    Ok(Response::ok(problem_body(error))?
        .with_status(error.status())
        .with_headers(headers))
}
