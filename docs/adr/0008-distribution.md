# ADR 0008: cargo-dist releases; the in-repo action wraps the released binary

- **Status:** Accepted
- **Date:** 2026-08-21
- **Context doc:** [../../action/action.yml](../../action/action.yml)

## Context

The CLI and the action serve the same write path from two positions: a
person bootstraps a deployment or a laptop, and CI pushes from a
runner. Three properties matter for both: the bits a user runs are the
bits this repo's gates ran; the consumer can pin them; and the user
never needs a Rust toolchain. The previous deployment shipped the
action as a checked-in esbuild bundle with a private source, where
pinning worked and reviewability did not.

## Decision

1. cargo-dist, driven by tags matching the workspace version, builds
   the CLI for the four runner-and-laptop triples
   (x86_64/aarch64, linux/macos), packages each as a tar.xz with the
   LICENSE and README, and attaches sha256 checksums to the GitHub
   Release. A shell installer script ships alongside for laptops.
2. The action lives in this repo as a composite plus a nested node24
   main/post pair (composite actions cannot declare post steps). The
   composite downloads the pinned release archive, verifies its sha256
   before the archive opens, and the nested pair execs the binary for
   the snapshot and push phases.
3. The binary version the action runs is the `cachet-version` input
   with a metadata default; bumping the default is part of tagging a
   release. Consumers pin the moving major line tag: `@v0` while the
   releases are 0.x (the v0 line re-points at each v0 release commit),
   and a `v1` line opens with the first 1.0 tag.
4. The wasm bundle of the worker is NOT distributed this way: deploys
   build it from source through `just deploy` (ADR 0009), because a
   deployment's config and code travel together.

## Consequences

Every consumer's binary is bit-identical to what CI produced, and the
action's fetch is checksum-gated so a tampered or truncated download
is refused before anything executes. Reviewers audit the action as
plain text in the repo (the post step is two twenty-line wrappers),
and the behavior lives in the tested CLI. Version pinning is one
input.

## Alternatives considered

A checked-in bundled JS post step (the old design): review-hostile and
forks the pipeline into a second language; rejected. Requiring users to
`cargo install --path .`: matches none of the target populations
(operators on laptops, ephemeral runners); rejected. Downloading a
moving latest release without verification: silent supply-chain
exposure on every CI run; rejected.
