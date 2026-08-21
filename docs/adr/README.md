# Architecture decision records

Each ADR records one decision: the context that forced it, the decision
itself, the consequences accepted, and the alternatives weighed. The
living docs (CLAUDE.md, README.md, docs/) carry what the system does and
are sufficient on their own to act; the ADRs carry why it does it that
way and are opened on demand.

Format: a `# ADR NNNN: <title>` heading, then a Status, Date, and
Context doc header, then the sections Context, Decision, Consequences,
and Alternatives considered, in that order. A new ADR takes the next
number. ADRs stay out of the CLAUDE.md import manifest: they hold the
why, and the why is read on demand.

| ADR | Title | What it settles |
| --- | --- | --- |
| [0001](0001-server-side-signing.md) | Server-side signing, verify-then-sign | Whoever can place a narinfo that clients trust can execute code on every machine that substitutes; with server-side signing, a stolen cache credential no longer gets there, because every stored narinfo is signed only after its NAR verifies byte-for-byte |
| [0002](0002-three-authentication-flows.md) | OIDC writes, device-flow laptop reads, browser OAuth | Only CI writes; laptops and browsers read. No shared tokens: every credential is personal, revocable, and checked against org membership on a bounded cache |
| [0003](0003-kv-ceiling.md) | KV holds verdicts, sessions, and OAuth state only | R2 is the only cache-data store; KV is the only place auth answers may wait, each with a TTL that defines its revocation window |
| [0004](0004-the-crypto-set.md) | sha2, ed25519-dalek, ruzstd, rsa | The worker's cryptography is what wasm32 compiles and proves; native crates get TLS through rustls, which alone may carry ring |
| [0005](0005-gc-on-the-cron.md) | The collector is armed from day one | A cache that never deletes is a disk bill and a staleness hazard; the cron driver is cursor-resumable, gate-aborts, and reports every run |
| [0006](0006-multipart-protocol.md) | The upload protocol's fixed constants | 94,371,800-byte single cap, 64 MiB parts, 1000 parts, one declared total: the upload shape is a contract clients and the worker both enforce |
| [0007](0007-openapi-from-code.md) | The OpenAPI document is generated from code | The served spec is the route descriptors, regenerated and drift-gated, never hand-authored |
| [0008](0008-distribution.md) | cargo-dist releases; the in-repo action wraps the released binary | Users get a verified binary per platform from a tag; the action downloads exactly that artifact with its checksum |
| [0009](0009-alchemy-deployments.md) | alchemy provisions, stages isolate | Deploys converge via one stack program; stages share no resources; the worker bundle uploads byte-for-byte |
| [0010](0010-deployment-identity-is-configuration.md) | Deployment identity is configuration | An open-source repo carries no hostname, no org slug, no key bytes: every deployment value arrives as a var or a secret |
| [0011](0011-deployment-names.md) | Deployments are named, and the name is housekeeping | One name drives the env file, the alchemy stage, and the resource prefix; the host stays the only protocol identity |
