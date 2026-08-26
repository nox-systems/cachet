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
    render(error, false)
}

/// Render a client error for a path nix reads objects from.
///
/// The difference is one header. A 401 here carries the Basic challenge,
/// because nix fetches through curl with CURLAUTH_ANY, which waits for
/// the server to name a scheme before it sends netrc credentials: curl
/// sends Basic ahead of the challenge in practice, so this is not the
/// cache's current cost, but a client following the negotiation
/// literally would pay an extra round trip on every narinfo and every
/// NAR, and the header is what the protocol says a 401 carries.
///
/// # Errors
///
/// Propagates a header failure as the worker's generic 500.
pub fn object_read_problem(error: ClientError) -> Result<Response> {
    render(error, true)
}

/// The shared body, and the one header that differs.
///
/// why: the challenge is confined to the object paths. A browser that
/// receives a 401 carrying `WWW-Authenticate: Basic` opens its own
/// credential dialog, even for a `fetch` a page made, so emitting it from
/// the JSON API meant the console asked for a username and password
/// before it could render the screen that offers to sign you in with
/// GitHub. Nix reads objects and never reads the API; the console reads
/// the API and never reads objects. The header belongs to one of them.
fn render(error: ClientError, challenge: bool) -> Result<Response> {
    let headers = Headers::new();
    headers.set("content-type", PROBLEM_CONTENT_TYPE)?;
    if challenge && error.status() == 401 {
        headers.set(
            "www-authenticate",
            r#"Basic realm="cachet", charset="UTF-8""#,
        )?;
    }
    Ok(Response::ok(problem_body(error))?
        .with_status(error.status())
        .with_headers(headers))
}
