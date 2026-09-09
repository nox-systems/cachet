# ADR 0019: The collector fires hourly, and health measures against that

- **Status:** Accepted
- **Date:** 2026-09-08
- **Context doc:** [0005-gc-on-the-cron.md](0005-gc-on-the-cron.md);
  [../DEPLOY.md](../DEPLOY.md)

## Context

ADR 0005 gave the collector one firing a day and a per-invocation budget
of 900 binding operations, with runs that park a cursor and resume on the
next firing. Those two decisions interact in a way the original record
did not work through.

Before the sweep can delete a candidate's NAR, the collect stage has to
learn which NAR the candidate's narinfo names, and that is one bucket
read per candidate. A run plans nothing until it has collected every
candidate, so the reads a deployment can afford per day have to exceed
the paths that cross the grace window per day. One firing a day meant
about 860 reads after the listing and the mark stage took their share.

Production crossed that line on 8 September 2026, the first day anything
was old enough to sweep. The 14-day window expired on roughly 4,900
paths at once. The run collected 534 of them, parked, and waited for the
next day, while another day's worth of paths kept crossing the window at
over a thousand a day. The candidate set was growing faster than the
collector could read it, so no run would ever reach its sweep. Fourteen
runs had already deleted nothing, and the inventory had gone from 4,966
narinfos to 14,634.

The health route had a related problem, invisible until the schedule
changed. Its staleness bound was two days, written as a constant rather
than derived from the cron, so it happened to mean two cron periods
only while the cron was daily.

## Decision

1. The cron is `0 * * * *`. Twenty-four firings a day multiply the
   collect stage's daily allowance by twenty-four and change no single
   invocation's ceiling, so nothing about the per-run budget, the
   resumability, or the gates moves.
2. Health's staleness bound is two of the deployment's own cron periods,
   read from `CACHET_GC_CRON` through the schedule parser. A cron the
   parser does not recognize falls back to two days, which is the loose
   answer rather than one guessed from an expression nobody read.
3. The schedule parser recognizes the hourly shape (`M * * * *`) beside
   the daily one (`M H * * *`) and reports the period of each. Every
   other cron expression still answers nothing, so a console shows no
   countdown rather than a wrong one.
4. Health reads `meta/gc-cursor` and treats a parked run as proof the
   schedule fired. Reports land only when a run concludes, so a
   collection spanning many ticks leaves the newest report aging while
   the collector works, and with an hourly bound that would answer
   `degraded` during ordinary operation.

## Consequences

A deployment collects on the hour. A tick with nothing to do spends its
listing and exits, so the added firings cost a bucket listing an hour on
an idle cache. Production's 4,900-path backlog drains in about six
firings instead of never.

The colour on the console now means something on both schedules: an
hourly deployment goes `degraded` after two missed hours, where it used
to need two days of silence, which is forty-eight missed firings.

Health costs one more bucket read per request. It is an admin route the
console polls, and the read is a `get` on one small key.

## Alternatives considered

**Raise `GC_OP_BUDGET`.** More reads per firing rather than more
firings. Rejected: the budget exists to keep one invocation inside the
platform's subrequest and CPU ceilings, and raising it trades a
collector that cannot finish for invocations that get killed partway.

**Bound the collect stage instead: plan and sweep what has been
collected so far.** This converges without touching the cron, and it is
the more thorough fix, because it removes the requirement that one run
see every candidate. Rejected for now: partial plans mean a NAR whose
narinfo was collected in one run and swept in another, and working out
which orderings stay safe is a larger change than the schedule that was
starving it. Worth doing if the read per candidate ever becomes the
bottleneck again.

**Store each narinfo's NAR key beside it at write time** so the collect
stage needs no read at all. The measurement written when a NAR lands
(ADR 0012) is the natural place. This is the real fix to the cost and
not just to the allowance, and it stays open.
