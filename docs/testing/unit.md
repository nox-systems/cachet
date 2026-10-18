# The unit lane

The unit lane runs `cargo nextest` over the whole workspace except
cachet-worker, under the ci profile with zero retries (`.config/nextest.toml`).
It covers every pure function in cachet-core and cachet-crypto: grammar
parsers, document round-trips, claim-policy decisions, and the rejection
matrix. It exists to catch logic, parsing, and state-machine bugs close to
the code that has them.

The binding bijection runs here too (cachet-deploy/tests/roster.rs). It
reads cachet-worker's source and cachet-core's constants from disk and
asserts that every binding name the worker reads is in cachet-deploy's
roster and every rostered name is read, because the worker is wasm32
only and no lane runs its own tests on a host.

The push pipeline's scripted tests (cachet-push's pipeflow module) count
what a push asks of nix and of the wire. A push whose new paths the cache
already holds is held to one store read, one mint, and one probe off the
default branch, and to one evaluation per root and the lease renewal on
it, with no wait and no retry. The tests count calls and never time
them, so a loaded runner cannot fail them.

A test in this lane is one concrete input with one asserted behavior.
Arbitrary-input coverage belongs to the property lane; byte-exact output
coverage belongs to the golden lane. A test that fails intermittently is a
determinism bug in the code or the test, and the fix is never a retry
(CLAUDE.md §9).

Run it: `just unit`.
