# ADR 0002: OIDC writes, deployment-issued laptop reads, browser OAuth for the UI

- **Status:** Accepted
- **Date:** 2026-08-25
- **Context doc:** [../../CLAUDE.md](../../CLAUDE.md) §1; [../../SECURITY.md](../../SECURITY.md)

## Context

The cache has three caller classes with different powers. CI jobs push;
they prove platform identity by running inside GitHub-hosted runners.
Laptops read: people on machines they own, whose trust is their GitHub
membership. Browsers read for a future UI: same people, over the web.
One credential type for all three was the previous deployment's answer
(a shared read token in a vault), and it failed the test of a
replacement: a shared token cannot be revoked per person, and every
person who leaves the org is a race against a rotated distribution.

The laptop case has a second constraint that took a while to surface.
Whatever credential a laptop holds is read by the nix daemon out of a
netrc file, on every substitution, with no code of ours in the path.
Nothing can refresh it, retry it, or notice it has gone stale. So the
laptop's credential has to be one that does not need refreshing on a
timescale a person would notice, and it has to be one the deployment is
willing to be handed on every request.

A GitHub user token is neither. GitHub made short-lived user tokens the
default for new OAuth Apps in August 2026: eight hours, with a refresh
token the daemon has no way to use. A laptop closed overnight would open
to a dead credential, silent 401s, and every build recompiling from
source. And sending it on every request meant the deployment accumulated
live, replayable `read:org` tokens for everyone who had ever logged in,
which is a store of other people's credentials it has no reason to keep.

## Decision

1. Writes accept GitHub OIDC tokens only: RS256 signatures verified
   against GitHub's published JWKS with a pinned algorithm and exact
   `iss`/`aud` checks, `repository_owner` against the deployment's org
   list, a bounded staleness on `iat`, and `exp` against the request
   clock. The token is the write credential; nothing else signs
   anything.
2. Laptops authenticate through GitHub's device flow, run by the CLI
   against github.com directly, and then trade the result for a
   credential the deployment issues. `POST /api/login/exchange` takes
   the GitHub token and its refresh token, checks `/user` and org
   membership, and answers an opaque token prefixed `cachet_`. The
   deployment stores that token's SHA-256, under `readtoken/<digest>` in
   KV, against a record naming the login and holding the GitHub
   credentials. The laptop keeps only the issued token, so no GitHub
   credential ever reaches the netrc or crosses the wire again.
3. The issued token is a pointer, not a verdict. Every read resolves it
   to its record and re-checks membership against GitHub through the
   same verdict cache every other credential uses, 600s for an allow and
   60s for a deny, so someone who leaves the organisation loses access
   within that TTL. The GitHub token is consulted only when that cached
   verdict has lapsed, and only then, if it is near its eight hours, is
   it renewed from the stored refresh token, which needs no client
   secret for a device-flow grant. Nobody logs in again for it. The
   order matters: GitHub rotates a refresh token on use, and nix opens a
   build with a couple of dozen requests at once, so renewing before
   checking the verdict would have had them all race and all but one
   present a spent token. A request that loses that race anyway re-reads
   the record and uses what the winner wrote.
   `READ_TOKEN_TTL_MS`, thirty days, is the outer bound that stops an
   unused credential living forever. `POST /api/login/revoke` deletes
   the record and clears the issuing isolate's memo; `cachet logout`
   calls it.
4. Browsers use the OAuth code flow against the same worker:
   `/_auth/login` stores a one-shot state record, `/_auth/callback`
   exchanges the code server-side, checks org membership, and sets an
   HttpOnly+Secure+SameSite=Lax session cookie; `/logout` deletes the
   session. State records are consumed on use.
5. CI jobs read with the same OIDC credential they write with: a worker
   that sees a three-segment base64url token dispatches it to the
   write-path verifier instead of the issued-token path, so runners need
   no second credential for substitution. The three shapes are told
   apart by their grammar, not by trying each verifier in turn: the
   `cachet_` prefix and a fixed body length name an issued token, three
   base64url segments name an OIDC token, and anything else is a GitHub
   token checked upstream.

## Consequences

Each credential is personal and revocable one holder at a time, and all
three classes now die with their holder's GitHub standing at the same
bound: one verdict TTL. A laptop that sat closed for a fortnight opens
working, because the renewal that GitHub's eight-hour token needs
happens on the deployment's side of the wire.

Nothing replayable crosses the wire. What the daemon sends a thousand
times a build authenticates against this deployment and nowhere else,
and a copy of it is useless anywhere but here.

The trade is where the GitHub credentials rest. They are in the
deployment's KV rather than on the laptop and in every request: one
place, at rest, under the deployment's own control, alongside the
browser sessions KV already holds (ADR 0003). That is a store worth
naming, and it buys both the revocation window and the absence of
credentials in transit. An operator who would rather hold nothing can
run an OAuth App whose tokens do not expire, in which case no refresh
token exists to store.

A read costs one KV read for the record plus the verdict lookup, which
the isolate memo answers for free on a warm isolate.

## Alternatives considered

**Keep the GitHub token in the netrc and refresh it.** Rejected: nothing
is positioned to do the refresh. The daemon reads the file directly, so
a refresh only happens when the person runs a cachet command, which
means the credential is stale exactly when they have not been using it
and the failure is a silent fall back to building from source.

**Issue the token and keep nothing, accepting a thirty-day revocation
window.** Rejected: shipped briefly and withdrawn. A month of access
after someone leaves is not a window an operator can be asked to accept,
and the reasoning that led there confused "the laptop must not hold a
GitHub credential" with "nobody may". Holding it server-side satisfies
the first and keeps membership checkable.

**A revocation denylist rebuilt on a schedule.** Rejected: it answers
the wrong question. Checking whether a token was revoked at github.com
is not checking whether its holder is still in the organisation, and
those come apart exactly when it matters, because leaving an org does
not revoke a personal access token.

**A shared token** (the old design): fails per-person revocation;
rejected. **Fine-grained PATs distributed by the operator**: same
distribution problem with extra bookkeeping; rejected. **Requiring OIDC
for reads too, issued through a vending endpoint**: that is what point 2
became, with the device flow as its front door rather than a second
moving part.
