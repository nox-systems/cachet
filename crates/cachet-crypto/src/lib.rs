//! cachet-crypto is the only place hashing, signing, and decompression
//! happen (CLAUDE.md §5): SHA-256 over streams, nix base32 and base64 hash
//! encodings, ed25519 signing of the nix narinfo fingerprint, zstd
//! decoding, and RS256/JWKS verification for GitHub OIDC. Every function
//! is pure and every dependency builds for wasm32-unknown-unknown;
//! deny.toml bans ring, aws-lc, and the C zstd crate from the whole tree.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod base32;
pub mod ed25519;
pub mod rs256;
pub mod sha256;
pub mod zstd_stream;
