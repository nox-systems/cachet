# The workerd lane

The workerd lane runs the built worker bundle under `wrangler dev --local`,
which is miniflare over the real workerd binary: R2, KV, the Cache API,
scheduled invocations, and the module-pipeline semantics are the runtime's
own rather than a mock's. Assertions reach the worker over real HTTP from
`workerd/check.mjs`, a node script with no npm dependencies (the
devshell's node 22 supplies fetch and child_process).

Each scenario gets its own persistence directory: the driver seeds the
local R2 with `wrangler r2 object put --local`, boots the worker on a
free port, asserts, and kills it. Cache behavior is not observed by
introspecting miniflare internals: the worker emits events
(read.edge_hit, read.bucket_hit, read.miss, generation.document_corrupt)
and the driver matches them in the wrangler log stream, so a cache that
silently never stores surfaces as a failure the way it would in
production.

Outbound calls have the same treatment. The driver runs a stub server
that plays four roles: the OIDC JWKS endpoint, holding a per-run RSA
keypair; the OAuth web endpoint, exchanging one known code for a member
token and one for an outsider token; the GitHub API endpoints the
verdict path calls (`/user` and org membership), counting hits so the
scenarios can prove the KV verdict cache serves repeat reads; and
Cloudflare's SQL API, which receives the statement the counter route
composed as plain text and answers rows the scenario chose. The worker
reaches them through the `CACHET_JWKS_URL`, `CACHET_GITHUB_API_URL`,
`CACHET_GITHUB_WEB_URL`, and `CACHET_STATS_API_URL` variables, which the
driver passes with `wrangler dev --var`; auth scenarios mint RS256 tokens
against the stub's private key, so a verification that silently skips
would fail the matrix. The lane's ed25519 signing secret, the OAuth
client secret, and the counter route's analytics token enter as
`.dev.vars`, the same way a deployment's secrets enter as bindings; the
file is written before the scenarios that need it, deleted when the lane
ends, and gitignored.

The lane binds `CACHET_EVENTS`, so `stats::emit` runs its real path in
every scenario rather than returning early on a missing binding: each
counted request builds its point and marshals its blobs and doubles.
workerd discards the point, so what the lane proves is the marshalling
and not the storage, and it proves it negatively, by asserting that no
`stats.write_failed` event appears beside the reads and writes that
produced one.

The lane covers the read path, the write path, and the API surface so
far: the handshake body and its headers; narinfo and NAR serving with
wire headers; positive and negative edge caching through their two key
spaces (a stored object's entry carries no generation and survives a
sweep, a cached absence carries one and does not); HEAD semantics;
problem+json rejections by shape and by exact bytes; the
corrupt-generation bypass, which degrades the negative half only;
generation-zero behavior on an empty bucket; the OIDC rejection matrix
(wrong org, alg confusion, staleness, expired tokens); guard ordering
(411 before 413, 401 before 411); the verify-then-sign pipeline end to
end, from a NAR upload through a signed narinfo whose signatures both
verify against the lane's own public key (nix's fingerprint recipe
re-derived in the driver: a lane asserting the Sig line's name alone
armors the text while the bytes are free to rot) and the file facts; the
write-time measurement that pipeline rests on, where a NAR write that
omits its declared decompressed size answers 411, bytes that do not hash
to the key naming them are refused and deleted, the facts document beside
a stored NAR is unreachable from any request, and re-pushing a narinfo
this deployment already signed leaves its signature list unchanged;
the multipart quartet with its record, part-size enforcement, replay, and
abort; the CLI's own push, driven as the composite action drives it
(snapshot step, `nix-store --add-fixed` payload, push) with the stub
minting its OIDC tokens, asserting that the path the client serialized
and compressed itself reaches the bucket and serves back signed; read verdicts cached in KV for both the admit and the deny
direction, with OIDC tokens answering reads as CI expects; the isolate's
decision memo, proven by deleting the KV verdict between reads and
watching the repeat in both directions answer with no GitHub API hit and
an `auth.memo_hit` event; lease renewal bound to the token's own claims with
forbidden_ref and forbidden_project refusals; the project listing; the
public config document, whose identity fields (the deployment's name, the
worker's version, and the absence of a build stamp and a font stylesheet
on a build that has neither) are what a console header reads before it
knows whether its caller is an admin; the bulk probe (`POST /api/probe`): the sorted,
deduplicated held-subset answer derived from the bucket enumeration
itself, with NAR and lease objects proven never to leak in, the answer
answering equally for laptop and OIDC credentials, and its rejection
rows (unauthorized, forbidden_org, malformed_probe from bad JSON, bad
hashes, and over-cap entry lists, and the byte cap's body_too_large);
the browser login flow, from the login
redirect's exact parameters through state consumption (a replayed state
never reaches the exchange), the outsider's forbidden_org refusal, the
session cookie's attributes, and logout's session deletion, with the
session proven see-only: it answers `/api/whoami` with its own login,
standing, and expiry and reads the admin surface, and it answers 401
unauthorized on a narinfo and on a NAR, because a cookie a browser sends
by itself and holds for a fortnight without re-checking membership must
not substitute from the cache; and the counter route's answering half,
where the stub receives the composed statement and the lane asserts it
whole rather than by substring, since it runs with an account token
behind it: a dimension list, a filtered question, every filter stacked in
column order, a daily series gap-filled to one row per day with the
reported buckets landing where they belong, an hourly series bounded at
twenty-four, and an upstream refusal answering 503 without repeating what
upstream said. The served
OpenAPI document is asserted byte-identical to the committed one, which is
the served half of the drift bijection (CLAUDE.md §8). The GC scenarios
invoke the scheduled handler over wrangler's dev endpoint with grace
zeroed: one proves the collector sweeps a dead path's narinfo and NAR,
spares the leased path, reaps a stale upload, bumps the generation, and
lands its report and stage artifacts, with the bucket state read back
through `wrangler r2 object get`; the other proves the fraction gate
aborts a wholesale sweep with nothing deleted. The reports the first run
lands then serve through the admin API: the run list, the report read,
and the stats derivation answer the admin token, 401 the anonymous
request, and 403 forbidden_admin the org member outside CACHET_ADMINS.
The health route answers beside them, reading the run that just landed as
`healthy` with no gate and a countdown to the lane's own cron at 05:00
UTC that is always ahead of now; a scenario that seeds nothing proves the
other half, where `/api/self/stats` answers 404 because a projection with
nothing to project has no honest body and health answers 200 `unknown`
because it renders in a header on every screen.
The counter route is gated in the same scenario, which runs with no
`.dev.vars` and therefore no analytics token: its rows prove the gate and
the parser refuse before any query runs, that an inadmissible choice
answers 400 malformed_query (a hostile string, a filter naming nothing, a
bucket finer than its window can hold), and that a deployment with no
token answers 503 rather than pretending to report.

Every code in the error table (cachet-core/src/error.rs) has at least one
wire-level assertion here. storage_unavailable is asserted through the
counter route, whose upstream the driver controls: the stub answers 500
and the route answers 503 without repeating what upstream said. Its other
trigger, the platform's own R2 or KV failing, has no deterministic
stand-in under wrangler --local, so the problem body itself is pinned
byte-for-byte in the golden lane. A
uniform Authorization-header contract holds on both paths: a
present-but-unparseable credential (oversized header, wrong scheme)
answers 400 malformed_auth, a parseable-but-wrong one answers 401
unauthorized.

Run it: `just workerd`.
