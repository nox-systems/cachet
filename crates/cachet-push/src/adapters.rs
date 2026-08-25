//! The seams the pipeline composes over: nix commands, staging, HTTP, and
//! the OIDC mint. Each trait is small and synchronous-shaped; the real
//! implementations live in `real`, the scripted ones in the tests, and the
//! pipeline never knows which.

use std::future::Future;
use std::path::{Path, PathBuf};

use crate::PushError;
use crate::stage::{NarBody, PathFacts, StagedNar};

/// What the pipeline needs from nix and the filesystem.
pub trait Commands: Send + Sync {
    /// `nix path-info --all`, raw stdout.
    fn path_info_all(&self) -> impl Future<Output = Result<String, PushError>> + Send;
    /// `nix path-info <installable>`, raw stdout.
    fn path_info(
        &self,
        installable: &str,
    ) -> impl Future<Output = Result<String, PushError>> + Send;
    /// `nix path-info --json <paths...>`: the facts every narinfo needs,
    /// for many paths in one invocation. Paths nix does not answer for are
    /// absent from the result rather than an error, because a path that
    /// left the store between the diff and this call is nothing to push.
    fn path_facts(
        &self,
        paths: &[String],
    ) -> impl Future<Output = Result<Vec<PathFacts>, PushError>> + Send;
    /// Serialize one path's NAR, compress it, and measure it. The bytes
    /// never touch a shared staging tree, so nothing but this path is
    /// compressed and nothing has to be filtered out afterwards.
    fn stage_nar(
        &self,
        facts: &PathFacts,
    ) -> impl Future<Output = Result<StagedNar, PushError>> + Send;
}

/// A request body the wire can send more than once.
///
/// A retry has to send the same bytes again, and the previous pipeline
/// did that by cloning the whole body per attempt: up to ninety mebibytes
/// memcopied for every try. Both shapes here are cheap to clone. Bytes
/// share one allocation behind a refcount, and a file range is a path and
/// two integers that the adapter re-reads when it sends.
#[derive(Debug, Clone)]
pub enum UploadBody {
    /// Bytes already in memory, shared rather than copied.
    Bytes(std::sync::Arc<[u8]>),
    /// A range of a scratch file, read as it is sent.
    FileRange {
        /// The file to read.
        path: PathBuf,
        /// Where the range starts.
        offset: u64,
        /// How long the range runs.
        len: u64,
    },
}

impl UploadBody {
    /// How many bytes the body sends.
    pub fn len(&self) -> u64 {
        match self {
            Self::Bytes(bytes) => bytes.len() as u64,
            Self::FileRange { len, .. } => *len,
        }
    }

    /// Whether the body sends nothing.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// A sub-range of this body, for one multipart part. A range past the
    /// end clamps, so a caller can never name bytes the body lacks.
    #[must_use]
    pub fn slice(&self, offset: u64, len: u64) -> Self {
        match self {
            Self::Bytes(bytes) => {
                let start = usize::try_from(offset)
                    .unwrap_or(bytes.len())
                    .min(bytes.len());
                let end = usize::try_from(offset.saturating_add(len))
                    .unwrap_or(bytes.len())
                    .min(bytes.len());
                Self::Bytes(std::sync::Arc::from(&bytes[start..end]))
            }
            Self::FileRange {
                path,
                offset: base,
                len: whole,
            } => {
                let start = base.saturating_add(offset);
                let remaining = whole.saturating_sub(offset);
                Self::FileRange {
                    path: path.clone(),
                    offset: start,
                    len: len.min(remaining),
                }
            }
        }
    }
}

impl From<NarBody> for UploadBody {
    fn from(body: NarBody) -> Self {
        let len = body.len();
        match body {
            NarBody::Bytes(bytes) => Self::Bytes(bytes),
            NarBody::File(path) => Self::FileRange {
                path: path.to_path_buf(),
                offset: 0,
                len,
            },
        }
    }
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
        body: UploadBody,
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
    /// Drop any run-scoped credential the caller holds: a 401 from the
    /// API means the token it refused must not be issued again this run.
    /// Sources without a memo keep the no-op default.
    fn invalidate(&self, _audience: &str) -> impl Future<Output = ()> + Send {
        async {}
    }
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
