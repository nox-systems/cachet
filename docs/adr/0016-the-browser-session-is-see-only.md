# ADR 0016: The browser session is see-only

- **Status:** Accepted
- **Date:** 2026-08-26
- **Context doc:** [0002-three-authentication-flows.md](0002-three-authentication-flows.md);
  [../security/threat-model.md](../security/threat-model.md)

## Context

ADR 0002 gave browsers the third credential: an OAuth code flow against
the worker, a KV session record, and an HttpOnly cookie. The session was
resolved by the same `authorize_read` every other read credential goes
through, and nothing downstream distinguished it, so a session cookie
authenticated narinfo and NAR reads exactly as a laptop's token did.

Two facts about that cookie make it a worse credential than the ones
beside it. It is sent automatically by the browser to every path on the
origin, which is what cookies do. And its record holds a login and a
creation time and no GitHub credential, so unlike a laptop's issued token
there is nothing to re-check membership with: the session is accepted
until its fourteen-day expiry. A person who left the org kept a working
cache-read credential for up to a fortnight, and the threat model claimed
otherwise, describing a 600-second verdict re-check the code did not
perform.

Building the console made this matter more, because the console is the
reason sessions exist at all and it made them common.

## Decision

1. A session identity authenticates the console's own surface:
   `/api/self/*`, `/api/whoami`, `/api/probe`, and `/roots`.
2. It is refused on the paths that serve cache bytes, `{hash}.narinfo`
   and `/nar/{key}`, with 401 `unauthorized`: the same answer an
   anonymous read gets, because naming the reason would tell a caller
   which credential class it holds.
3. The fourteen-day window on the console's surface stands, and the
   threat model says so plainly instead of claiming a re-check.

## Consequences

Nothing that reads for real loses a credential: nix reads through a
netrc, and the daemon never sends a cookie. What the change removes is a
credential that could be copied out of a browser's cookie jar and used to
substitute from the cache.

An ex-member's session can still read counters and collection reports
until it expires or an operator deletes it. That is a narrower thing to
be exposed than the cache's contents, and it is now the written bound
rather than an unwritten one.

The console gains nothing to work around: every screen it has reads the
surface a session still authenticates.

## Alternatives considered

**Store the GitHub token on the session record and re-check membership,
the way ADR 0002 does for laptops.** This would close the counter window
to 600 seconds as well. Rejected for now: it puts a second GitHub
credential at rest in KV (ADR 0003 weighs that cost) to shorten exposure
of a surface that shows counts rather than contents. If the console ever
serves something worth a tighter bound, this is the change to make.

**Shorten `SESSION_TTL_MS`.** Rejected: it trades the window against how
often a person has to sign in again, and it would not have stopped a
copied cookie from substituting within the window.

**Correct the threat model to match the code and change nothing.**
Rejected: it leaves the console's own credential as the only one that can
read the cache without its holder's org membership ever being rechecked.
