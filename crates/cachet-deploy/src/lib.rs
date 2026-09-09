//! The deployment grammar: every fact about a cachet deployment derived
//! from its name and a small environment map.
//!
//! # Why this is Rust
//!
//! The binding names are one list that lived in four places: the alchemy
//! program, `infra/src/config.ts`, `cachet-core/src/constants.rs`, and a
//! dozen literals across `cachet-worker`. A name added to one and missed
//! in another produces a worker that deploys and then answers
//! `auth_unavailable` at the first request that reads it. Here the roster
//! is a `const` table that `cachet-worker` itself imports, and a test in
//! this crate scans the worker's source to hold the two sets equal in both
//! directions, so the names have one home by construction.
//!
//! Two smaller reasons follow. The plan is a wire contract that a deploy
//! tool and a hosted deployer both read, and the golden lane locks wire
//! contracts. And one property only a typed language carries cleanly:
//! over arbitrary environment maps, no rendered plan or error ever
//! contains a value from the map, only a name. The map holds the signing
//! key.
//!
//! # The split
//!
//! Everything past [`roster`] sits behind the `plan` feature.
//! `cachet-worker` compiles to wasm and imports the roster alone, so a
//! dependency added for the plan never reaches the bundle.

pub mod roster;

#[cfg(feature = "plan")]
pub mod env;
#[cfg(feature = "plan")]
pub mod error;
#[cfg(feature = "plan")]
pub mod manifest;
#[cfg(feature = "plan")]
pub mod name;
#[cfg(feature = "plan")]
pub mod plan;
#[cfg(feature = "plan")]
pub mod spec;

#[cfg(feature = "plan")]
pub use error::DeployError;
#[cfg(feature = "plan")]
pub use manifest::Manifest;
#[cfg(feature = "plan")]
pub use name::Deployment;
#[cfg(feature = "plan")]
pub use plan::{DeployPlan, plan};
