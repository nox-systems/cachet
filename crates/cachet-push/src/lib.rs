//! cachet-push is the writer-side pipeline as a native library (CLAUDE.md
//! §1): snapshot the local store, diff against a previous snapshot, filter
//! against the cache and the upstream substituter, stage through nix with
//! zstd compression, upload NARs before narinfos with a fresh OIDC token
//! per request, and renew the project lease on the default branch. The CI
//! contract is to log and exit zero on failure rather than fail the job.
//!
//! Drawn in two layers: the decision layer (`snapshot`, `plan`, `retry`,
//! `filter`) is pure data with unit proofs; the execution layer (`oidc`,
//! `http`, `pipeline`) composes decisions with nix and the wire.

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

pub use error::PushError;

#[cfg(test)]
mod pipeflow;
