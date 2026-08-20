# The workerd lane

The workerd lane runs the built worker bundle under `wrangler dev --local`,
which is miniflare over the real workerd binary: R2, the Cache API,
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

The lane covers the read path today: the handshake body and its headers,
narinfo and NAR serving with wire headers, positive and negative edge
caching through the generation-scoped key space, HEAD semantics,
problem+json rejections by shape and by exact bytes, the corrupt-
generation bypass, and generation-zero behavior on an empty bucket.
Write-path, auth, and GC scenarios join it with the modules that ship
them.

Run it: `just workerd`.
