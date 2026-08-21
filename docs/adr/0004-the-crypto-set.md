# ADR 0004 — The crypto set: sha2, ed25519-dalek, ruzstd, WebCrypto RS256

- **Status:** Accepted
- **Date:** 2026-08-21
- **Context doc:** [../../CLAUDE.md](../../CLAUDE.md) §5; [../../deny.toml](../../deny.toml)

## Context

The worker compiles to wasm32-unknown-unknown, a target with no native
crypto libraries and no OpenSSL. The common Rust accelerator crates
(aws-lc-rs, ring's build, C zstd) embed C or assembly and do not build
for it. Meanwhile the native crates (push, CLI) need TLS on hosts, and
rustls's remaining provider choices are ring and aws-lc. One blanket
rule cannot serve both constraints, so the rule needs teeth in each
direction.

## Decision

1. The worker's cryptography is the pure-Rust, wasm-provable set:
   `sha2` for hashing, `ed25519-dalek` for signing, `ruzstd` for
   decompression.
2. RS256 verification (GitHub OIDC tokens) uses the pure-Rust `rsa`
   crate with `sha2` digests, everywhere, worker included. The only RSA
   operation cachet performs is public-key verification of presented
   JWKs, so the `rsa` crate's private-key timing side channels are
   structurally unreachable; deny.toml records that reasoning per
   advisory.
3. The native crates bind TLS through reqwest's rustls feature, whose
   provider is `ring`. deny.toml bans `ring` workspace-wide except
   through the `rustls`/`rustls-webpki` wrappers, so a new dependency
   path to ring fails the gate; the wasm-hygiene scan of the shipped
   bundle is the independent backstop.
4. Entropy at the worker edge comes from the runtime's `getrandom` with
   the JS backend; host-side it is the OS. Secrets are zeroized where
   the type permits it, and the signing key exists only inside the
   secret-binding read that the signing path performs.

## Consequences

The supply-chain surface on the wasm side is auditable in one
read: four names, all pure Rust. deny.toml plus the bundle scan make
the wasm ban mechanical: both execute on every green run. The native exception
is written down exactly where it is enforced, so a later contributor
reads the rule and the reason in the same place.

## Alternatives considered

`rustls` with aws-lc as the native provider: aws-lc ships C code with
its own build graph for no TLS benefit on the targets we support;
rejected. A single builder for both targets with ring enabled both
ways: ring does not build for wasm32-unknown-unknown; impossible, not
rejected. Verifying RS256 through workerd's WebCrypto instead of the
`rsa` crate: one less big-integer dependency, at the price of an async
FFI verify call inside the signing path's carefully synchronous
verdict pipeline and a second crypto implementation to test against
the browsers' matrix; the workspace already carried `rsa` for hosts
and tests, so the worker uses the same code path; rejected.
