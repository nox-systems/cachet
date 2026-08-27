# Threat model

cachet sits in the supply chain of every machine that substitutes from
it, so the model starts from the worst placement: what a defender must
still hold when each credential class is lost. Every scenario names the
defense in force and where it is proven. The deployment's own operator
(Cloudflare account, API tokens, OAuth App custody) is outside this
document's scope; the repo's answer for it is the pinning, gating, and
byte-verification habits the rest of this file describes.

## What is protected

The signing key, held as a Workers secret binding. The bucket's object
integrity: what a client substitutes must be what CI's build produced.
The org boundary: nobody outside the configured orgs authenticates for
anything. The OAuth client secret, browser sessions, and the read
credentials the deployment issues to laptops. The action
users' CI jobs, from cachet's own release artifacts. GC's correctness:
live paths (leases and their closures) are never swept.

## Scenarios

**Poisoned narinfo.** An attacker with a valid write credential uploads
a narinfo naming bytes other than what CI built: a wrong `NarHash`,
a wrong `NarSize`, a NAR that is not the compressed form of the store
path. Defense: verify-then-sign; the worker measures the stored object's
hashes and signs only a narinfo whose declared facts match the stored
bytes. The measurement happens on the write that stores those bytes,
which records it beside the object, and the narinfo request reads the
record rather than the object (ADR 0012); a multipart upload measures on
completion, because its parts arrive out of order. The verified value is
a type the signer cannot be called without (ADR 0001), and a narinfo
whose NAR was never measured is refused rather than signed. Proven: the
rejection matrix in the workerd lane (hash mismatch, size mismatch,
missing NAR, malformed lines, bytes that do not hash to the key naming
them), the verify pipeline's unit tests.

**Decompression amp.** A crafted `.nar.zst` expands past plausible
memory or CPU. Defense: streaming decompression with a hard byte limit
declared before decode; the stream aborts at the cap. The limit is the
lower of two bounds. The client declares what its frame decodes to in
`x-cachet-nar-bytes`, capped by `NAR_DECOMPRESSED_BYTES_MAX`; because
that declaration is attacker-chosen, it is paired with a bound on how far
the uploaded bytes may expand (`NAR_EXPANSION_RATIO_MAX`), so a bomb can
only spend CPU in proportion to bytes it actually sent. Proven: ruzstd
stream tests with compressed-bomb fixtures, the decode-bound cases in
cachet-core's write tests.

**Forged OIDC.** A presented token with the algorithm switched to
`none`/`HS256`, a wrong `iss` or `aud`, an org the deployment does not
serve, a stale or expired token, an `aud` array, a token minted for
another audience. Defense: the claim policy pins RS256 against
GitHub's JWKS (per-isolate cache, 10-minute TTL, staleness fallback),
requires exact `iss`/`aud` strings, checks `repository_owner` against
the configured orgs, and bounds `iat`/`exp` against the request clock.
Proven: the OIDC rejection matrix in the workerd lane (alg confusion,
expired, wrong audience, wrong org, cold JWKS answering 503) and the
claim policy's tests for the rows that need no wire (the unknown-kid
one-refetch rule, the audience array, mistyped claims).

**Ex-member's token.** A person leaves the org; their access should
die. The bound differs by credential class. A CI job's OIDC token
expires in minutes and is re-verified
against GitHub's JWKS every time. A laptop holds a token this
deployment issued, which is a pointer to a record holding the GitHub
credential it stands for, so every read re-checks membership through
the verdict cache and closes at 600s for an allow and 60s for a deny
(ADR 0002). Access also ends immediately when an operator deletes the
`readtoken/` record or the holder runs `cachet logout`, and in any case
at the record's thirty-day outer bound.

A browser session is the loose one, and it is bounded by what it can
reach rather than by a re-check. The session record holds a login and a
creation time and no GitHub credential, so nothing is re-checked against
GitHub after the mint: the session is accepted until its fourteen-day
expiry or until logout deletes it. What keeps that from being a hole in
the cache is that a session is see-only (ADR 0016). It authenticates the
console's own surface, `/api/self/*`, `/api/whoami`, `/api/probe`, and
`/roots`, and answers 401 on the paths that serve cache bytes, so a
cookie copied out of a browser cannot substitute. An ex-member's session
can read counters and collection reports for up to a fortnight, which is
the accepted trade; it cannot read what the cache holds. Proven:
verdict-caching scenario in the workerd lane (hit counts on the stub
GitHub API), the issued-token scenario's membership-lapse and revocation
rows, the browser-flow scenario's 401 on a narinfo and a NAR against a
live session cookie, and the TTL constants in cachet-core.

**A stolen copy of the deployment's own state.** An attacker reads the
KV namespace or an export of it. What they find: read-token records
keyed by SHA-256, so the issued credentials themselves are not there and
the digests authenticate nothing; and, inside those records, the GitHub
tokens the deployment holds so that membership stays checkable. Those
last are the real prize, and the honest statement is that KV is where
they live, alongside the browser sessions it already held (ADR 0003).
The scope is `read:org`, they belong to members of the deployment's own
organisation, and a deployment whose OAuth App issues non-expiring
tokens stores no refresh token at all. What changed with the exchange is
transit rather than storage: the laptop's GitHub token used to ride
every substitution as an HTTP Basic password, so it was on the wire
thousands of times a build and in the netrc of every machine. Now it is
in one place, at rest. Proven: the exchange scenario asserts the stored
record is keyed by a digest and that the issued token never appears in
KV.

