# ADR 0001: Server-side signing, verify-then-sign

- **Status:** Accepted
- **Date:** 2026-08-21
- **Context doc:** [../../CLAUDE.md](../../CLAUDE.md) §1, §7

## Context

Nix's trust model for binary caches is not transport trust. A narinfo
carries a `Sig` field: a detached ed25519 signature, labelled with a key
name, over a fingerprint composed from the store path, the NAR hash, the
NAR size, and the sorted references. A substituting client recomputes
the fingerprint and verifies the signature against its configured
trusted public keys; a failure means build-from-source, quietly. TLS and
authentication decide who may fetch, not what may be believed.

Someone who can place narinfos that clients trust can execute code on
every machine that substitutes from the cache. The previous cachet
deployment signed client-side: CI runners held the org's signing key as
a GitHub secret, and every job that could read the secret could mint
trust. Runners are shared, caches are sticky, and the key's disclosure
radius was every workflow in every repo in the org.

## Decision

1. The server holds the only signing key, as a Workers secret binding
   provisioned at deploy time. Clients upload unsigned narinfos staging
   through `nix copy` to a file:// destination without a secret key.
2. The worker signs a narinfo only after verifying its bytes: the
   store-path grammar, the NAR's presence in the bucket, the declared
   `NarHash` and `NarSize` recomputed against the stored object. The
   verified value is a distinct type the signing path requires, so an
   unverified narinfo cannot reach the signature by construction.
3. The narinfo's key name is the deployment's host with a numeric
   suffix (`<host>-1`), which makes rotation a redeploy and clients
   learn the public half from the deployment's public config document.

## Consequences

The trust radius of the signing key is the worker's secret binding and
the operator's deploy environment. A stolen OIDC token can upload
paths, but the uploaded narinfo must pass byte verification against the
NAR it names before it is signed; forgery requires defeating the verification itself; holding a credential does not suffice. The public config document is
the single source for the key clients trust, so rotation is one redeploy with an incremented suffix instead of an org-wide key redistribution. The cost is strictness: a wrong hash or size in an uploaded
narinfo is a hard 400, and pushes must stage exact bytes, which the
push pipeline does by hashing nothing itself and letting the server
check.

## Alternatives considered

Client-side signing (the previous deployment) keeps the worker simple
and puts the verification burden on consumers, at the price of the key
material described above; rejected. Signing an unverified document
server-side is strictly worse than either, since it turns a
configuration mistake into trusted poison; rejected.
