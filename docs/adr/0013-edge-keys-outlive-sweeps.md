# ADR 0013: An object's edge-cache key carries no generation

- **Status:** Accepted
- **Date:** 2026-08-25
- **Context doc:** [testing/workerd.md](../testing/workerd.md)

## Context

Every edge-cache entry was keyed under the current generation:
`https://cachet-edge.invalid/g{N}/{path}`. Bumping the generation
changed every object's key at once, which invalidated the whole cache in
every point of presence without a purge API or a per-key walk.

The collector bumps the generation whenever a sweep deletes anything, and
it runs daily. Objects are given a thirty-day `immutable` lifetime, so
that lifetime could never be reached: one deleted path each morning threw
away every warm entry everywhere. The first substitution of the day after
a sweep was a cold bucket read for the entire working set.

## Decision

1. An object that exists is keyed without a generation, under
   `/object/{path}`. Both kinds of object are addressed by a hash of
   their own content: a NAR key names the hash of its bytes, and a
   narinfo describes one store path's fixed facts. A generation says
   nothing about whether those bytes are still those bytes.

2. An absence keeps the generation, under `/g{N}/miss/{path}`. A cached
   404 is the only entry on the object path that a write can make wrong,
   and a sweep followed by a push is exactly when it would be. These
   entries live thirty seconds anyway, so the prefix costs a sweep
   nothing measurable.

3. A generation that will not resolve therefore degrades the negative
   half only. A stored object still answers from the edge, because
   nothing about a corrupt generation document bears on whether its bytes
   are its bytes.

## Consequences

The thirty-day lifetime is now reachable, and a sweep no longer empties
the cache it swept one path from.

An object the collector deleted can still answer from a warm point of
presence until its entry expires. That is a path nothing references any
more, and a client that substitutes it gets bytes that verify against the
narinfo naming them, so the staleness is invisible rather than wrong.

A narinfo is not strictly immutable: a key rotation re-signs it. A
rotation adds a key rather than retiring one (docs/DEPLOY.md), so the
older signature still verifies for every client configured before it, and
the stale entry expires on its own.

## Alternatives considered

**Keep the global bump.** Rejected: it is correct and it is why the cache
was cold every morning. Prompt invalidation of content-addressed bytes
buys nothing, because those bytes cannot go stale.

**Purge the affected keys on deletion.** Rejected: the Cache API's delete
reaches the point of presence that serves the request, not the others, so
it cannot stand in for a global invalidation.

**Drop the generation entirely.** Rejected: negative entries genuinely
need it. Without a generation prefix a sweep-then-push sequence could
leave a freshly pushed path reading as absent for its whole window.
