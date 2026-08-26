# ADR 0017: The collector refuses on blindness, not on scale

- **Status:** Accepted
- **Date:** 2026-08-26
- **Context doc:** [0005-gc-on-the-cron.md](0005-gc-on-the-cron.md);
  [../security/threat-model.md](../security/threat-model.md)

## Context

The collector carried three abort gates. Two fire when it cannot see the
truth: an unreadable root narinfo or an unparseable lease means the mark
set is wrong, and a truncated enumeration means the inventory is
incomplete. The third fired on scale: a sweep whose candidate count
exceeded a quarter of the narinfo inventory aborted the run.

The third one cannot be satisfied. Aborting produces an empty deletion
plan, so nothing is deleted, so the next run enumerates the same
inventory and computes the same candidates and aborts again. The
candidate set only grows, because every path already past its grace
window stays past it. A deployment where more than a quarter of the cache
is genuinely dead stops collecting permanently, and the only exit is a
person with bucket access.

That is the wrong failure for this system twice over. A binary cache's
characteristic problem is unbounded growth, and this gate converts "a lot
is dead" into "nothing is ever collected". And the thing it protected
against is cheap here: over-deleting from a cache costs a rebuild and a
re-push, not data. The store paths still exist in every machine that
built them.

## Decision

1. The sweep fraction gate is removed, along with its constants, its
   `GateTrip` variant, and its report value.
2. The gates that remain are the ones that fire on blindness: an
   unreadable root narinfo, an unparseable lease, a truncated inventory
   or lease listing, an exhausted walk budget, and a corrupt generation
   document. Each of those means the collector's picture of what is live
   is incomplete, which is the condition worth refusing on.
3. How much one run may delete is not bounded.

## Consequences

A collection deletes everything the mark phase proved dead, however much
that is. A deployment that decommissions half its repositories collects
normally on the next tick instead of jamming.

The blast radius of a mark-phase bug widens: one that failed to mark live
paths would now sweep them. What limits it is unchanged and is the part
that was always doing the work: nothing is swept until its grace window
past the last lease that named it has passed, so a mistake needs to
survive fourteen days of nothing referencing a path. And the recovery is
a push, because that is what a cache is.

## Alternatives considered

**Keep the gate and add an override.** A variable an operator sets to let
one run through. Rejected: it makes the deadlock a thing you have to know
about and then know how to escape, and the escape is a setting nobody
reads until they are already stuck.

**Raise the fraction.** Any fixed fraction has the same deadlock at some
inventory, and picking a bigger number only moves the wall.

**Cap the deletions per run rather than refusing them.** Sweep at most N
and let successive runs drain the rest. This is a real option and would
bound a bug's damage per night while still converging. Rejected for now
because it adds a partial-sweep state to a collector whose runs are
currently whole, and the grace window already gives the same protection
in time rather than in count. Worth revisiting if a mark-phase bug ever
does happen.
