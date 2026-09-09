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

A test in this lane is one concrete input with one asserted behavior.
Arbitrary-input coverage belongs to the property lane; byte-exact output
coverage belongs to the golden lane. A test that fails intermittently is a
determinism bug in the code or the test, and the fix is never a retry
(CLAUDE.md §9).

Run it: `just unit`.
