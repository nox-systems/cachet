# ADR 0022: A workflow run trades its OIDC token for a credential that lasts the job

- **Status:** Accepted
- **Date:** 2026-10-13
- **Context doc:** [../../README.md](../../README.md) (For CI);
  amends point 5 of [ADR 0002](0002-three-authentication-flows.md)

## Context

The action wrote the job's GitHub OIDC token into `/etc/nix/netrc` as the
cache password. The nix daemon presents whatever that file holds on every
substitution, with no code of ours in the path (ADR 0002), and GitHub's
OIDC tokens are short-lived: the example token on GitHub's OpenID Connect
page carries an `exp` 300 seconds after its `iat`
(https://docs.github.com/en/actions/concepts/security/openid-connect). A
job that ran longer than its token lost the cache partway through.

nox-systems/actual's main run 38468685026 on October 13, 2026 shows it in
its `fuzz` job. The action provisioned the credential at 21:11:32, its
read probe answered 404 (authenticated) at 21:11:41, and 293 paths
substituted between 21:12:04 and 21:12:54. From 21:18:29 every narinfo
request answered 401, 662 of them, and the job built everything else from
source until the runner's disk filled. The probe passed because it ran
nine seconds after provisioning, inside the token's life.

Laptops had the same problem with GitHub's eight-hour user tokens, and
ADR 0002 solved it by having the deployment issue its own credential at
`POST /api/login/exchange`. A run cannot use the device flow, but it holds
something the deployment already knows how to verify: its OIDC token.

## Decision

1. `POST /api/login/exchange` accepts a GitHub OIDC token as a second
   identity kind. The presented credential's grammar picks the verifier,
   as it does on every read: three base64url segments go to the write
   path's verifier (RS256 against GitHub's JWKS, the exact issuer and
   audience, the served orgs, and the token's `exp` and `iat`), and
   anything else takes the GitHub path ADR 0002 describes.
2. The answer is the same `cachet_` credential a laptop gets. The
   deployment stores only its SHA-256, under `readtoken/<digest>`, against
   a record naming the repository owner and the claims the token proved:
   repository, ref, run id, run attempt, check run id, and commit. The
   OIDC token is not stored. Those claims name the job that holds the
   credential: every job of a workflow run shares the run id and attempt,
   and the check run id ("The check run ID of the current job",
   https://docs.github.com/en/actions/reference/security/oidc) tells one
   job from another. A token missing any of the three is refused, because
   points 6 and 7 count and revoke credentials per job.
3. The record lives six hours from the exchange. GitHub ends a job on a
   GitHub-hosted runner after six hours ("Each job in a workflow can run
   for up to 6 hours of execution time",
   https://docs.github.com/en/actions/reference/limits), and a job's
   `timeout-minutes` defaults to 360 on any runner
   (https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax).
   Nothing re-checks the run during those hours. Its OIDC token has
   expired and the deployment never kept it, so nothing could.
4. A run's credential resolves to the same CI identity its OIDC token
   resolves to. It reads, it is never an admin, and it cannot write,
   because writes accept an OIDC token and nothing else.
5. The action exchanges the token as soon as it has it, before nix
   starts, and writes the issued credential to the netrc. When the
   deployment answers anything but a credential, the action writes the
   OIDC token as every earlier version did and logs one line saying the
   cache stops answering the job when that token expires. A job without
   the `id-token` grant builds uncached, as before.
6. One job holds at most four credentials. The exchange counts a job's
   credentials under `runtokens/<job>`, where `<job>` is the digest of
   the repository, run id, run attempt, and check run id, and a fifth
   exchange answers 403 `credential_limit`. Four covers a job that uses
   the composite twice and an exchange retried after its answer was
   lost. Without a bound, any step holding the job's `id-token` grant
   could mint credentials without limit, each outliving the job by up to
   six hours and each costing a KV write. The count bounds the exchange
   and nothing else: no read consults it.
7. Revoking a run's credential revokes every credential of its job. The
   revocation writes `runrevoked/<job>`, which nothing else writes, and a
   read of any of the job's credentials refuses while it exists, as does
   any further exchange for the job, even with an OIDC token minted
   before the job ended. Other jobs of the same run are untouched.
   `POST /api/login/revoke` answers 204 when there was a record to revoke
   and 404 when there was not. The 404 tells nobody anything, because
   only the holder of a credential can name its record.
8. A nested action, `action/revoke`, revokes the job's credential in its
   post phase, whether the job succeeded or failed, and never fails the
   job. Point 7 takes credentials a step of the job minted for itself
   with it, along with the credential of a second use of the composite
   in the same job, which shares the job's check run id. The revoke action is the composite's first step.
   GitHub runs post phases in the reverse order of their main phases and
   runs one only for a step whose main phase ran, so the first step's
   post phase should run after the push's and still run when a later
   step fails. GitHub documents that order for a job's steps and not for
   actions nested in a composite, so the release confirms it in a real
   job's log (below). The actionlint gate holds the step order.
9. Pushes are unchanged. The push mints a fresh OIDC token when the post
   step starts and mints again after any 401, so it never presents an
   expired one.

## Consequences

A run's read credential now outlives its OIDC token by up to six hours.
A copy taken during the job, by a dependency the build ran for example,
reads the cache until the revoke step runs, or until the record expires
when that step cannot run (a cancelled runner, or one without
passwordless sudo to read the root-owned netrc). The OIDC token the netrc
used to hold was a write credential for its five minutes, and the issued
one only reads, so a stolen copy now lasts longer and can do less.

A self-hosted job that raises `timeout-minutes` past six hours outlives
its credential and builds the rest of the job uncached.

A step holding the job's `id-token` grant, or anyone who steals an OIDC
token during its five minutes, can mint at most four credentials for the
job, counting the action's own. All of them end when the revoke step
runs. A stolen token therefore buys reads until the job ends, or up to
six hours when the revoke step cannot run, in place of the five minutes
of reads and writes the token itself allowed.

The count is one KV entry per job, read and written whole by each
exchange. Two exchanges for one job that race on it can lose an entry,
which admits one credential past the bound, and KV takes one write a
second to one key, so an exchange that follows the job's previous one
within a second can answer 503 and is retried. The action exchanges once
per use of the composite, so neither happens to a job that is not racing
itself, and different jobs never share the entry. Listing KV keys instead
of keeping one count was rejected because a listing lags writes by up to
a minute. Revocation is kept out of the count on purpose: an exchange
that read the count before a revocation, or from a colo whose copy was
stale, writes it back afterwards, and a mark kept inside it would have
been undone.

A read of a run's credential costs two KV reads, its record and its
job's revocation mark, and the isolate memo answers repeats for two
minutes. The mark is normally absent, so a read straight after the
exchange cannot be refused by a KV copy that has not caught up. A
revocation reaches a colo that cached the mark's absence within KV's
own propagation, up to a minute and sometimes more, plus the memo's two
minutes.

Each CI job costs the deployment three KV writes and one delete: the
exchange writes the credential's record and its job's count, and the
revoke writes the job's revocation mark and deletes the record. Workers Free allows
1,000 KV writes a day (https://developers.cloudflare.com/kv/platform/limits/),
about 330 jobs, so a deployment serving CI expects Workers Paid.

The worker before this change refuses a run's record as corrupt, because
it requires the GitHub token a laptop's record carries. Rolling the
worker back therefore ends every run credential at once, and jobs that
start afterwards fall back to their OIDC tokens. For the same reason the
worker deploys before the action's tag moves: an action released first
finds an old deployment, falls back, and says so in the job log.

The composite refers to `nox-systems/cachet/action/revoke@v0`, which
exists only once the `v0` tag points at a commit that has it. Releasing
therefore takes these steps, in order:

1. Deploy the worker, staging and then production.
2. Move the `v0` tag to the release commit.
3. In a real job that uses the action, confirm that the log shows the
   push's post step finishing before the line `cachet: revoked this
   job's read credential`. That line is the runtime proof of point 8's
   order, which GitHub does not document for nested actions.

## Alternatives considered

**Rewrite the netrc with a fresh OIDC token during the job.** The daemon
reads the file on every substitution, so a refresher would work if
something ran it. Rejected: a composite action's steps run before the
job's own steps and cannot leave a process running beside them that the
runner is guaranteed to clean up, and every refresh puts another write
credential at rest.

**A separate credential system for runs**, with its own prefix, route,
and records. Rejected: the exchange route, the `cachet_` grammar, the
digest-keyed records, revocation, and the isolate memo already did
everything a run needed, and a second copy would have to be kept in step
with the first.

**Tie the credential's life to the job's end.** No signal reaches the
deployment when a job ends, so the revoke post step is the closest
available bound, and the six-hour expiry covers the cases where it
cannot run.
