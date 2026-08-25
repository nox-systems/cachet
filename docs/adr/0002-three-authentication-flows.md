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
   the GitHub token, checks `/user` and org membership once, and answers
   an opaque token prefixed `cachet_`. The deployment stores only that
   token's SHA-256, under `readtoken/<digest>` in KV, against a record
   naming the login and an expiry. The GitHub token is not stored by
   either side and never reaches the netrc.
3. An issued token is accepted for `READ_TOKEN_TTL_MS`, thirty days, and
   that window is the deployment's revocation control for a laptop.
   `POST /api/login/revoke` deletes the record and clears the issuing
   isolate's memo; `cachet logout` calls it. Nothing re-checks GitHub
   membership while a token is live, because after the exchange nothing
   holds a GitHub credential with which to ask.
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

Each credential is personal and revocable one holder at a time. A laptop
logs in once a month rather than once a shift, and a machine that sat
closed for a fortnight opens working. The deployment holds no GitHub
credentials: the worst a stolen KV export yields is a set of SHA-256
digests, which cannot be presented anywhere. Reads get cheaper as a side
effect, because an issued token resolves against one KV record instead
of two GitHub API calls.

The revocation window for a laptop widens from one verdict TTL to the
issued token's lifetime. Someone who leaves the org keeps read access to
the cache until their token expires or an operator deletes its record.
That is the price of a credential the daemon can carry, and it is
written down here rather than discovered: thirty days is the number, and
an operator who wants sooner deletes the record. It does not apply to
the other two classes, whose credentials still die with their GitHub
standing: an OIDC token expires in minutes, and a browser session
re-checks membership against the verdict cache's 600 seconds.

## Alternatives considered

**Keep the GitHub token in the netrc and refresh it.** Rejected: nothing
is positioned to do the refresh. The daemon reads the file directly, so
a refresh only happens when the person runs a cachet command, which
means the credential is stale exactly when they have not been using it
and the failure is a silent fall back to building from source.

**Issue the token but keep re-checking membership.** Rejected as
unbuildable rather than undesirable: re-checking needs a GitHub
credential for that user, so keeping the ability means keeping the
token, which is the thing being removed. An org-installed GitHub App
could list members without one, but that replaces the OAuth App every
deployment already creates and is a larger change than the window it
narrows.

**A shared token** (the old design): fails per-person revocation;
rejected. **Fine-grained PATs distributed by the operator**: same
distribution problem with extra bookkeeping; rejected. **Requiring OIDC
for reads too, issued through a vending endpoint**: that is what point 2
became, with the device flow as its front door rather than a second
moving part.
