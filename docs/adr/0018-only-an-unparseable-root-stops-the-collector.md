# ADR 0018: Only an unparseable root narinfo stops the collector

- **Status:** Accepted
- **Date:** 2026-08-26
- **Context doc:** [0005-gc-on-the-cron.md](0005-gc-on-the-cron.md);
  [0017-the-collector-refuses-on-blindness-not-on-scale.md](0017-the-collector-refuses-on-blindness-not-on-scale.md)

## Context

The mark phase walks from the roots the leases name. When a narinfo read
fails, the walker learns which way it failed: `Absent` means the bucket
answered that no such object is there, and `Unparseable` means the object
is there and its text does not parse. A read that fails because the R2
binding itself failed is neither. It freezes the run for the next tick
without answering the walker at all.

Both failures were treated the same way at a root, and both tripped the
`unreadable_root_narinfo` gate, which aborts the run and deletes nothing.
Deeper in the closure the same two failures were treated the other way:
the path is marked, `unreadableDeep` counts it, and the walk goes on.

The absent case has the deadlock shape ADR 0017 removed from the sweep
fraction. A lease naming a path whose narinfo is gone keeps naming it,
and nothing about a later run brings the narinfo back, so every
collection from then on aborts at the same root. Staging reached that
state: every run answered "stopped at the unreadable_root_narinfo gate,
nothing was deleted", and the exits were pushing that project again or
editing the lease by hand.

Refusing there also protects nothing. A path whose narinfo is absent is
a path no client can substitute, because a substitution starts by
fetching that narinfo. Its references cannot be enumerated, and nothing
reachable only through it is reachable at all.

The unparseable case is different. That root is present and servable: a
client fetches the narinfo, gets bytes, and the worker answers. Its
references cannot be read, so continuing would sweep a closure whose top
a client still reaches. The collector's picture of what is live is
incomplete there, which is the condition ADR 0017 kept the remaining
gates for.

## Decision

1. An absent root narinfo does not gate. It is marked, so the sweep
   never touches it, and it is counted in `unreadableDeep` the same way
   an absent reference deeper in the closure has always been counted.
2. An unparseable root narinfo still trips `unreadable_root_narinfo`,
   and the run aborts having deleted nothing.
3. A read that fails because the R2 binding failed keeps freezing the
   run for the next tick, unchanged. That distinction is what makes the
   first point safe: `Absent` is the bucket answering that the object is
   not there, and a binding failure never reaches the walker as one.

## Consequences

A lease naming a path the cache no longer holds costs one counted
unreadable reference per run instead of stopping collection for good.
Deployments already stuck resume on the next tick with no operator
action.

`unreadableDeep` on a report now counts absent roots alongside absent
references, so a number that climbs says leases are naming paths that
left the cache. The gate used to deliver that signal by stopping
everything, and this delivers it without stopping anything.

## Alternatives considered

**Drop the root gate entirely.** Treat unparseable like absent and let
the walk continue past both. Rejected: an unparseable root is servable
with references nobody can read, so continuing sweeps live paths clients
can still reach through it.

**Delete the lease when its root is absent.** The collector repairs the
condition rather than counting it. Rejected: the collector reads leases
and never writes them, and a lease naming a path that gets pushed again
tomorrow describes a real project whose cache entry has expired.
