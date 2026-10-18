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
knows whether its caller is an admin, whose `previousPublicKeys` a
rotated deployment lists and an unrotated one omits, with a malformed
entry refusing the whole document as `auth_unavailable` because `cachet
setup` would otherwise write it into the daemon's trusted keys (ADR
0021), and whose `managedBy` a deployer-managed deployment carries and a
self-hosted one omits; the bulk probe (`POST /api/probe`): the sorted,
deduplicated held-subset answer read from the bucket itself, by one head
per hash for a small question and by one listing per first character for
a large one, each counted in the probe's own event so the lane holds its
cost without a clock (ADR 0024), with NAR and lease objects proven never
to leak in, the answer answering equally for laptop and OIDC credentials,
and its rejection rows (unauthorized, forbidden_org, malformed_probe from
bad JSON, bad hashes, and over-cap entry lists, and the byte cap's
body_too_large);
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
zeroed: one proves the collector sweeps a dead path's narinfo, its NAR,
and the NAR's facts document, spares the leased path, reaps a stale upload, bumps the generation, and
lands its report and stage artifacts, with the bucket state read back
through `wrangler r2 object get`; another proves a collection that
clears the cache clears it, since how much one run may delete is not
bounded (ADR 0017); and two more separate the ways a root narinfo fails
to read, because the collector answers them differently and each answer
needs its own boot (a tripped gate ends the run, so one bucket cannot
show both). A lease naming a path whose narinfo is absent counts one
unreadable reference and sweeps normally, and a lease naming a root whose
narinfo will not parse trips `unreadable_root_narinfo` with the dead
path still in the bucket (ADR 0018). The reports the first run
lands then serve through the admin API: the run list, the report read,
and the stats derivation answer the admin token, 401 the anonymous
request, and 403 forbidden_admin the org member outside CACHET_ADMINS.
The health route answers beside them, reading the run that just landed as
`healthy` with no gate and a countdown to the lane's own hourly cron that
is always ahead of now and never more than an hour out; a scenario that
seeds nothing proves the other half, where `/api/self/stats` answers 404
because a projection with nothing to project has no honest body and
health answers 200 `unknown` because it renders in a header on every
screen. A third scenario covers the staleness bound in both directions
from one seed, a report three hours old against the lane's hourly cron:
with a cursor in the bucket the answer is `healthy`, because a run in
progress is proof the schedule fired and reports land only when a run
concludes, and with that cursor deleted the same report reads `degraded`
(ADR 0019).
The counter route is gated in the same scenario, which runs with no
`.dev.vars` and therefore no analytics token: its rows prove the gate and
the parser refuse before any query runs, that an inadmissible choice
answers 400 malformed_query (a hostile string, a filter naming nothing, a
bucket finer than its window can hold), and that a deployment with no
token answers 503 rather than pretending to report.

The console has a scenario of its own, and it is the only one that boots
against an asset directory. Binding one under miniflare puts Cloudflare's
asset router in front of the worker, and that router has no scheduled
handler, so the collector's dev endpoint answers "exception" with it
bound; the driver therefore generates a copy of the lane config with the
asset block appended, under a name carrying its own process id so two
lanes on one checkout never delete each other's, and boots that scenario
alone against it, rather than committing a second config that would
drift from the first. The
assets themselves are a two-file stand-in rather than a real build
(workerd/fixtures/assets/README.md), because the law under test is which
paths the layer may answer at all. The rows: the root redirects to
`/console`; `/console`, `/console/`, and a deep link all render the
shell; a hashed file is served by its own name and is not the shell; the
shell needs no credential and grants none, so the API behind it still
answers 401; the protocol paths answer exactly as they do without assets,
including a path that merely looks like the console's; and the row the
whole design exists for, that an absent narinfo and an absent NAR still
answer the protocol's own miss, 404 `text/plain` with the negative
caching headers and never the console's shell, while the router's own
fallback keeps its problem+json shape.

A narinfo is answered only beside a NAR the deployment can serve
(ADR 0023), and a scenario of its own seeds the ways a narinfo outlives
its NAR. A narinfo the bucket holds without its NAR answers a GET with the
protocol's own 404, logs `read.narinfo_nar_missing`, and is gone from the
bucket afterwards, and a HEAD and a ranged GET of such a narinfo answer
404 and leave the bucket without it as well. A narinfo the edge cache kept
from before the bucket lost both objects, the state a sweep leaves
behind, answers 404 once the worker finds the NAR in neither the edge
cache nor the bucket, and the next read finds no stale copy to log. An
edge copy whose NAR is gone while the bucket still holds the narinfo
answers 404 and deletes the bucket's copy too, and one whose bucket now
holds a newer narinfo naming a NAR that is there serves the newer one.
When the edge cache kept the NAR too, the narinfo still answers, because
the NAR still serves from there. The CLI push scenario ends by removing a
pushed path's NAR and facts from the bucket: the narinfo read answers
404, and the next push finds the path absent, uploads it again, and both
objects serve.

