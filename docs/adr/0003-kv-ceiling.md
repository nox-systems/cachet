# ADR 0003 — KV holds verdicts, sessions, and OAuth state only

- **Status:** Accepted
- **Date:** 2026-08-21
- **Context doc:** [../../CLAUDE.md](../../CLAUDE.md) §1

## Context

Cloudflare gives a Workers deployment three obvious stores: R2 (object
storage, strong consistency, generous sizes), KV (globally replicated
key-value, eventually consistent, small values), and platform caches.
Narinfos, NARs, leases, upload records, GC cursors, and GC reports all
want durability and exact reads, which is R2's contract. Auth answers
want speed at the edge and short lives, which is KV's. The previous
deployment put no data in KV at all; the new auth design (ADR 0002) has
two places where a cached answer is the difference between a hot path
and a GitHub API call per request.

## Decision

1. R2 is the only cache-data store: object bytes and every worker-owned
   document (leases, upload records, GC cursors, GC reports, the
   generation marker) live there.
2. KV holds exactly three document kinds: token-validation verdicts
   (the `/user` plus org-membership answer, 600s allow TTL, 60s deny),
   browser sessions (14-day TTL), and OAuth state records (10-minute
   TTL, single-use).
3. Every KV document carries its own TTL; nothing in KV is expected to
   be true later than its TTL, and loss of the namespace degrades auth
   to the uncached path while keeping it available.

## Consequences

The set of possible consistency bugs halves: every correctness-critical
read goes to a strongly-consistent store, and every KV answer is the
one the system would compute again anyway. The revocation window for a
GitHub-side change (membership removed, token revoked) is the verdict
TTL and is written down. KV's eventual consistency never
lurks in GC or lease decisions.

## Alternatives considered

R2 for verdicts too: workable but pays R2's per-read latency on every
authenticated read and muddies the inventory enumeration with auth
documents; rejected. No KV at all (the old deployment): the per-request
GitHub API calls would rate-limit real usage; rejected. Durable Objects
for sessions: stronger than needed, and a single-writer object per
session buys nothing for a read-mostly session; rejected.
