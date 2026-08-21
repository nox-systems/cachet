# ADR 0011: Deployments are named, and the name is housekeeping

- **Status:** Accepted
- **Date:** 2026-08-21
- **Context doc:** [../DEPLOY.md](../DEPLOY.md); supersedes the stage
  grammar of [ADR 0009](0009-alchemy-deployments.md) (its isolation and
  idempotence decisions stand)

## Context

cachet is self-hosted: every operator runs their own deployments, and an
operator may run several in one account (a staging rehearsal, multiple
production caches for different orgs). ADR 0009 framed stages as the
fixed pair `staging`/`production` and special-cased production:
only it defaulted its domain to the host, every other stage refused
without `CACHET_DEPLOY_DOMAIN`, and GC grace defaulted by stage name.
When deployments became a user-facing concept, the fixed pair could not
express "four production deployments in this account", and the
name-keyed special cases pushed deploy behavior into string comparisons.

## Decision

1. A deployment has an operator-chosen name matching
   `^[a-z][a-z0-9-]{1,31}$`, asked for first at `just bootstrap`. The
   name drives the env file (`infra/.env.<name>`), the alchemy stage,
   and the resource names (`cachet-<name>`; the prefix is a guarantee,
   not a ritual — a name already starting with `cachet-` is used as-is,
   so no stack ever reads `cachet-cachet-*`). `infra/src/config.ts`
   enforces the grammar on every deploy, so hand-written env files obey
   it too.
2. The name is housekeeping. The protocol identity stays the host: the
   signing key is `<host>-1`, narinfo signatures name hosts, and
   renaming a deployment never re-signs anything.
3. The custom domain defaults to the host for every deployment;
   `CACHET_DEPLOY_DOMAIN` is a pure override. The production-only
   default and the no-domain refusal are deleted.
4. GC grace defaults to the worker's fourteen days for every deployment;
   throwaway deployments opt into zero explicitly through
   `CACHET_DEPLOY_GC_GRACE_MS`. This repository's own staging pins the
   zero in its deploy workflow, where the reason is readable.
5. One name converges one stack per account: two operators sharing an
   account who both deploy `production` run one shared stack. DEPLOY.md
   carries the caveat.

## Consequences

`just deploy <name>` and `just destroy <name>` need no other changes:
recipes were already parameterized, and bootstrap now owns naming.
Account-side state (ADR 0009's decision) is keyed by stage, so each
name keeps its own state rows and upgrades or destroys one deployment
without touching another. Bootstrap's rerun path doubles as recovery:
the non-secret values regenerate from the live deployment's public
config, and only the two secrets need a second copy kept (password
manager or the CI environment), with rotation as the floor.

## Alternatives considered

Keep the fixed `staging`/`production` pair and let extra deployments be
forks: multiplies review surface for a naming problem; rejected. Make
the deployment name part of the signing identity (`<name>-<host>-1`):
renaming a deployment would rotate client trust for zero protocol
benefit, since signatures answer "which cache signed this", not "which
stack"; rejected. Require `CACHET_DEPLOY_DOMAIN` on every non-production
name (the old rule, generalized): one more first-deploy failure mode
that the host already answers; rejected.
