# The kani lane

The kani lane runs Kani bounded model checking over the branch-tight cores
where a proof is worth more than a corpus. Proofs live colocated with the
code they prove as `#[cfg(kani)]` harnesses, so a rustdoc reader sees them
where they apply.

Only proofs over a handful of scalar symbolic inputs belong here:
arithmetic identities and bound checks with shallow decision structure,
where verification finishes in seconds. Two kinds of code are out of
scope, both measured, not guessed. Code that mutates std collections
under nondeterminism costs the verifier symbolic heap shape (which
objects exist, which pointers alias) rather than the law under test.
Deep symbolic value chains, such as a full codec round trip or elliptic
curve arithmetic over symbolic keys, bit-blast into formulas the solver
cannot finish. A gate that cannot finish cannot gate (CLAUDE.md §0), so
those laws live elsewhere: codec dialects are pinned byte-for-byte by
the golden lane against real nix output, and decision spaces small
enough to exhaust are exhausted in the property lane.

Today the lane proves the multipart part-plan laws: for any admissible
total the plan sums exactly and stays within the cap, and for any
inadmissible total the refusal is typed.

scripts/list-kani-crates.sh derives the crate list by grepping for
`cfg(kani)`: a proof joins the lane by existing, and no list can drift.
The lane fails loudly when the list is empty.

The verifier is pinned and installed on demand because no nixpkgs package
exists (justfile). Verification includes the default panics and
unwinding-completeness checks, so a harness passes only when its own
assertions and those checks hold.

Run it: `just kani`, or `just kani <crate>` for one crate.
