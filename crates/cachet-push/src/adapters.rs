//! The seams the pipeline composes over: nix commands, file reads, HTTP,
//! and the OIDC mint. Each trait is small and synchronous-shaped; the
//! real implementations live in `real`, the scripted ones in the tests,
//! and the pipeline never knows which.

use std::future::Future;
use std::path::{Path, PathBuf};

use crate::PushError;

/// What the pipeline needs from nix and the filesystem.
pub trait Commands: Send + Sync {
    /// `nix path-info --all`, raw stdout.
    fn path_info_all(&self) -> impl Future<Output = Result<String, PushError>> + Send;
    /// `nix path-info <installable>`, raw stdout.
    fn path_info(
        &self,
        installable: &str,
    ) -> impl Future<Output = Result<String, PushError>> + Send;
    /// `nix copy --to <destination> <paths...>`.
    fn copy_to(
        &self,
        destination: &str,
        paths: &[String],
    ) -> impl Future<Output = Result<(), PushError>> + Send;
    /// One directory level of (name, size), for the staging layout.
    fn read_dir(
        &self,
        dir: &Path,
    ) -> impl Future<Output = Result<Vec<(String, u64)>, PushError>> + Send;
    /// A file's bytes, for single-PUT bodies.
    fn read_file(&self, path: &Path) -> impl Future<Output = Result<Vec<u8>, PushError>> + Send;
    /// A file's byte range, for multipart parts.
    fn read_range(
        &self,
        path: &Path,
        offset: u64,
        len: u64,
    ) -> impl Future<Output = Result<Vec<u8>, PushError>> + Send;
}

/// One HTTP answer: status and body. Headers arrive through the request
/// parameters rather than a type map, because the pipeline's shapes are
/// exact and few.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireAnswer {
    /// The status code.
    pub status: u16,
    /// The body bytes.
    pub body: Vec<u8>,
}

/// What the pipeline needs from the wire.
pub trait Http: Send + Sync {
    /// `HEAD <url>`, optionally Bearer-authorized.
    fn head(
        &self,
        url: &str,
        bearer: Option<&str>,
    ) -> impl Future<Output = Result<u16, PushError>> + Send;
    /// `GET <url>`, optionally Bearer-authorized.
    fn get(
        &self,
        url: &str,
        bearer: Option<&str>,
    ) -> impl Future<Output = Result<WireAnswer, PushError>> + Send;
    /// `PUT <url>` with an exact body and headers.
    fn put(
        &self,
        url: &str,
        bearer: &str,
        body: Vec<u8>,
        headers: &[(String, String)],
    ) -> impl Future<Output = Result<WireAnswer, PushError>> + Send;
    /// `POST <url>` with an exact body and headers.
    fn post(
        &self,
        url: &str,
        bearer: &str,
        body: Vec<u8>,
        headers: &[(String, String)],
    ) -> impl Future<Output = Result<WireAnswer, PushError>> + Send;
    /// `DELETE <url>`, Bearer-authorized.
    fn delete(
        &self,
        url: &str,
        bearer: &str,
    ) -> impl Future<Output = Result<u16, PushError>> + Send;
}

/// The OIDC mint, isolated the way the pipeline sees it: one call, one
/// fresh token (or the error line).
pub trait TokenSource: Send + Sync {
    /// Mint a fresh OIDC token for the audience.
    fn mint(&self, audience: &str) -> impl Future<Output = Result<String, PushError>> + Send;
}

/// Where the snapshot file lives between a run's steps.
pub fn snapshot_path(runner_temp: &Path) -> PathBuf {
    runner_temp.join("cachet-store-before.txt")
}

/// The four seams in one place: every pipeline function takes this
/// rather than re-declaring the same parameter train (CLAUDE.md §10
/// favors one explanatory type over four carried arguments).
pub struct Adapters<'a, C: Commands, H: Http, T: TokenSource> {
    /// nix and the filesystem.
    pub commands: &'a C,
    /// the wire.
    pub http: &'a H,
    /// the OIDC mint.
    pub tokens: &'a T,
    /// the sleep injection.
    pub sleep: &'a crate::pipeline::Sleeper,
}