A write never replaces a NAR the bucket holds with its facts (ADR 0023).
In the write scenario, a single write of a decodable zstd frame that is
not the stored NAR's bytes is refused with `file_hash_mismatch`, and a
HEAD, which reads the bucket rather than the edge, still answers the
stored NAR's length. The NAR's own bytes then answer 204 and log
`write.nar_claimed`, the facts rewrite the collector keeps a NAR by. The
multipart scenario does the same over the NAR it assembled: an upload of
the stranger's bytes assembles under a scratch key and its completion is
refused, and an upload of the NAR's own bytes completes with 204, both
leaving the stored NAR's length as it was.

Three collector scenarios cover what the sweep keeps. Two park a run at
its sweep with two planned NARs, one whose facts document was written
after the plan's cutoff and one with none, the first cursor carrying the
cutoff and the second written before cursors carried one. The run keeps
the claimed NAR and its facts, deletes the other, reports `narsDeleted`
as 1, and leaves the claimed NAR on the held list at `meta/gc-held-nars`.
The third seeds that list with two NARs no narinfo names, under an hour
of grace: the run deletes the one with no facts document and holds the
one written seconds before.

A download that breaks partway has its own scenario, over a three MiB
object of random bytes seeded under a NAR key. The driver reads a MiB of
it, drops the connection, and asks for the rest with `Range: bytes=N-`,
which is what curl sends when nix resumes. The answer must be 206 with
`Content-Range: bytes N-last/size`, and the two halves together must hash
to the seeded object. Around that, a bounded range and a suffix answer
exactly their bytes, a last offset past the end stops at the end, a range
starting at the end and a zero suffix answer 416 `range_not_satisfiable`
with `Content-Range: bytes */size`, ranges the deployment does not serve
(reversed, several, another unit, or sent with `If-Range`) answer the
whole object, a narinfo answers a range the same way, a ranged read
without a credential answers 401, and every whole read and HEAD carries
`Accept-Ranges: bytes`. The CLI push scenario then has nix itself
substitute the path it pushed through a pass-through in the stub that
cuts the NAR download halfway: the copy must succeed, and the stub must
have seen nix resume with a range.

The composite action's provisioning step runs here as the script the
action runs, `action/provision.sh`, against the stub's Actions token
endpoint and the lane's worker. Two stand-ins make it runnable without
root: a `sudo` on the PATH that runs its command as the lane's user, and
a netrc path the lane can write in place of `/etc/nix/netrc`. The stub
issues that run a token that expires five seconds after it is minted,
and the scenario waits until the worker itself refuses the token before
it reads with the password the step wrote to the netrc, which is the
read the nix daemon makes for the rest of a job. That read must answer
200 for a stored narinfo and 404 for an absent one. It is the regression
row for the failure ADR 0022 records, where a job's OIDC token sat in the
netrc and every read after it expired answered 401. The wait is real
time, because workerd gives the worker's clock no seam to move.

Around that row, the scenario asserts the rest of ADR 0022. The issued
credential resolves as the run (`/api/whoami` answers `ci` for the
repository owner), cannot renew a lease or reach the admin surface, and
is stored under its digest as a record holding the run's claims and a
six-hour lifetime and neither the credential nor the OIDC token, with its
job's count naming it. Three more exchanges for the same job succeed and
the fifth answers 403 `credential_limit`, while another job of the run
and a re-run of this one each start a fresh count, and a token missing
its run id, run attempt, or check run id is refused. The revoke action's
post step, `action/revoke/post.cjs`, then ends the step's credential and
a sibling the scenario minted and had already read with, so the
isolate's memo is cleared too, and the job is refused any further
exchange. Rewriting the job's count afterwards, as an exchange that read
it before the revocation would, does not bring the sibling back. One
job's revocation leaves another job of the same run reading, and
eighteen jobs of one run exchanging at once all succeed, which is what
cachet's own ci.yml asks of it. Revoking again answers 404 and the step
says the credential was already gone, a credential never issued answers
404 as well, and a `sudo` that refuses is named as the reason in the
step's log. The stub's pass-through to the worker then answers an
exchange with the HTML 500 Cloudflare serves when a worker throws, and
the step must read only the retried answer and still write the issued
credential. The last two runs of the script cover a deployment that
refuses the exchange, which leaves the job on its OIDC token with the log
line saying the cache stops when that token expires, and a job with no
OIDC grant, which writes no credential and configures no cache.

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

The CLI scenario assumes it is the only writer to the nix store while it
runs, and that holds on CI, where each runner's store belongs to the one
job. `cachet push` uploads every path the store gained since its snapshot
step, as the composite action does in a CI job, and the scenario's checks
count those uploads exactly. On a machine where another nix build
registers a path between the snapshot and a push, the push uploads that
path too and the counting checks fail with more new paths than the
scenario added. On October 14 a build that registered a path between the
two payloads turned the multipart check's one new path into two. The
scenario runs against the machine's own store until it gets a private
chroot store of its own, so a local failure of that shape is this
interference.

Run it: `just workerd`.
