# CLAUDE.md: the cachet constitution

cachet is a self-hostable nix binary cache on Cloudflare Workers, written
in Rust with workers-rs. This document binds every contribution and every
agent that works in this repository. Every rule carries its reason.

## §0 The prime directive

A gate, lane, or doc name exists in conversation only when it exists in the
flake, the justfile, or the manifest. Suggesting a check that cannot run is
a defect. When you add one, land the verb, the CI job, and the doc row in
one commit.

## §1 The stack

The worker runs on workers-rs compiled to wasm32-unknown-unknown, with R2
as the only cache-data store and KV for auth verdicts and sessions only.
Deployments are provisioned by an alchemy-run program under `infra/`; the
`cachet` CLI ships natively through cargo-dist; the GitHub Action lives in
this repo under `action/`. The three auth flows are GitHub OIDC for
writes, GitHub device flow for laptop reads, and browser OAuth with KV
sessions for the future SPA. Signing is server-side only, and a narinfo is
signed only after its stored NAR verifies byte-for-byte.

## §2 The prose law

PROSE.md governs every Markdown file, comment, and user-visible string in
this repository, including its own README. The doc-manifest gate enforces
that the manifest in §11 loads all of it.

## §3 Clock, entropy, and layout determinism

Time comes from a Clock seam sampled once at request start; entropy comes
from an Rng seam wired only at the worker edge; map layout uses BTreeMap
and BTreeSet. clippy.toml bans the ambient forms and the ban is
mechanical, not advisory. Banned verbatim: SystemTime::now, Instant::now,
thread::spawn, HashMap, HashSet.

## §4 Crate boundaries

The dependency direction is one-way: cachet-core is pure; cachet-crypto
computes; cachet-api describes the HTTP surface and generates the OpenAPI
document; cachet-worker is the wasm32 deployable that owns every binding;
cachet-push and cachet-cli are native writer tools. cachet-worker fails
host builds and is excluded from workspace default-members; its build and
its truth lane are the wasm verbs. A path dependency against this
direction fails review.

## §5 Wasm and crypto hygiene

ring, aws-lc-rs, aws-lc-sys, and the C zstd crate do not build for
wasm32-unknown-unknown and are banned in deny.toml. Hashing uses sha2,
signing uses ed25519-dalek, decompression uses ruzstd, and RS256 uses the
crypto decided in docs/adr. scripts/check-wasm-hygiene.sh scans the shipped
bundle for the banned runtimes and for secret-shaped strings; the gate
protects the artifact, not intentions.

## §6 Dependencies

Versions pin exactly where generation behavior matters and by minor series
elsewhere. New dependencies need one `// why:` comment at their
Cargo.toml entry: what they do and why nothing in the tree already does it.

## §7 Errors and panics

External-input failures are typed errors with the error-code table's
vocabulary; RFC 9457 problem+json bodies leave the worker. Invariant
violations panic with named invariants, and an attacker-reachable
condition never panics. Test code panics on missing fixtures rather than
skipping.

## §8 The manifest gates

Two bijections keep docs and runs honest, both enforced by scripts in
`scripts/`: the §11 manifest against the repo's non-ADR Markdown
(check-doc-manifest.sh), and docs/testing/lanes.toml against the lane docs
and CI jobs (check-lane-manifest.sh).

## §9 The lanes

Every test belongs to exactly one lane, and each lane is job-named the
same everywhere: the lanes.toml row, the docs/testing doc, the just verb,
and the CI job. A failed test that passes on retry is a bug, never a flake:
retries stay zero. The registry docs/testing/lanes.toml is the list.

## §10 Comments

Code comments state why, not what: `// why:` begins every non-obvious
choice so the reason greps. Rustdoc states what a public item does; the
comment beside it states why the code chose this way.

## §11 Required context

The manifest below is the bijection's source. Scripts assert it loads
exactly the repo's non-ADR Markdown, no more and no less.

@README.md
@PROSE.md
@SECURITY.md
@docs/testing/unit.md
@docs/testing/property.md
@docs/testing/golden.md
@docs/testing/kani.md
@docs/testing/workerd.md
