# ADR 0020: The deployment grammar is Rust, and the manifest is the contract

- **Status:** Accepted
- **Date:** 2026-09-09
- **Context doc:** [../DEPLOY.md](../DEPLOY.md); builds on
  [ADR 0010](0010-deployment-identity-is-configuration.md) and
  [ADR 0011](0011-deployment-names.md); prepares the retirement of
  [ADR 0009](0009-alchemy-deployments.md)

## Context

The binding names a cachet deployment carries lived in four places: the
alchemy program under `infra/`, `infra/src/config.ts`,
`cachet-core/src/constants.rs`, and a dozen string literals across
`cachet-worker`. Nothing held them to each other. A name added to the
worker and missed in the deploy program produced a worker that deployed
cleanly and then answered `auth_unavailable` at the first request that
read the missing var, which is the worst place to find out.

A second pressure arrived with the decision to build a hosted deployer
that installs cachet into customers' own Cloudflare accounts. That
service needs to know what a deployment is: which resources to create,
which bindings to render, which values to ask an operator for and which
to derive. Writing that knowledge into the service would have made a
fifth home for the list, and the fifth would have been the one that
decided what customers run.

Sesame faced the same two pressures earlier and answered with
`crates/sesame-deploy`: a Rust crate whose roster the worker imports, a
plan that is secret-free by construction, and golden and property lanes
over both. Its header comment argues the case, and cachet adopts the
shape rather than re-deriving it.

## Decision

1. `crates/cachet-deploy` is the deployment grammar. Its roster is a
   `const` table of every binding the worker reads, with the kind, the
   requirement, the deploy-time environment variable that supplies it,
   and the default a required var takes when omitted. The worker imports
   the roster's names, and a test in the crate reads the worker's source
   and `cachet-core`'s constants from disk and asserts both directions:
   nothing read is unrostered, nothing rostered is unread.
2. `plan(name, env)` derives a deployment from its name and its
   environment map, refusing exactly what `infra/src/config.ts` refuses,
   in its order, naming every missing value at once. The plan carries the
   names of secrets and never their values, and the property lane asserts
   over arbitrary maps that no plan and no error ever contains a value
   from the map.
3. `Manifest::template(version)` describes one release as a template: the
   worker's modules, compatibility, and limits; the resources to create;
   the bindings; the vars and secrets with placeholders from a closed set
   (`$NAME`, `$RESOURCE_NAME`, `$RESOURCE_NAME_UNDERSCORED`, `$HOST`,
   `$VAR(...)`, `$SECRET(...)`, `$VERSION`, `$GENERATED_PUBLIC(...)`);
   the cron; the route; the asset layer; the probes that verify a deploy;
   and, for the values an operator supplies, how a wizard asks. The
   template is committed as `deploy-manifest.json` and drift-gated by
   `scripts/check-deploy-manifest.sh`, the fourth bijection under
   CLAUDE.md §8. A release adds the measured artifacts and a signature.
4. The crate is pure. No I/O, no clock, no ambient environment read: the
   caller builds the map, the grammar reads it, and two machines with the
   same inputs produce byte-identical output, which the golden lane pins.
5. Everything past the roster sits behind a default-on `plan` feature the
   worker turns off, so the roster the wasm bundle imports brings no
   dependency with it.

## Consequences

The compatibility flags become explicit. alchemy added `nodejs_compat` on
its own for a worker with a compatibility date past 2024-09-23, so the
deployed script has carried it without the program naming it. The
manifest states it, because a deploy through the REST API would otherwise
change the runtime.

The alchemy program and the crate both hold the cron, the compatibility
date, and the CPU ceiling for now. The program is retired once
`cachet deploy` has deployed staging cleanly (ADR 0009's supersession lands
with that change), and until then the golden lane pins the crate's values
and a reviewer compares.

Adding a binding is one roster entry, and the bijection test says where
else it has to appear. Adding a value an operator supplies is a roster
entry plus a wizard hint, and the manifest carries both to every deploy
tool.

The workspace version now drifts two generated documents, so a bump
regenerates `docs/openapi.yaml` and `deploy-manifest.json` in the same
commit.

## Alternatives considered

**Keep the grammar in TypeScript beside alchemy.** The deploy program is
already TypeScript. Rejected: `tsc --noEmit` proves types and never
behavior, no lane runs TypeScript behavior tests, and the two properties
that matter here (the plan is a locked wire contract; no value from the
environment ever renders) want the golden and property lanes, which are
Rust.

**Replace every literal in the worker with a roster constant and stop
there.** Rejected: it proves each name is spelled once and still lets the
two sets drift apart. The source-scan test proves the sets agree, which
is the law that matters, and it holds with or without the literals.

**Generate `wrangler.toml` from the plan as sesame does.** Useful for a
self-hoster without the deploy tool, and deferred: `cachet deploy` is the
path this crate exists for, and a second rendered artifact is a second
thing to keep in step before the first has shipped.
