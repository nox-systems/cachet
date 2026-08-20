//! `/api/public/config`: the one unauthenticated JSON document. The CLI
//! and the later SPA discover the deployment's OAuth client id, orgs,
//! host, and public signing key here, so nothing the client needs ever
//! rides in a secret channel it does not yet have.

use cachet_core::error::ClientError;
use cachet_crypto::ed25519::NixSecretKey;
use worker::{Env, Response};

use crate::{auth, log};

/// Serve the public configuration. The signing key's public half is
/// computed from the secret binding: the deployment answers from its own
/// custody rather than from a second copy that could drift.
pub fn public_config(env: &Env) -> worker::Result<Response> {
    let config = match auth::oidc_config(env) {
        Ok(config) => config,
        Err(code) => return crate::error::problem_response(code),
    };
    let client_id = match env.var("CACHET_OAUTH_CLIENT_ID") {
        Ok(client_id) => client_id.to_string(),
        Err(_) => return crate::error::problem_response(ClientError::AuthUnavailable),
    };
    let host = match env.var("CACHET_HOST") {
        Ok(host) => host.to_string(),
        Err(_) => return crate::error::problem_response(ClientError::AuthUnavailable),
    };
    let public_key = match env.secret("CACHET_SIGNING_KEY") {
        Ok(secret) => match NixSecretKey::parse(&secret.to_string()) {
            Ok(key) => key.public_key_text(),
            Err(failure) => {
                crate::log::event(
                    "error",
                    "api.signing_key_corrupt",
                    &[("error", format!("{failure:?}"))],
                );
                return crate::error::problem_response(ClientError::AuthUnavailable);
            }
        },
        Err(_) => return crate::error::problem_response(ClientError::AuthUnavailable),
    };
    log::event("info", "api.public_config", &[]);
    // The body is cachet-api's shared type: the served wire and the
    // published OpenAPI schema cannot drift apart here.
    let body = serde_json::to_string(&cachet_api::PublicConfig {
        oauth_client_id: client_id,
        orgs: config.orgs,
        host,
        public_key,
    })
    .expect("the config fields serialize");
    let headers = worker::Headers::new();
    headers.set("content-type", "application/json")?;
    headers.set("cache-control", "no-store")?;
    Ok(Response::ok(body)?.with_headers(headers))
}

/// `/api/openapi.json`: the committed generated document, served verbatim
/// so the drift gate and the served bytes check the same artifact.
pub fn openapi_document() -> worker::Result<Response> {
    let headers = worker::Headers::new();
    headers.set("content-type", "application/yaml")?;
    headers.set("cache-control", "public, max-age=300")?;
    Ok(Response::ok(OPENAPI_YAML)?.with_headers(headers))
}

// why: serving reads the same committed file the drift gate regenerates,
// so a spec change that forgets to regenerate fails CI, not clients.
const OPENAPI_YAML: &str = include_str!("../../../docs/openapi.yaml");