**Browser session takeover paths.** Replay of the OAuth `state`, reuse
of an exchanged `code`, cross-site request forgery against session
cookies, sessions surviving logout. Defense: state records are KV
documents deleted at the start of the callback (consumption precedes
exchange), the session cookie is HttpOnly+Secure+SameSite=Lax, and
logout deletes the KV session. Proven: the browser-flow scenario
(replayed state refused, outsider's forbidden_org, cookie attributes
asserted, logout deletes).

**Multipart abuse.** Parts outside the declared total, reordered or
conflicting completions, upload-id guessing, abandoning uploads to
leak storage. Defense: the worker enforces the declared plan (fixed
part size, one-based numbering, completion consistency against the
record), mints random upload ids, and the collector reaps records
older than the grace window. Proven: the multipart quartet scenario
with replay and abort, the stale-upload reaper in the GC scenario.

**GC sweeping the living.** A collector bug or a poisoned lease file
causes mass deletion. Defense: leases pin roots, the mark phase walks
narinfo references, the sweep requires age past grace, and the gates
abort a run whose picture of what is live is incomplete: an unparseable
lease or root narinfo, a truncated enumeration, an exhausted walk
budget, a corrupt generation document. Narinfos delete before NARs so a
client never sees a dangling narinfo.

A root whose narinfo is absent does not abort the run. No client can
substitute that path, because a substitution starts by fetching the
narinfo that is not there, so nothing reachable only through it is
reachable at all. The path is marked, counted in `unreadableDeep`, and
the walk goes on. Refusing there was permanent, because the lease keeps
naming the path and no later run brings the narinfo back (ADR 0018).

How much one run may delete is not bounded, and a gate on that was
removed rather than kept: it could not be satisfied by the run after it
either, so a deployment with a lot of genuinely dead paths stopped
collecting permanently (ADR 0017). What bounds a mistake is the grace
window, in time rather than in count, and the recovery is a push, because
over-deleting from a cache costs a rebuild rather than data. Proven: the
GC laws in the property lane (reserved keys, marked paths, grace
boundary, NAR survival, and the walk law that only an unparseable root
gates) and the four workerd GC scenarios.

**Credential leaves in artifacts or logs.** The signing key or an OIDC
token lands in the shipped bundle, a log line, or a committed file.
Defense: worker logs never include credential fields; the bundle is
scanned for banned runtimes and secret-shaped strings every green run;
deploy-time secrets enter only through redacted bindings; the
bootstrap env file is 0600 and gitignored. Proven:
`just wasm-hygiene`, the log-hygiene review of the worker's event
vocabulary, and the .gitignore covering `.env*`.

**Release artifact tampering.** The binary the action runs is replaced
or corrupted in transit. Defense: the action downloads the pinned
release archive plus its `.sha256` and verifies before the archive
opens; the release itself is built by cargo-dist from the tag, and the
repository keeps releases immutable, so a published release's tag and
assets can never be replaced after the fact. Proven: the download
step's verification before extraction; a mismatch fails the install
step.

**The counter route's Cloudflare token.** The deployment holds an API
token so an admin can read its own counters, because a worker cannot
read the dataset it writes to: that is Cloudflare's SQL API, and it
takes an account credential. Defense, in three parts. The token is
scoped to reading account analytics and nothing else, so a worker
compromise yields a view of counts the operator already owns and no
power over R2, KV, the worker, or DNS. The route is admin-gated like
every other `/api/self` route. And the caller chooses a question rather
than composing one: `subject`, `by`, and `window` each parse into a
closed enum or are refused, and the statement is built from literals and
enum values, so no caller text reaches SQL that would run with that
token's authority. Filters narrow only the three columns whose values
come from closed vocabularies (`kind`, `outcome`, `actor`); the columns
holding text a pusher chose, `repository` and `reference`, can be
grouped by and never filtered on, because a `GROUP BY` names a column
where a filter names a value. The token is optional; a deployment
without it counts and does not report. Proven: the query builder's tests
(a hostile string parses into no choice at all), the counter route's
rows in the workerd lane covering the anonymous, non-admin, and
unoffered-choice answers, and the lane's stub SQL endpoint, which
receives the composed statement and asserts it whole rather than by
substring, since a clause that moved or a bound that changed would run
with that token behind it.

**Admin API abuse.** A non-admin org member reads GC reports or stats,
or an anonymous caller reaches them. Defense: admin routes require a
resolved identity plus membership in `CACHET_ADMINS`; an org member
outside the list answers 403 forbidden_admin, an anonymous request
401. Proven: the reports-API scenario covering all three answers.

**Supply chain of cachet itself.** A malicious dependency or a
wasm-incompatible crypto crate enters the tree. Defense: cargo deny
(advisories, licenses, bans, with ring reachable only through the rustls
wrappers and the C zstd library only through the native push client,
which never compiles to wasm), exact pins where generation behavior
matters (utoipa), dependabot weekly, and the wasm-hygiene scan over the
shipped bundle, which refuses ring, aws-lc, tokio, and the C zstd
library's symbols in the artifact regardless of what the ban list says.
Proven: `just deny`, `just wasm-hygiene`, the workspace `publish =
false` (no crate ever uploads to crates.io).

## Non-goals

No spray-rate limiting beyond platform defaults and the verdict cache:
authenticated abuse is an operator decision. No content scanning of
store paths. No protection against a GitHub account takeover of an
org member: the org's own authentication policies govern that, and
the verdict TTLs bound how long a revoked token keeps answering.
