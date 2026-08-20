//! cachet-push is the writer-side pipeline as a native library (CLAUDE.md
//! §1): snapshot the local store, diff against a previous snapshot, filter
//! against the cache and the upstream substituter, stage through nix with
//! zstd compression, upload NARs before narinfos with a fresh OIDC token
//! per request, and renew the project lease on the default branch. The CI
//! contract is to log and exit zero on failure rather than fail the job.

#![forbid(unsafe_code)]
