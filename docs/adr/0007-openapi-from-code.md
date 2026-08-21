# ADR 0007: The OpenAPI document is generated from code

- **Status:** Accepted
- **Date:** 2026-08-21
- **Context doc:** [../../CLAUDE.md](../../CLAUDE.md) §8

## Context

A public cache's API surface is a contract: the CLI, the action, and
every org's dashboards consume it, and drift between the documented and
actual surface is a bug class that compiles clean. The previous
deployment had no generated document; routes and their rejection
matrices lived in prose.

## Decision

1. cachet-api owns the HTTP surface as typed route descriptors with
   utoipa derive macros, pinned to utoipa 5.5.0 exactly, because
   generation behavior is part of the output.
2. `just openapi` regenerates docs/openapi.yaml from the descriptors;
   the committed file is the only copy, formatted by the generator
   alone (treefmt excludes it).
3. The gate `just openapi-check` regenerates and diffs: drift between
   descriptor and document fails CI.
4. The worker serves the committed document at `/api/openapi.json`
   byte-for-byte; the workerd lane asserts served == committed, so the
   bijection closes from both directions.

## Consequences

The spec can never silently diverge from the implementation: a route,
a rejection code, or a body shape that changes without regenerating
fails the gate. Reviewers read one document for the whole surface,
including the rejection matrix. The cost is the yoking: every surface
edit pays a regen step, and utoipa stays pinned exactly.

## Alternatives considered

Hand-maintained OpenAPI authored first, with code generated from it:
the document becomes the design surface, which inverts where the truth
lives and makes the worker the downstream artifact; rejected. No
machine-readable spec (the old state): the API surface then exists
only in prose and tests; rejected.
