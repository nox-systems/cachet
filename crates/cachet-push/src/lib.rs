//! cachet-push is the writer-side pipeline as a native library (CLAUDE.md
//! §1): snapshot the local store, diff against a previous snapshot, ask
//! the cache in one probe what it already holds, serialize and compress
//! each surviving path on its own, upload NARs before narinfos with a
//! fresh OIDC token per request, and renew the project lease on the
//! default branch. The CI contract is to log and exit zero on failure
//! rather than fail the job.
//!
//! Drawn in two layers: the decision layer (`snapshot`, `plan`, `retry`,
//! `filter`, `stage`) is pure data with unit proofs; the execution layer
//! (`oidc`, `real`, `pipeline`) composes decisions with nix and the wire.

#![forbid(unsafe_code)]

pub mod adapters;
pub mod error;
pub mod filter;
pub mod oidc;
pub mod pipeline;
pub mod plan;
pub mod real;
pub mod retry;
pub mod snapshot;
pub mod stage;

pub use error::PushError;

#[cfg(test)]
mod pipeflow;
