# The workerd lane

The workerd lane runs the built worker bundle under `wrangler dev --local`,
which is miniflare over the real workerd binary: R2, KV, the Cache API,
waitUntil draining, and the module-pipeline semantics are the runtime's
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
that plays three roles: the OIDC JWKS endpoint, holding a per-run RSA
keypair; the OAuth web endpoint, exchanging one known code for a member
token and one for an outsider token; and the GitHub API endpoints the
verdict path calls (`/user` and org membership), counting hits so the
scenarios can prove the KV verdict cache serves repeat reads. The worker
reaches them through the `CACHET_JWKS_URL`, `CACHET_GITHUB_API_URL`, and
`CACHET_GITHUB_WEB_URL` variables, which the driver passes with
`wrangler dev --var`; auth scenarios mint RS256 tokens against the
stub's private key, so a verification that silently skips would fail the
matrix. The lane's ed25519 signing secret and the OAuth client secret
enter as `.dev.vars`, the same way a deployment's secrets enter as
bindings; the file is written before the write scenarios, deleted when
the lane ends, and gitignored.

The lane covers the read path, the write path, and the API surface so
far: the handshake body and its headers; narinfo and NAR serving with
wire headers; positive and negative edge caching through the
generation-scoped key space; HEAD semantics; problem+json rejections by
shape and by exact bytes; the corrupt-generation bypass; generation-zero
behavior on an empty bucket; the OIDC rejection matrix (wrong org, alg
confusion, staleness, expired tokens); guard ordering (411 before 413,
401 before 411); the verify-then-sign pipeline end to end, from a NAR
upload through a signed narinfo with both signatures and the file facts;
the multipart quartet with its record, part-size enforcement, replay, and
abort; read verdicts cached in KV for both the admit and the deny
direction; lease renewal bound to the token's own claims with
forbidden_ref and forbidden_project refusals; the project listing; the
public config document; and the browser login flow, from the login
redirect's exact parameters through state consumption (a replayed state
never reaches the exchange), the outsider's forbidden_org refusal, the
session cookie's attributes, and logout's session deletion. The served
OpenAPI document is asserted byte-identical to the committed one, which is
the served half of the drift bijection (CLAUDE.md §8). GC scenarios join
it with the collector.

Run it: `just workerd`.
