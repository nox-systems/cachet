//! cachet-core holds every protocol decision as a total function over
//! injected values: narinfo grammar, key validation, lease and generation
//! documents, multipart part planning, GC mark and sweep, OIDC claim
//! policy, and the error-code table (CLAUDE.md §1, §4). It performs no I/O
//! and reads no ambient clock or entropy; the Clock and Rng traits are
//! defined here and implemented at the edges.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod auth;
pub mod constants;
pub mod error;
pub mod gc;
pub mod generation;
pub mod keys;
pub mod lease;
pub mod multipart;
pub mod nar_facts;
pub mod narinfo;
pub mod oauth;
pub mod problem;
pub mod read;
pub mod read_token;
pub mod roots;
pub mod roots_payload;
pub mod stats;
pub mod stats_query;
pub mod types;
pub mod upload_record;
pub mod write;
