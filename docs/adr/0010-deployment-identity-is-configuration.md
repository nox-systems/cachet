# ADR 0010 — Deployment identity is configuration

- **Status:** Accepted
- **Date:** 2026-08-21
- **Context doc:** [../DEPLOY.md](../DEPLOY.md)

## Context

The previous deployment baked its identity into shared surfaces: the
hostname, the public key, and the org were literal defaults in the
login script and the action metadata, so every consumer inherited them
silently. That worked for exactly one deployment. As open-source
software that others deploy, the repo cannot carry any one
deployment's identity: a hostname or a key in a default would be a
claim on somebody else's deployment.

## Decision

1. The repo carries no deployment identity: no hostname, no org slug,
   no key material, no OAuth client id in committed code or metadata
   defaults. Every such value arrives as configuration: worker vars,
   the `CACHET_DEPLOY_*` env contract, action inputs, CLI flags.
2. The deployment's host name is the signing-key name's prefix
   (`<host>-1`), so key identity follows deployment identity through
   one value. The public half is fetched from the deployment's own
   public config document by every consumer that needs it (the laptop
   `setup`, the action's install step, `doctor`) and is never pasted.
3. Test fixtures use non-production identities throughout
   (`cachet.lane.invalid`, `lane-org`), which makes an accidental
   committed identity visible in review instead of invisible by being
   the default.

## Consequences

Forks and org deployments are first-class: a fresh clone produces a
working deployment of somebody else's identity with zero source edits.
No secret-shape strings exist to leak through fixtures, which the
wasm-hygiene scan verifies on every green run. Where an input default
must exist (the action's audience, the branch ref), the default is a
protocol constant, not an identity fact.

## Alternatives considered

Compile-time identity through generated config per consumer repo: a
generated step that must be re-run per clone, and a harder fork story;
rejected. A public "reference deployment" baked as defaults for
convenience, overridable by input: the exact failure the rule guards
against, with extra steps; rejected.
