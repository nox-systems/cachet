# ADR 0009: alchemy provisions, stages isolate

- **Status:** Accepted
- **Date:** 2026-08-21
- **Context doc:** [../DEPLOY.md](../DEPLOY.md)

## Context

A self-hosted deployment needs a repeatable answer to "make the
infrastructure exist": the bucket, the namespace, the worker, its
bindings, the cron, the domain. The previous deployment was wrangler
commands run by hand plus console clicks for bindings, which is why a
second deployment was a half-day of reading the first one's dashboard.
This repo's deployments must be reproducible by construct: orgs run
their own, and we run staging and production.

## Decision

1. `infra/` holds one alchemy stack program (v2, effect-native) pinned
   to an exact alchemy version with a committed lockfile. The stack
   declares the R2 bucket, the KV namespace, the worker and its
   bindings, the cron trigger, and the custom domain, and
   `just deploy <stage>` runs it.
2. The worker artifact uploads byte-for-byte (`bundle: false`): the
   deployable is exactly what worker-build emitted, which is exactly
   what the lanes tested; no deploy-time bundler rewrites module
   shape.
3. Stages isolate: `cachet-staging-*` and `cachet-production-*` share
   no resources. Configuration that must differ per stage (domain
   attachment, GC grace) derives from the stage in the stack; secrets
   and orgs come from the stage's env.
4. Deploy state uses alchemy's Cloudflare store, so CI and laptops
   converge on one truth instead of per-machine local state files.
5. Deploy-time misconfiguration fails before the account: the stack
   validates the whole CACHET_DEPLOY_* set and names every missing
   value at once.

## Consequences

A deployment is one command with idempotent converge semantics;
destroying a stage is one command. The deploy path honors the same
pinning discipline as the code path: exact alchemy version, locked
install, validation up front. The dependency accepted is the alchemy
project itself, pinned and lockfiled, with wrangler kept in the dev shell
as the manual fallback (`wrangler deploy` against the same bundle) if
the stack ever cannot run.

## Alternatives considered

Raw wrangler per environment: reintroduces the console-click failure
mode for bindings, domains, and crons; rejected. Terraform: the CF
provider covers these resources, at the price of a second toolchain,
a state backend to operate, and no Effect-native resources for the
worker build; rejected for a repo already carrying nix and bun. A
re-bundling deploy tool (wrangler's build): rewrites the module graph
around the wasm, re-opening everything the bundle-hygiene gate just
proved; rejected at the bundler boundary instead of the tool level.
