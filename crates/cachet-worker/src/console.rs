//! Serving the browser console.
//!
//! The console is static files, and the asset layer serves the ones that
//! exist without a worker in the path. What the layer must never do is
//! invent an answer for a request that matches no file, because the
//! deployment's other job is answering nix, and nix reads a cache by
//! asking about paths that mostly do not exist. A cache miss is a 404,
//! and a layer configured to answer an unmatched request with an
//! application shell would answer `200 text/html` to every substituter
//! that asked. So both of the layer's handling modes are off: it answers
//! files and nothing else, and everything it does not answer falls
//! through to the router, which decides it the way it always has.
//!
//! What is left for this module is the console's own routes. A person
//! following a link to `/console/traffic` is asking for a route inside
//! the console's router rather than a file, and every one of those
//! renders the same shell, which this module fetches from the layer by
//! name (ADR 0014).

use worker::{Env, Request, Response, Result};

use cachet_core::error::ClientError;

/// The console's prefix. Everything it serves lives under this, and the
/// deployment's root redirects here.
pub(crate) const CONSOLE_PREFIX: &str = "/console";

/// The binding the asset layer answers on.
const ASSETS_BINDING: &str = "ASSETS";

/// The document every console route renders from.
const CONSOLE_SHELL: &str = "/console/index.html";

/// Whether this path belongs to the console.
///
/// Exact prefix matching, so `/consoles-of-the-world.narinfo` is a
/// narinfo and not a console route.
pub(crate) fn owns(path: &str) -> bool {
    path == CONSOLE_PREFIX || path.starts_with("/console/")
}

/// Serve one console request from the asset layer.
///
/// A path whose last segment carries an extension is a built file, which
/// the asset layer normally answers before the worker is invoked at all;
/// this handles the ones that reach here anyway, so the two paths agree.
/// Anything else is a route inside the console's own router, and every
/// one of those renders the same shell. Deciding by extension rather
/// than by a list of known routes means adding a screen is a change to
/// the console alone.
///
/// A deployment whose worker predates the binding answers 404 here,
/// exactly as it did before the console existed. Degrading is the same
/// choice the counter binding makes: an upgrade that has not been
/// redeployed serves the protocol perfectly and simply has no console.
pub(crate) async fn serve(env: &Env, req: &Request, path: &str) -> Result<Response> {
    let Ok(assets) = env.assets(ASSETS_BINDING) else {
        crate::log::event("info", "console.unbound", &[]);
        return crate::error::problem_response(ClientError::NotFound);
    };
    let mut url = req.url()?;
    url.set_path(if names_a_file(path) {
        path
    } else {
        CONSOLE_SHELL
    });
    url.set_query(None);
    assets.fetch(url.to_string(), None).await
}

/// The deployment's root, which is the console's front door.
///
/// nix never asks for `/`: it asks for `/nix-cache-info` and for paths.
/// So the root is free, and a person who types the deployment's host
/// into a browser should land somewhere rather than read a 404.
pub(crate) fn redirect_to_console() -> Result<Response> {
    let headers = worker::Headers::new();
    headers.set("location", CONSOLE_PREFIX)?;
    Ok(Response::empty()?.with_status(302).with_headers(headers))
}

/// Whether the last path segment names a file rather than a route.
fn names_a_file(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .is_some_and(|segment| segment.contains('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_console_owns_its_prefix_and_nothing_adjacent() {
        assert!(owns("/console"));
        assert!(owns("/console/"));
        assert!(owns("/console/traffic"));
        assert!(owns("/console/assets/main-a4f31c.js"));
        // The nix key space starts at the root, so a prefix match that
        // was not exact would take narinfos out of the protocol's hands.
        assert!(!owns("/consoles"));
        assert!(!owns("/console.narinfo"));
        assert!(!owns("/nar/console/x"));
        assert!(!owns("/"));
        assert!(!owns("/nix-cache-info"));
        assert!(!owns("/api/self/events"));
    }

    #[test]
    fn a_route_renders_the_shell_and_a_file_is_asked_for_by_name() {
        for route in ["/console", "/console/", "/console/traffic", "/console/gc"] {
            assert!(!names_a_file(route), "{route}");
        }
        for file in [
            "/console/index.html",
            "/console/assets/main-a4f31c.js",
            "/console/assets/main-a4f31c.css",
            "/console/favicon.ico",
        ] {
            assert!(names_a_file(file), "{file}");
        }
    }
}
