# ADR 0002 — OIDC writes, device-flow laptop reads, browser OAuth for the UI

- **Status:** Accepted
- **Date:** 2026-08-21
- **Context doc:** [../../CLAUDE.md](../../CLAUDE.md) §1; [../../SECURITY.md](../../SECURITY.md)

## Context

The cache has three caller classes with different powers. CI jobs push: they run inside GitHub-hosted runners that can prove platform identity.
Laptops read: people on machines they own, whose trust is their GitHub
membership. Browsers read for a future UI: same people, over the web.
One credential type for all three was the previous deployment's answer
(a shared read token in a vault), and it failed the test of a
replacement: a shared token cannot be revoked per person, and every
person who leaves the org is a race against a rotated distribution.

## Decision

1. Writes accept GitHub OIDC tokens only: RS256 signatures verified
   against GitHub's published JWKS with a pinned algorithm and exact
   `iss`/`aud` checks, `repository_owner` against the deployment's org
   list, a bounded staleness on `iat`, and `exp` against the request
   clock. The token is the write credential; nothing else signs
   anything.
2. Laptops authenticate through GitHub's device flow, run by the CLI
   against github.com directly. The worker never participates in the
   flow; it validates the resulting user token per request by calling
   `/user` and an org-membership endpoint, caching the verdict in KV
   (600s for allow, 60s for deny). The token is personal, stored 0600
   on the laptop, and revocable at github.com.
3. Browsers use the OAuth code flow against the same worker:
   `/_auth/login` stores a one-shot state record, `/_auth/callback`
   exchanges the code server-side, checks org membership, and sets an
   HttpOnly+Secure+SameSite=Lax session cookie; `/logout` deletes the
   session. State records are consumed on use.
4. CI jobs read with the same OIDC credential they write with: a worker
   that sees a three-segment base64url token dispatches it to the
   write-path verifier instead of the user-token path, so runners need
   no second credential for substitution.

## Consequences

Each credential is personal and dies with its holder's org membership
(at most one verdict TTL late). Revoking a laptop means revoking one
GitHub account, not redistributing a secret. The device flow works on
headless machines. The KV verdict cache bounds GitHub API load and
defines the revocation window explicitly, in writing.

## Alternatives considered

A shared pre-shared token (the old design): fails per-person
revocation; rejected. Fine-grained PATs distributed by the operator:
same distribution problem with extra bookkeeping; rejected. Requiring
OIDC for reads too, issued through a vending endpoint: adds a moving
part whose only customer is the laptop case the device flow already
serves; rejected.
