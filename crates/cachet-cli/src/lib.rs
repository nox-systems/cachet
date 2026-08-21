//! cachet-cli is the user-facing client (CLAUDE.md §4): device-flow login
//! against github.com, managed-block edits of nix.conf and netrc, the CI
//! push driver over cachet-push, keypair generation, and the
//! 404-versus-401 doctor probe.

#![forbid(unsafe_code)]

pub mod config;
pub mod doctor;
pub mod keygen;
pub mod login;
pub mod push;
pub mod setup;

/// The one user-facing failure shape: a sentence, no machinery. Every
/// command's failure path renders it with the `cachet:` prefix, matching
/// the log vocabulary the pipeline already speaks.
#[derive(Debug)]
pub struct CliError(pub String);

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CliError {}

/// Build the shared TLS client once per command.
///
/// # Errors
///
/// [`CliError`] when the TLS stack cannot initialize.
pub fn http_client() -> Result<reqwest::Client, CliError> {
    reqwest::Client::builder()
        .user_agent(concat!("cachet/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|failure| CliError(format!("could not build the HTTP client: {failure}")))
}
