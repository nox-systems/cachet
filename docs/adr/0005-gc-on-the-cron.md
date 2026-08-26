# ADR 0005: The collector is armed from day one, on the cron

- **Status:** Accepted; point 4's sweep fraction removed by
  [ADR 0017](0017-the-collector-refuses-on-blindness-not-on-scale.md)
- **Date:** 2026-08-21
- **Context doc:** [../../CLAUDE.md](../../CLAUDE.md) §1; [../testing/workerd.md](../testing/workerd.md)

## Context

The previous deployment had a mark/sweep design it never enforced in production: turning deletion on was a separate operation from deploying the cache, and it stayed off.
The bill was the small cost; the staleness was the real one, since
nothing enforced that dead paths ever leave, so the cache's semantics drifted
from "what projects need" to "everything CI ever built."

The design constraint that shaped the implementation: a deployment's
inventory can exceed one invocation's budget, and Cloudflare caps
worker CPU-time, subrequests, and wall time. A correct collector must
survive partial progress, and a safe one must refuse to sweep when the
mark phase's inputs are suspect.

## Decision

1. The collector runs on the worker's cron trigger (`0 5 * * *`),
   armed by default: `CACHET_GC_ARMED=0` disarms, everything else
   sweeps. Arming is not an operator decision point.
2. One invocation advances the run as far as a budget of 900 binding
   operations and a 13-minute headroom allow, then persists a cursor
   document into the bucket and exits cleanly. The next tick resumes
   exactly there. A run of any inventory size completes over N ticks.
3. Stages recompute at every boundary from bucket truth: inventory and
   leases never persist across ticks; mark/collect/sweep progress does,
   as cursor state.
4. The gate tripped by an unparseable lease, an unreadable root, or a
   truncated enumeration aborts the run and lands a report naming the
   gate. Nothing is deleted when the inputs are not whole. (This point
   also named a sweep fraction over 25% of inventory; ADR 0017 removed
   it, because a gate on how much a run may delete could not be
   satisfied by the run after it.)
5. Narinfos delete before their NARs, in batches, so a narinfo never
   dangles a missing NAR at any instant. Orphan NARs (referenced by no
   narinfo) survive; that is a v2 decision, guarded now by the grace
   window rather than by a bound on the sweep's size.
6. Every run lands a report at `gc-reports/{runId}.json` plus a
   `latest.json`, and the admin API serves the history.

## Consequences

Deployments get deletion with no operational surface: the cron arrives
provisioned, the first night's run sweeps what the grace window allows,
and a halt mid-run is normal operation, not an incident. The budget
split (recomputed reads vs. frozen plan state) costs some duplicate
bucket reads on multi-tick runs, accepted for resumability without
trust in persisted enumeration.

## Alternatives considered

Single-invocation sweeps with no cursor (the old design): fails open at
scale: the run dies at a platform limit and the next run starts over;
rejected. Deletion behind a manual flag or an external runner: the old
failure mode institutionalized; rejected. Sweeping orphan NARs in v1:
an inventory-level claim the mark phase cannot cheaply prove; rejected
for v1's shape.
